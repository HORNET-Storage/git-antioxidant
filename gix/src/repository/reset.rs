//! Move `HEAD` and reset the index and the worktree, as `git reset` does.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_hash::ObjectId;
use gix_ref::{
    Target,
    transaction::{PreviousValue, RefEdit},
};

use super::commit::LockedIndex;
use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    commit::message::normalize_reflog_message,
    config::{cache::util::ApplyLeniencyDefault, tree::Core},
    error::{ResultExt, bail, message},
    reset::{Changes, Error as Refused, Mode, Options, Outcome},
};

impl Repository {
    /// Reset `HEAD` to `commit`, and the index and the worktree as `mode` says, as `git reset --<mode> <commit>` does, or
    /// to the commit of `HEAD` if `commit` is `None`, as `git reset --<mode>` does. This follows `cmd_reset()`,
    /// `reset_index()` and `reset_refs()` in `builtin/reset.c`.
    ///
    /// * `commit` is peeled to a commit, as Git resolves the commit to reset to. On an unborn branch without `commit`,
    ///   the index and the worktree are reset to the empty tree, and `HEAD` stays unborn.
    /// * The index and the worktree change as [`gix_worktree_state::reset()`] and [`gix_worktree_state::reset::mixed()`]
    ///   change them for `mode`, while the index is locked, and the index is written afterwards.
    /// * Then `ORIG_HEAD` points to the previous commit of `HEAD`, or is deleted if `HEAD` was unborn, and `HEAD`, or its
    ///   branch, moves to the commit with the reflog message `reset: moving to <revision>`, which names
    ///   [`Options::revision`].
    /// * Last, the state of a merge goes away as `remove_branch_state()` in `branch.c` removes it: `SQUASH_MSG`,
    ///   `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE` and the pseudo-reference `AUTO_MERGE`. Failures to remove them
    ///   are ignored, as in Git.
    ///
    /// `should_interrupt` stops a hard, merge or keep reset as long as the worktree didn't change yet.
    ///
    /// ### Refusals
    ///
    /// Nothing changes and a [`reset::Error`](crate::reset::Error) is returned where Git refuses the reset: a soft or keep
    /// reset while `MERGE_HEAD` exists or the index has conflicts, a keep reset while `HEAD` has no commit, a reset of the
    /// index or the worktree without worktree, and a hard, merge or keep reset that would lose the changes or untracked
    /// files of paths, which [`Refused`](crate::reset::Error::Refused) lists.
    ///
    /// ### Deviation
    ///
    /// * The deviations of [`gix_worktree_state::reset()`] apply, among them that the entries of the index aren't
    ///   refreshed.
    /// * `CHERRY_PICK_HEAD`, `REVERT_HEAD` and the state of the sequencer stay, which Git removes with
    ///   `sequencer_post_commit_cleanup()`, and `MERGE_AUTOSTASH` isn't saved to the stash.
    #[doc(alias = "git reset")]
    pub fn reset(
        &self,
        commit: Option<ObjectId>,
        mode: Mode,
        options: Options,
        should_interrupt: &AtomicBool,
    ) -> Result<Outcome> {
        let git_dir = self.current_dir().join(self.git_dir());
        let mut head = self.head()?;
        let symbolic_head = !head.is_detached();
        let previous = if head.is_unborn() {
            None
        } else {
            Some(head.peel_to_commit()?.id)
        };
        let (target, tree) = match commit {
            Some(id) => {
                let commit = self.find_object(id)?.peel_to_commit()?;
                let tree = commit.tree_id()?.detach();
                (Some(commit.id), tree)
            }
            None => match previous {
                Some(id) => (Some(id), self.find_commit(id)?.tree_id()?.detach()),
                None => (None, self.empty_tree().id),
            },
        };
        let revision = options.revision.clone().unwrap_or_else(|| match commit {
            Some(id) => id.to_string().into(),
            None => "HEAD".into(),
        });
        if mode != Mode::Soft && self.workdir().is_none() {
            bail!(Refused::BareRepository { mode });
        }
        let merging = git_dir
            .join("MERGE_HEAD")
            .try_exists()
            .or_raise(|| message("Could not check whether MERGE_HEAD exists"))?;

        let changes = match mode {
            Mode::Soft => {
                if merging || has_conflicts(&self.read_index_file()?) {
                    bail!(Refused::InMerge { mode });
                }
                Changes::default()
            }
            Mode::Mixed => {
                let LockedIndex { lock, mut index } = self.lock_index()?;
                let changes = gix_worktree_state::reset::mixed(
                    &mut index,
                    &tree,
                    &self.objects,
                    |_| true,
                    self.config.protect_options()?,
                    self.config.stat_options()?,
                )?;
                self.write_locked_index(lock, &index)?;
                changes
            }
            Mode::Hard | Mode::Merge | Mode::Keep => {
                let LockedIndex { lock, mut index } = self.lock_index()?;
                let worktree_mode = match mode {
                    Mode::Keep => {
                        if merging || has_conflicts(&index) {
                            bail!(Refused::InMerge { mode });
                        }
                        let Some(head) = previous else {
                            bail!(Refused::InvalidHead);
                        };
                        gix_worktree_state::reset::Mode::Keep {
                            head_tree: self.find_commit(head)?.tree_id()?.detach(),
                        }
                    }
                    Mode::Merge => gix_worktree_state::reset::Mode::Merge,
                    Mode::Soft | Mode::Mixed | Mode::Hard => gix_worktree_state::reset::Mode::Hard,
                };
                match self.reset_worktree(&mut index, &tree, worktree_mode, should_interrupt)? {
                    gix_worktree_state::reset::Outcome::Reset(changes) => {
                        self.write_locked_index(lock, &index)?;
                        changes
                    }
                    gix_worktree_state::reset::Outcome::Refused(refusals) => {
                        bail!(Refused::Refused { revision, refusals })
                    }
                }
            }
        };

        if let Some(target) = target {
            self.reset_refs(
                target,
                previous,
                symbolic_head,
                revision.as_bstr(),
                options.reflog_action.as_ref().map(|action| action.as_bstr()),
            )?;
        }
        self.remove_branch_state(&git_dir);
        Ok(Outcome {
            commit_id: target,
            previous,
            changes,
        })
    }

    /// Reset the entries of the index at the paths that `patterns` match to those of `tree`, or of the commit of `HEAD` if
    /// `tree` is `None`, as `git reset [<tree-ish>] -- <pathspec>` does with `read_from_tree()` in `builtin/reset.c`,
    /// without changing the worktree, `HEAD` or the state of a merge. Return what changed in the index.
    ///
    /// `tree` is peeled to a tree, as Git resolves a tree-ish there. On an unborn branch without `tree`, the entries of
    /// the matching paths are removed, as if `HEAD` had the empty tree. The patterns are pathspecs relative to the current
    /// working directory in the worktree, as those of Git are. Without patterns, all entries inside of that directory are
    /// reset, which are all entries of the index in the root of the worktree.
    ///
    /// ### Deviation
    ///
    /// The deviations of [`gix_worktree_state::reset::mixed()`] apply, among them that the entries of the index aren't
    /// refreshed.
    pub fn reset_paths(
        &self,
        tree: Option<ObjectId>,
        patterns: impl IntoIterator<Item = impl AsRef<BStr>>,
    ) -> Result<Changes> {
        let tree = match tree {
            Some(id) => self.find_object(id)?.peel_to_tree()?.id,
            None => {
                let mut head = self.head()?;
                if head.is_unborn() {
                    self.empty_tree().id
                } else {
                    head.peel_to_commit()?.tree_id()?.detach()
                }
            }
        };
        if self.workdir().is_none() {
            bail!(Refused::BareRepository { mode: Mode::Mixed });
        }
        let LockedIndex { lock, mut index } = self.lock_index()?;
        let mut pathspec = self.pathspec(
            true,
            patterns,
            false,
            &index,
            gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )?;
        let changes = gix_worktree_state::reset::mixed(
            &mut index,
            &tree,
            &self.objects,
            |path| pathspec.is_included(path, Some(false)),
            self.config.protect_options()?,
            self.config.stat_options()?,
        )?;
        self.write_locked_index(lock, &index)?;
        Ok(changes)
    }

    /// Return what a switch or reset of `index` and the worktree needs from this repository: the root of the worktree, the
    /// real path of the Git directory, the attributes and exclude patterns, and the options of the checkout.
    pub(crate) fn worktree_context(&self, index: &gix_index::State) -> Result<WorktreeContext> {
        let current_dir = self.current_dir();
        let worktree = current_dir.join(
            self.workdir()
                .ok_or_else(|| message("The repository has no worktree").validation_error())?,
        );
        let git_dir_realpath = gix_path::realpath(current_dir.join(self.git_dir()))
            .or_raise(|| message("Could not resolve the path of the Git directory"))?;
        let stack = self
            .attributes(
                index,
                gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
                gix_worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
                None,
            )?
            .detach();
        let fscache = Core::FS_CACHE
            .enrich_error(self.config.resolved.boolean(Core::FS_CACHE))
            .with_lenient_default(self.config.lenient_config)?
            .unwrap_or_default();
        let options = gix_worktree_state::switch::Options {
            checkout: self.checkout_options(gix_worktree::stack::state::attributes::Source::IdMappingThenWorktree)?,
            fscache,
        };
        Ok(WorktreeContext {
            worktree,
            git_dir_realpath,
            stack,
            options,
        })
    }

    /// Reset `index` and the worktree to `tree` in `mode` with [`gix_worktree_state::reset()`].
    fn reset_worktree(
        &self,
        index: &mut gix_index::State,
        tree: &gix_hash::oid,
        mode: gix_worktree_state::reset::Mode,
        should_interrupt: &AtomicBool,
    ) -> Result<gix_worktree_state::reset::Outcome> {
        let WorktreeContext {
            worktree,
            git_dir_realpath,
            stack,
            options,
        } = self.worktree_context(index)?;
        let objects = self
            .objects
            .clone()
            .into_arc()
            .or_raise(|| message("Could not share the object database for the reset"))?;
        gix_worktree_state::reset(
            index,
            &worktree,
            tree,
            mode,
            objects,
            gix_worktree_state::switch::Context {
                stack,
                git_dir_realpath: &git_dir_realpath,
                current_dir: self.current_dir(),
                should_interrupt,
            },
            options,
        )
    }

    /// Point `ORIG_HEAD` to `previous`, or delete it without `previous`, and `HEAD` to `target`, as `reset_refs()` does,
    /// with the reflog messages of `set_reflog_message()`.
    fn reset_refs(
        &self,
        target: ObjectId,
        previous: Option<ObjectId>,
        symbolic_head: bool,
        revision: &BStr,
        reflog_action: Option<&BStr>,
    ) -> Result<()> {
        let reflog_message = |action: &str, revision: Option<&BStr>| -> BString {
            let mut message = BString::default();
            match (reflog_action, revision) {
                (Some(reflog_action), _) => {
                    message.extend_from_slice(reflog_action);
                    message.extend_from_slice(b": ");
                    message.extend_from_slice(action.as_bytes());
                }
                (None, Some(revision)) => {
                    message.extend_from_slice(b"reset: moving to ");
                    message.extend_from_slice(revision);
                }
                (None, None) => {
                    message.extend_from_slice(b"reset: ");
                    message.extend_from_slice(action.as_bytes());
                }
            }
            normalize_reflog_message(message.as_bstr())
        };
        let orig_head_name = gix_ref::FullName::try_from("ORIG_HEAD").or_error()?;
        let orig_head = self
            .try_find_reference("ORIG_HEAD")?
            .filter(|reference| reference.name().as_bstr() == "ORIG_HEAD")
            .map(|reference| reference.inner.target);
        match previous {
            Some(previous) => {
                self.edit_reference(RefEdit::update(
                    orig_head_name,
                    previous,
                    orig_head.map_or(PreviousValue::Any, PreviousValue::MustExistAndMatch),
                    reflog_message("updating ORIG_HEAD", None),
                ))?;
            }
            None => {
                if let Some(orig_head) = orig_head {
                    self.edit_reference(RefEdit::delete(
                        orig_head_name,
                        PreviousValue::MustExistAndMatch(orig_head),
                    ))?;
                }
            }
        }
        let head_message = reflog_message("updating HEAD", Some(revision));
        let expected = previous.map_or(PreviousValue::Any, |previous| {
            PreviousValue::MustExistAndMatch(Target::Object(previous))
        });
        self.edit_reference(
            RefEdit::update(
                gix_ref::FullName::try_from("HEAD").or_error()?,
                target,
                expected,
                head_message.clone(),
            )
            .with_deref(true),
        )?;
        if symbolic_head && previous == Some(target) {
            self.log_unchanged_head(target, head_message.as_bstr())?;
        }
        Ok(())
    }

    /// Append the reflog entry of a symbolic `HEAD` that Git writes when `HEAD` is updated to the commit that it points to
    /// already: the references of Git log the update of `HEAD` even then, and only skip the branch, while `gix-ref` writes
    /// no entry for an update that keeps the value. Nothing is written while reflogs are disabled, as `gix-ref` writes
    /// none then.
    fn log_unchanged_head(&self, id: ObjectId, log_message: &BStr) -> Result<()> {
        if self.refs.write_reflog == gix_ref::store::WriteReflog::Disable {
            return Ok(());
        }
        let committer = self
            .committer()
            .ok_or_else(|| message("Committer identity is not configured").validation_error())??;
        let mut line = format!("{id} {id} ").into_bytes();
        committer.trim().write_to(&mut line).or_error()?;
        if !log_message.is_empty() {
            line.push(b'\t');
            line.extend_from_slice(log_message);
        }
        line.push(b'\n');
        let path = self.current_dir().join(self.git_dir()).join("logs").join("HEAD");
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .or_raise(|| message!("Could not create the directory of the reflog at {}", path.display()))?;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&line))
            .or_raise(|| message!("Could not append to the reflog at {}", path.display()))?;
        Ok(())
    }

    /// Remove the state of a merge after a reset without paths, as `remove_branch_state()` in `branch.c` removes it:
    /// `SQUASH_MSG`, then `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE` and the pseudo-reference `AUTO_MERGE`. As in
    /// Git, failures are ignored.
    fn remove_branch_state(&self, git_dir: &Path) {
        for name in ["SQUASH_MSG", "MERGE_HEAD", "MERGE_RR", "MERGE_MSG", "MERGE_MODE"] {
            let _ = std::fs::remove_file(git_dir.join(name));
        }
        let _ = self.delete_auto_merge();
    }
}

/// What a switch or a reset of the index and the worktree needs from a repository.
pub(crate) struct WorktreeContext {
    /// The root of the worktree.
    pub(crate) worktree: PathBuf,
    /// The real path of the Git directory.
    pub(crate) git_dir_realpath: PathBuf,
    /// The attributes and exclude patterns of the worktree and the index.
    pub(crate) stack: gix_worktree::Stack,
    /// The options of the checkout of files.
    pub(crate) options: gix_worktree_state::switch::Options,
}

/// Return `true` if `index` has entries of conflicts.
fn has_conflicts(index: &gix_index::File) -> bool {
    index.entries().iter().any(|entry| entry.stage_raw() != 0)
}
