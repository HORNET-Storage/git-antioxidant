//! Merge a commit into `HEAD` as `git merge` does, and conclude, abort or quit a merge that stopped before its commit.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_hash::ObjectId;
use gix_ref::transaction::{PreviousValue, RefEdit};

use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    commit::{
        index::{Error as CommitRefusal, Merge as MergeCommit, Options as CommitOptions},
        message::{Cleanup, CommentPrefix, normalize_reflog_message},
    },
    config::tree::{Key, Merge as MergeKeys},
    error::{ResultExt, bail, message},
    merge::into_head::{Changes, Error, FastForward, Options, Outcome, Refusals},
    repository::{commit::LockedIndex, reset::WorktreeContext},
    status::tree_index::TrackRenames,
};

/// The flag of the revision walk of a squash for a commit that was queued, as `SEEN` in `revision.h`.
const SEEN: u8 = 1 << 0;
/// The flag of the revision walk of a squash for a commit whose parents were looked at, as `ADDED` in `revision.h`.
const ADDED: u8 = 1 << 1;
/// The flag of the revision walk of a squash for a commit that `HEAD` reaches, as `UNINTERESTING` in `revision.h`.
const UNINTERESTING: u8 = 1 << 2;
/// How many more commits the revision walk of a squash takes from its queue once only uninteresting ones are left in it,
/// as `SLOP` in `revision.c`.
const SLOP: usize = 5;

impl Repository {
    /// Merge the commit that `name` names into `HEAD` as `git merge <name>` does with its default strategy, `ort`, following
    /// `cmd_merge()` in `builtin/merge.c`, and return what was done.
    ///
    /// `name` is resolved as a revision, with `-` standing for `@{-1}`. It names the merged commit in the default message
    /// of the merge, in the conflict markers and in the reflog messages, as Git names it there.
    ///
    /// * If `HEAD` is an unborn branch, the index and the worktree switch from the empty tree to the tree of the merged
    ///   commit, as `git read-tree -m -u` does for Git, and the branch is created at the merged commit with the reflog
    ///   message `initial pull`.
    /// * Otherwise `ORIG_HEAD` points to the commit of `HEAD` first, without reflog entry, as Git sets it before it
    ///   decides how to merge.
    /// * If `HEAD` contains the merged commit, the state of an earlier merge goes away as `remove_merge_branch_state()`
    ///   removes it, and [`Outcome::UpToDate`] is returned.
    /// * If `HEAD` is an ancestor of the merged commit and the merge may fast-forward, the index and the worktree switch
    ///   to the tree of the merged commit with the two-way merge of `checkout_fast_forward()` in `merge.c`, and `HEAD`,
    ///   or its branch, points to the merged commit with the reflog message `<action>: Fast-forward`, as `finish()` moves
    ///   it. Then `post-merge` runs with `0`, and the state of an earlier merge goes away.
    /// * Otherwise the commits are merged with [`merge_commits()`](Self::merge_commits()), with the sides of conflicts
    ///   labelled `HEAD` and `name`, and the result is applied to the index and the worktree with
    ///   [`apply_merge_result()`](Self::apply_merge_result()). Without conflicts, the merge is committed with
    ///   [`commit_index()`](Self::commit_index()) as [`Merge`](crate::commit::index::Merge) describes, as
    ///   `finish_automerge()` commits it, with [`Options::message`] or the message of `fmt_merge_msg()`, like
    ///   `Merge branch 'topic' into feature`, and the reflog message `<action>: Merge made by the 'ort' strategy.`.
    /// * A merge with conflicts or with [`Options::no_commit`] leaves its state as `write_merge_state()` writes it:
    ///   `MERGE_HEAD` with the merged commit, or the merged annotated tag, `MERGE_MODE`, and `MERGE_MSG` with the message
    ///   and a line feed, to which the hint about the conflicts is appended as `suggest_conflicts()` appends it.
    /// * With [`Options::squash`], `HEAD` doesn't move, and neither a commit nor the state of a merge is made, but
    ///   `$GIT_DIR/SQUASH_MSG` lists the merged commits that `HEAD` doesn't contain, as `squash_message()` writes it,
    ///   and `post-merge` runs with `1`. A hint about conflicts goes into `MERGE_MSG` as well.
    ///
    /// The action is [`Options::reflog_action`], or `merge <name>`. The hooks receive it as `GIT_REFLOG_ACTION`, and
    /// `GITHEAD_<id of the merged commit>` with `name`, as `git merge` exports them. Without the `hooks` feature of
    /// `gix`, no hook runs. With it, nothing changes in a repository whose Git directory isn't fully trusted, as
    /// `Repository::hooks()` only returns the hooks of trusted ones.
    ///
    /// `should_interrupt` stops the switch of the index and the worktree as long as the worktree didn't change yet.
    ///
    /// ### Refusals
    ///
    /// Where `git merge` refuses or stops, an [`Error`](crate::merge::into_head::Error) with Git's message is returned:
    /// before anything changes in a bare repository, with unmerged entries, while a merge or cherry-pick is in
    /// progress, for a squash that may not fast-forward, a squash or a merge that may not fast-forward into an unborn
    /// branch, and for a name that names no commit; after `ORIG_HEAD` was set for unrelated histories unless
    /// [`Options::allow_unrelated_histories`] is set, for a merge that can't fast-forward with [`FastForward::Only`], for
    /// staged changes, and for changes or untracked files in the way of the switch, which then changes nothing. If a hook
    /// stops the merge commit or its message is empty, the merge was applied, and its state stays for `git commit`.
    ///
    /// ### Deviation
    ///
    /// * Only one commit is merged, with the strategy `ort`. `--autostash`, `git rerere`, `--signoff`, `--log` and
    ///   `merge.log`, `--verify-signatures`, `branch.<name>.mergeOptions`, `--into-name` and the merge of `FETCH_HEAD`
    ///   aren't supported, nor is a merge in the middle of a rebase.
    /// * Nothing is printed, so neither `merge.verbosity` nor `merge.stat` are read, and no maintenance runs.
    /// * The entries of the index aren't refreshed, and its resolve-undo extension isn't removed, as Git does it.
    /// * Where a merge is refused after it started, Git resets the index and the worktree and puts back the local
    ///   changes it saved with `git stash`, whose merge writes `AUTO_MERGE`, while here nothing changes.
    /// * Names that Git expands for messages, like `@{-1}`, are used as given.
    /// * The signature of a merged signed tag and its verification aren't added to the default message, where Git
    ///   adds them as comments.
    /// * A fast-forward with an empty [`Options::message`] moves `HEAD`, where Git says
    ///   `No merge message -- not updating HEAD` unless it is quiet.
    /// * Of `merge.ff`, only the last value of each `merge` section is read, and a `merge.suppressDest` without value
    ///   removes the patterns before it, where Git refuses it.
    /// * A fast-forward of an unborn branch refuses with the messages of `checkout_fast_forward()`, where Git's
    ///   `git read-tree` words them differently.
    #[doc(alias = "git merge")]
    pub fn merge_into_head(
        &self,
        name: impl AsRef<BStr>,
        options: Options,
        should_interrupt: &AtomicBool,
    ) -> Result<Outcome> {
        let name = name.as_ref();
        let name: &BStr = if name == "-" { "@{-1}".into() } else { name };
        let git_dir = self.merge_git_dir()?;
        if self
            .read_index_file()?
            .entries()
            .iter()
            .any(|entry| entry.stage_raw() != 0)
        {
            bail!(Error::UnmergedEntries);
        }
        if exists(&git_dir.join("MERGE_HEAD"))? {
            bail!(Error::MergeInProgress);
        }
        if exists(&git_dir.join("CHERRY_PICK_HEAD"))? {
            bail!(Error::CherryPickInProgress);
        }
        let mut fast_forward = match options.fast_forward {
            Some(fast_forward) => fast_forward,
            None => self.configured_fast_forward(),
        };
        if options.squash && fast_forward == FastForward::Never {
            bail!(Error::SquashWithNoFastForward);
        }

        let mut head = self.head()?;
        if head.is_unborn() {
            if options.squash {
                bail!(Error::SquashIntoUnborn);
            }
            if fast_forward == FastForward::Never {
                bail!(Error::NoFastForwardIntoUnborn);
            }
            let merged = self.merged(name)?;
            return self.merge_into_unborn(merged.commit, should_interrupt);
        }
        let into: BString = match head.referent_name() {
            Some(referent) => {
                let referent = referent.as_bstr().as_bytes();
                referent.strip_prefix(b"refs/heads/").unwrap_or(referent).into()
            }
            None => "HEAD".into(),
        };
        let head_commit = head.peel_to_commit()?;
        let head_id = head_commit.id;
        let head_tree = head_commit.tree_id()?.detach();
        let merged = self.merged(name)?;

        let action = options.reflog_action.clone().unwrap_or_else(|| {
            let mut action = BString::from("merge ");
            action.extend_from_slice(name);
            action
        });
        let hooks = MergeHooks {
            #[cfg(feature = "hooks")]
            hooks: self.hooks()?,
            env: vec![
                ("GIT_REFLOG_ACTION".into(), os_string(action.as_bstr())?),
                (format!("GITHEAD_{}", merged.commit).into(), os_string(name)?),
            ],
        };
        if fast_forward != FastForward::Only && self.is_throwaway_tag(&merged)? {
            fast_forward = FastForward::Never;
        }

        let cache = self.commit_graph_if_enabled()?;
        let mut graph: gix_revwalk::Graph<'_, '_, gix_revwalk::graph::Commit<gix_revision::merge_base::Flags>> =
            self.revision_graph(cache.as_ref())?;
        let bases: Vec<ObjectId> = gix_revision::merge_base(head_id, &[merged.commit], &mut graph)?
            .map(|bases| bases.iter().copied().collect())
            .unwrap_or_default();
        self.edit_reference(RefEdit::update(
            gix_ref::FullName::try_from("ORIG_HEAD").or_error()?,
            head_id,
            PreviousValue::Any,
            "updating ORIG_HEAD",
        ))?;

        if bases.is_empty() {
            if !options.allow_unrelated_histories {
                bail!(Error::UnrelatedHistories);
            }
        } else if bases == [merged.commit] {
            self.remove_merge_state(&git_dir);
            return Ok(Outcome::UpToDate);
        } else if fast_forward != FastForward::Never && bases == [head_id] {
            let tree = self.find_commit(merged.commit)?.tree_id()?.detach();
            let changes = match self.switch_to_tree(head_tree, tree, should_interrupt)? {
                Ok(changes) => changes,
                Err(refusals) => bail!(Error::FastForwardRefused { refusals }),
            };
            if options.squash {
                self.write_squash_message(&git_dir, head_id, merged.commit)?;
            } else {
                let mut reflog_message = action;
                reflog_message.extend_from_slice(if options.message.is_some() {
                    b": Fast-forward (no commit created; -m option ignored)".as_slice()
                } else {
                    b": Fast-forward".as_slice()
                });
                self.edit_reference(
                    RefEdit::update(
                        gix_ref::FullName::try_from("HEAD").or_error()?,
                        merged.commit,
                        PreviousValue::MustExistAndMatch(gix_ref::Target::Object(head_id)),
                        normalize_reflog_message(reflog_message.as_bstr()),
                    )
                    .with_deref(true),
                )?;
            }
            let post_merge = hooks.post_merge(options.squash);
            self.remove_merge_state(&git_dir);
            post_merge?;
            return Ok(Outcome::FastForward {
                previous: Some(head_id),
                commit_id: merged.commit,
                changes,
            });
        }
        if fast_forward == FastForward::Only {
            bail!(Error::NotPossibleToFastForward);
        }
        // As `git_committer_info(IDENT_STRICT)` does, require a committer for the merge even if it isn't committed.
        self.committer()
            .ok_or_else(|| message("Committer identity is not configured").validation_error())??;

        // `unclean()` in `merge-ort-wrappers.c` refuses an index that differs from `HEAD`.
        let index = self.read_index_file()?;
        let staged = self.staged_changes(head_tree, &index)?;
        if !staged.is_empty() {
            bail!(Error::LocalChanges { paths: staged });
        }
        let tree_merge = match options.tree_merge {
            Some(tree_merge) => tree_merge,
            None => self.tree_merge_options()?,
        };
        let mut merge = self.merge_commits(
            head_id,
            merged.commit,
            gix_merge::blob::builtin_driver::text::Labels {
                ancestor: None,
                current: Some("HEAD".into()),
                other: Some(name),
            },
            crate::merge::commit::Options::from(tree_merge)
                .with_allow_missing_merge_base(options.allow_unrelated_histories),
        )?;
        let applied = match self.apply_merge_result(head_tree, &mut merge.tree_merge, should_interrupt) {
            Ok(applied) => applied,
            Err(err) => match err.downcast_any_ref::<crate::merge::apply::Error>() {
                Some(crate::merge::apply::Error::Refused { refusals }) => {
                    bail!(Error::MergeRefused {
                        refusals: refusals.clone()
                    })
                }
                _ => return Err(err),
            },
        };

        let no_ff = fast_forward == FastForward::Never;
        let merge_message = match options.message {
            Some(message) => message,
            None => self.default_merge_message(&merged, into.as_bstr())?,
        };
        if !applied.conflicts && !options.no_commit && !options.squash {
            let mut reflog_message = action;
            reflog_message.extend_from_slice(b": Merge made by the 'ort' strategy.");
            let commit = self.commit_index(CommitOptions {
                message: Some(merge_message),
                cleanup: options.cleanup,
                no_verify: options.no_verify,
                sign: options.sign,
                merge: Some(MergeCommit {
                    heads: vec![merged.id],
                    no_ff,
                    reflog_message,
                    hook_env: hooks.env,
                }),
                ..Default::default()
            });
            return match commit {
                Ok(commit) => Ok(Outcome::Merged {
                    applied,
                    commit: Some(commit),
                }),
                Err(err) => match err.downcast_any_ref::<CommitRefusal>() {
                    Some(reason @ (CommitRefusal::HookFailed { .. } | CommitRefusal::EmptyMessage)) => {
                        bail!(Error::NotCommitted { reason: reason.clone() })
                    }
                    _ => Err(err),
                },
            };
        }
        if options.squash {
            // `finish()` without commit prepares the message of the squash and runs `post-merge`.
            self.write_squash_message(&git_dir, head_id, merged.commit)?;
            hooks.post_merge(true)?;
        } else {
            let mut message = merge_message;
            message.push(b'\n');
            gix_merge::state::State {
                heads: vec![merged.id],
                message: Some(message),
                no_ff,
            }
            .write(&git_dir)?;
        }
        if applied.conflicts {
            self.append_conflicts_hint(&git_dir, options.cleanup)?;
        }
        Ok(Outcome::Merged { applied, commit: None })
    }

    /// Conclude the merge in progress with its commit, as `git merge --continue` does, which runs `git commit`. That
    /// starts an editor, so the merge is committed as [`commit_index()`](Self::commit_index()) commits it with
    /// [`edited`](crate::commit::index::Options::edited) set and no other option: with the message of `MERGE_MSG`, whose
    /// comment lines, like the hint about the conflicts, go away unless `commit.cleanup` says otherwise.
    ///
    /// ### Refusals
    ///
    /// In a bare repository, [`BareRepository`](crate::merge::into_head::Error::BareRepository) is returned, and without
    /// `MERGE_HEAD`, [`NoMergeInProgress`](crate::merge::into_head::Error::NoMergeInProgress). Otherwise the refusals
    /// of [`commit_index()`](Self::commit_index()) apply.
    #[doc(alias = "git merge --continue")]
    pub fn merge_continue(&self) -> Result<crate::commit::index::Outcome> {
        let git_dir = self.merge_git_dir()?;
        if !exists(&git_dir.join("MERGE_HEAD"))? {
            bail!(Error::NoMergeInProgress);
        }
        self.commit_index(CommitOptions {
            edited: true,
            ..Default::default()
        })
    }

    /// Abort the merge in progress, as `git merge --abort` does, which runs `git reset --merge`: the index and the
    /// worktree go back to the commit of `HEAD` as [`reset()`](Self::reset()) resets them with
    /// [`Mode::Merge`](crate::reset::Mode::Merge), which keeps the local changes of the files that the merge didn't
    /// change, `ORIG_HEAD` points to `HEAD`, and the state of the merge goes away.
    ///
    /// `should_interrupt` stops the reset as long as the worktree didn't change yet.
    ///
    /// ### Refusals
    ///
    /// In a bare repository, [`BareRepository`](crate::merge::into_head::Error::BareRepository) is returned, and without
    /// `MERGE_HEAD`, [`NoMergeToAbort`](crate::merge::into_head::Error::NoMergeToAbort). Otherwise the refusals of
    /// [`reset()`](Self::reset()) apply.
    ///
    /// ### Deviation
    ///
    /// The changes that `--autostash` saved in `MERGE_AUTOSTASH` aren't applied.
    #[doc(alias = "git merge --abort")]
    pub fn merge_abort(&self, should_interrupt: &AtomicBool) -> Result<crate::reset::Outcome> {
        let git_dir = self.merge_git_dir()?;
        if !exists(&git_dir.join("MERGE_HEAD"))? {
            bail!(Error::NoMergeToAbort);
        }
        self.reset(None, crate::reset::Mode::Merge, Default::default(), should_interrupt)
    }

    /// Forget the merge in progress, as `git merge --quit` does: its state goes away as `remove_merge_branch_state()`
    /// in `branch.c` removes it, `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE` and the pseudo-reference
    /// `AUTO_MERGE`, while the index and the worktree stay as they are. Failures to remove them are ignored, as in Git.
    ///
    /// ### Refusals
    ///
    /// In a bare repository, [`BareRepository`](crate::merge::into_head::Error::BareRepository) is returned.
    #[doc(alias = "git merge --quit")]
    pub fn merge_quit(&self) -> Result<()> {
        let git_dir = self.merge_git_dir()?;
        self.remove_merge_state(&git_dir);
        Ok(())
    }

    /// Return the Git directory of this repository, in which `git merge` keeps the state of a merge, or refuse a
    /// repository without worktree, as `git merge` needs one.
    fn merge_git_dir(&self) -> Result<PathBuf> {
        if self.workdir().is_none() {
            bail!(Error::BareRepository);
        }
        Ok(self.current_dir().join(self.git_dir()))
    }

    /// Merge `commit` into the unborn branch of `HEAD`, as `cmd_merge()` does: the index and the worktree switch from
    /// the empty tree to the tree of `commit`, as `read_empty()` switches them, and the branch is created at `commit`
    /// with the reflog message `initial pull`.
    fn merge_into_unborn(&self, commit: ObjectId, should_interrupt: &AtomicBool) -> Result<Outcome> {
        let tree = self.find_commit(commit)?.tree_id()?.detach();
        let changes = match self.switch_to_tree(ObjectId::empty_tree(self.object_hash()), tree, should_interrupt)? {
            Ok(changes) => changes,
            Err(refusals) => bail!(Error::FastForwardRefused { refusals }),
        };
        self.edit_reference(
            RefEdit::update(
                gix_ref::FullName::try_from("HEAD").or_error()?,
                commit,
                PreviousValue::Any,
                "initial pull",
            )
            .with_deref(true),
        )?;
        Ok(Outcome::FastForward {
            previous: None,
            commit_id: commit,
            changes,
        })
    }

    /// Resolve `name` to the commit that `git merge` merges, as `get_merge_parent()` in `builtin/merge.c` does.
    fn merged<'a>(&self, name: &'a BStr) -> Result<Merged<'a>> {
        let not_mergeable = || Error::NotSomethingWeCanMerge { name: name.to_owned() };
        let Ok(id) = self.rev_parse_single(name) else {
            bail!(not_mergeable());
        };
        let object = id.object()?;
        let (id, kind) = (object.id, object.kind);
        let Ok(commit) = object.peel_to_commit() else {
            bail!(not_mergeable());
        };
        Ok(Merged {
            name,
            id,
            kind,
            commit: commit.id,
        })
    }

    /// Return how `git merge` fast-forwards unless it is told otherwise, as `git_merge_config()` in `builtin/merge.c`
    /// reads `merge.ff`: the last value that it knows wins, and [`FastForward::Allow`] is the default.
    fn configured_fast_forward(&self) -> FastForward {
        let mut fast_forward = FastForward::Allow;
        if let Some(sections) = self
            .config
            .resolved
            .sections_by_name_and_filter("merge", self.filter_config_section())
        {
            for section in sections.filter(|section| section.header().subsection_name().is_none()) {
                if let Some(value) = section.value_implicit(MergeKeys::FF.name())
                    && let Some(configured) =
                        MergeKeys::FF.try_into_fast_forward(value.as_ref().map(|value| value.as_bstr()))
                {
                    fast_forward = configured;
                }
            }
        }
        fast_forward
    }

    /// Return `true` if `merged` was named with an annotated tag that isn't the one that `refs/tags/<name of the tag>`
    /// holds, a tag that `merging_a_throwaway_tag()` in `builtin/merge.c` doesn't let fast-forward.
    fn is_throwaway_tag(&self, merged: &Merged<'_>) -> Result<bool> {
        if merged.kind != gix_object::Kind::Tag {
            return Ok(false);
        }
        let mut name = BString::from("refs/tags/");
        name.extend_from_slice(self.find_object(merged.id)?.try_to_tag_ref()?.name);
        let stored = self
            .try_find_reference(name.as_bstr())
            .ok()
            .flatten()
            .and_then(|reference| reference.target().try_id().map(ToOwned::to_owned));
        Ok(stored != Some(merged.id))
    }

    /// Switch the index and the worktree from the tree `from` to the tree `to` with the two-way merge of
    /// `unpack_trees()`, as `checkout_fast_forward()` in `merge.c` does, and write the index. Return what changed, or the
    /// paths that refused the switch, which then changed nothing.
    fn switch_to_tree(
        &self,
        from: ObjectId,
        to: ObjectId,
        should_interrupt: &AtomicBool,
    ) -> Result<std::result::Result<Changes, Refusals>> {
        let LockedIndex { lock, mut index } = self.lock_index()?;
        let WorktreeContext {
            worktree,
            git_dir_realpath,
            stack,
            options,
        } = self.worktree_context(&index)?;
        let objects = self
            .objects
            .clone()
            .into_arc()
            .or_raise(|| message("Could not share the object database for the checkout"))?;
        match gix_worktree_state::switch(
            &mut index,
            &worktree,
            Some(from.as_ref()),
            &to,
            objects,
            gix_worktree_state::switch::Context {
                stack,
                git_dir_realpath: &git_dir_realpath,
                current_dir: self.current_dir(),
                should_interrupt,
            },
            options,
        )? {
            gix_worktree_state::switch::Outcome::Switched(changes) => {
                self.write_locked_index(lock, &index)?;
                Ok(Ok(changes))
            }
            gix_worktree_state::switch::Outcome::Refused(refusals) => Ok(Err(refusals)),
        }
    }

    /// Return the paths at which `index` differs from `tree`, as `repo_index_has_changes()` lists them with the renames
    /// that `diff.renames` asks for, for the refusal of `unclean()` in `merge-ort-wrappers.c`.
    fn staged_changes(&self, tree: ObjectId, index: &gix_index::State) -> Result<Vec<BString>> {
        let renames = match crate::diff::utils::new_rewrites(&self.config.resolved, self.config.lenient_config)? {
            (Some(rewrites), _) => TrackRenames::Given(rewrites),
            (None, true) => TrackRenames::Disabled,
            (None, false) => TrackRenames::Given(Default::default()),
        };
        let mut paths = Vec::new();
        self.tree_index_status(&tree, index, None, renames, |change, _, _| {
            paths.push(change.location().to_owned());
            Ok(std::ops::ControlFlow::Continue(()))
        })?;
        Ok(paths)
    }

    /// Return the message that `git merge` prepares for the merge commit without `-m`: `fmt_merge_msg()` in
    /// `fmt-merge-msg.c` makes a title of how `merge_name()` in `builtin/merge.c` describes `merged`, which names the
    /// branch it merges `into` unless `merge.suppressDest` matches it, and adds the message of a merged annotated tag.
    /// The line feed at its end goes away, as `prepare_merge_message()` removes it.
    fn default_merge_message(&self, merged: &Merged<'_>, into: &BStr) -> Result<BString> {
        let (description, origin) = self.merge_name(merged);
        let mut message = BString::from("Merge ");
        message.extend_from_slice(&description);
        let suppressed = self.suppressed_destinations().iter().any(|pattern| {
            gix_glob::wildmatch(
                pattern.as_bstr(),
                into,
                gix_glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            )
        });
        if !suppressed {
            message.extend_from_slice(b" into ");
            message.extend_from_slice(into);
        }
        message.push(b'\n');
        if let Some(tag_message) = self.tag_message(origin)? {
            message.push(b'\n');
            message.extend_from_slice(&tag_message);
        }
        message.pop();
        Ok(message)
    }

    /// Return how `merge_name()` in `builtin/merge.c` describes `merged` for the title of the message of the merge,
    /// like `branch 'topic'`, and the object of the line it writes for `fmt_merge_msg()`, whose message is added to the
    /// message of the merge if it is an annotated tag.
    fn merge_name(&self, merged: &Merged<'_>) -> (BString, ObjectId) {
        let name = merged.name;
        if let Some(reference) = self.try_find_reference(name).ok().flatten() {
            let full_name = reference.name().as_bstr();
            let kind = if full_name.starts_with(b"refs/heads/") {
                Some("branch")
            } else if full_name.starts_with(b"refs/tags/") {
                Some("tag")
            } else if full_name.starts_with(b"refs/remotes/") {
                Some("remote-tracking branch")
            } else {
                None
            };
            if let Some(kind) = kind {
                let id = reference.target().try_id().map_or(merged.commit, ToOwned::to_owned);
                return (described(kind, name, ""), id);
            }
        }
        if let Some((branch, early)) = early_part(name) {
            let mut full_name = BString::from("refs/heads/");
            full_name.extend_from_slice(branch);
            if self.try_find_reference(full_name.as_bstr()).ok().flatten().is_some() {
                let suffix = if early { " (early part)" } else { "" };
                return (described("branch", branch, suffix), merged.commit);
            }
        }
        if merged.kind == gix_object::Kind::Tag {
            (described("tag", name, ""), merged.id)
        } else {
            (described("commit", name, ""), merged.commit)
        }
    }

    /// Return the message of the annotated tag `id` as `fmt_tag_signature()` in `fmt-merge-msg.c` adds it to the message
    /// of a merge: what follows the headers of the tag, without its signature, ending with a line feed, or `None` if `id`
    /// isn't an annotated tag or its message is empty.
    fn tag_message(&self, id: ObjectId) -> Result<Option<BString>> {
        let object = self.find_object(id)?;
        if object.kind != gix_object::Kind::Tag {
            return Ok(None);
        }
        let mut payload: &[u8] = &object.data;
        if let Some(signature) = object.try_to_tag_ref()?.signature
            && let Some(start) = payload.rfind(signature.as_bytes())
        {
            payload = &payload[..start];
        }
        let Some(headers_end) = payload.find(b"\n\n") else {
            return Ok(None);
        };
        let mut message = BString::from(&payload[headers_end + 2..]);
        if message.is_empty() {
            return Ok(None);
        }
        if !message.ends_with(b"\n") {
            message.push(b'\n');
        }
        Ok(Some(message))
    }

    /// Return the patterns of the branches into which the default message of a merge doesn't name the branch, as
    /// `fmt_merge_msg_config()` collects the values of `merge.suppressDest`: those after the last empty value, or `main`
    /// and `master` if there is no value.
    fn suppressed_destinations(&self) -> Vec<BString> {
        let mut patterns: Option<Vec<BString>> = None;
        if let Some(sections) = self
            .config
            .resolved
            .sections_by_name_and_filter("merge", self.filter_config_section())
        {
            for section in sections.filter(|section| section.header().subsection_name().is_none()) {
                for value in section.values(MergeKeys::SUPPRESS_DEST.name()) {
                    let patterns = patterns.get_or_insert_with(Vec::new);
                    if value.is_empty() {
                        patterns.clear();
                    } else {
                        patterns.push(value);
                    }
                }
            }
        }
        patterns.unwrap_or_else(|| vec!["main".into(), "master".into()])
    }

    /// Write `$GIT_DIR/SQUASH_MSG` in `git_dir` for the squash of `merged` into `head`, as `squash_message()` in
    /// `builtin/merge.c` writes it: `Squashed commit of the following:`, then each commit that `merged` reaches and
    /// `head` doesn't, in the order of the revision walk of Git, with its id and as `pretty_print_commit()` shows it in
    /// the `medium` format, with the abbreviated ids of the parents of a merge, the author and the date of authorship.
    fn write_squash_message(&self, git_dir: &Path, head: ObjectId, merged: ObjectId) -> Result<()> {
        let mut out = BString::from("Squashed commit of the following:\n");
        for id in self.squashed_commits(head, merged)? {
            let commit = self.find_commit(id)?;
            out.extend_from_slice(format!("\ncommit {id}\n").as_bytes());
            let parents: Vec<_> = commit.parent_ids().collect();
            if parents.len() > 1 {
                out.extend_from_slice(b"Merge:");
                for parent in parents {
                    out.extend_from_slice(format!(" {}", parent.shorten_or_id()).as_bytes());
                }
                out.push(b'\n');
            }
            let author = commit.author()?;
            out.extend_from_slice(b"Author: ");
            out.extend_from_slice(author.name);
            out.extend_from_slice(b" <");
            out.extend_from_slice(author.email);
            out.extend_from_slice(b">\nDate:   ");
            out.extend_from_slice(
                author
                    .time()?
                    .format_or_unix(gix_date::time::format::DEFAULT)
                    .as_bytes(),
            );
            out.extend_from_slice(b"\n\n");
            // `pp_remainder()` indents the lines of the message, without their trailing whitespace, and turns runs of
            // blank lines into one empty line.
            let mut gap = false;
            for line in skip_blank_lines(commit.message_raw()?).split(|byte| *byte == b'\n') {
                let line = trim_end(line);
                if line.is_empty() {
                    gap = true;
                    continue;
                }
                if gap {
                    out.push(b'\n');
                    gap = false;
                }
                out.extend_from_slice(b"    ");
                out.extend_from_slice(line);
                out.push(b'\n');
            }
            let len = trim_end(&out).len();
            out.truncate(len);
            out.push(b'\n');
        }
        let path = git_dir.join("SQUASH_MSG");
        std::fs::write(&path, &out).or_raise(|| message!("Could not write {}", path.display()))?;
        Ok(())
    }

    /// Return the commits that `merged` reaches and `head` doesn't, in the order in which `prepare_revision_walk()` and
    /// `limit_list()` in `revision.c` list them for `squash_message()`: by the time of their committer, newest first,
    /// and in the order in which they were queued if the time is the same.
    fn squashed_commits(&self, head: ObjectId, merged: ObjectId) -> Result<Vec<ObjectId>> {
        let mut walk = SquashWalk {
            repo: self,
            commits: HashMap::new(),
            queue: BinaryHeap::new(),
            queued: 0,
        };
        // `handle_commit()` of `HEAD`, which `squash_message()` marks as uninteresting, and then of the merged commit.
        walk.add_flags(head, UNINTERESTING);
        walk.parse(head)?;
        walk.mark_parents_uninteresting(head);
        walk.parse(merged)?;
        let mut tips = Vec::new();
        for id in [head, merged] {
            if walk.flags(id) & SEEN == 0 {
                walk.add_flags(id, SEEN);
                tips.push(id);
            }
        }
        // `commit_list_sort_by_date()` keeps the order of commits with the same time.
        tips.sort_by_key(|id| Reverse(walk.time(*id)));
        for id in tips {
            walk.queue(id);
        }

        let mut slop = SLOP;
        let mut date = i64::MAX;
        let mut listed = Vec::new();
        while let Some((time, _, id)) = walk.queue.pop() {
            walk.process_parents(id)?;
            if walk.flags(id) & UNINTERESTING != 0 {
                walk.mark_parents_uninteresting(id);
                slop = walk.still_interesting(date, slop);
                if slop == 0 {
                    break;
                }
                continue;
            }
            date = time;
            listed.push(id);
        }
        // `get_revision()` skips the commits that became uninteresting after they were listed.
        listed.retain(|id| walk.flags(*id) & UNINTERESTING == 0);
        Ok(listed)
    }

    /// Append the hint about the conflicts of the index to `MERGE_MSG` in `git_dir`, as `suggest_conflicts()` in
    /// `builtin/merge.c` does: with the comment prefix of `core.commentChar` or `core.commentString`, which is `#` for
    /// `auto`, and with the scissors line if `cleanup`, or `commit.cleanup` without it, is `scissors`.
    fn append_conflicts_hint(&self, git_dir: &Path, cleanup: Option<Cleanup>) -> Result<()> {
        let cleanup = match cleanup {
            Some(cleanup) => cleanup,
            None => self.commit_cleanup()?,
        };
        let comment_prefix = match self.comment_prefix()? {
            CommentPrefix::Fixed(prefix) => prefix,
            CommentPrefix::Auto => "#".into(),
        };
        let index = self.read_index_file()?;
        let hint = gix_merge::state::conflicts_hint(&index, comment_prefix.as_bstr(), cleanup == Cleanup::Scissors);
        let path = git_dir.join("MERGE_MSG");
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&hint))
            .or_raise(|| message!("Could not append to {}", path.display()))?;
        Ok(())
    }
}

/// The commit that `git merge` merges, as `get_merge_parent()` resolves its name.
struct Merged<'a> {
    /// The name of the commit as it was given.
    name: &'a BStr,
    /// The object that the name names: the commit, or an annotated tag of it.
    id: ObjectId,
    /// The kind of the object that the name names.
    kind: gix_object::Kind,
    /// The merged commit.
    commit: ObjectId,
}

/// The hooks of a merge, with the variables that `cmd_merge()` exports for them: `GIT_REFLOG_ACTION`, and
/// `GITHEAD_<id>` with the name of the merged commit.
struct MergeHooks {
    /// The hooks of the repository.
    #[cfg(feature = "hooks")]
    hooks: gix_hook::Hooks,
    /// The variables of the hooks, as names with their values.
    env: Vec<(OsString, OsString)>,
}

impl MergeHooks {
    /// Run `post-merge` with `1` after a squash and `0` otherwise, as `finish()` runs it, ignoring how it exits.
    fn post_merge(&self, squash: bool) -> Result<()> {
        #[cfg(feature = "hooks")]
        {
            let mut invocation = gix_hook::Invocation::post_merge(squash);
            invocation
                .env
                .extend(self.env.iter().map(|(name, value)| (name.clone(), Some(value.clone()))));
            self.hooks.run(&invocation)?;
        }
        #[cfg(not(feature = "hooks"))]
        let _ = squash;
        Ok(())
    }
}

/// A commit of the revision walk of a squash, as far as the walk knows it.
#[derive(Default)]
struct WalkCommit {
    /// The parents, once the commit was parsed.
    parents: Option<Vec<ObjectId>>,
    /// The time of the committer in seconds, once the commit was parsed.
    time: i64,
    /// The flags of the walk: [`SEEN`], [`ADDED`] and [`UNINTERESTING`].
    flags: u8,
}

/// The revision walk that lists the commits of a squash, as `prepare_revision_walk()` and `limit_list()` in `revision.c`
/// walk for `squash_message()`, without paths and without any order but that of the time of the committers.
struct SquashWalk<'repo> {
    /// The repository of the commits.
    repo: &'repo Repository,
    /// The commits that the walk knows.
    commits: HashMap<ObjectId, WalkCommit>,
    /// The queued commits by the time of their committer, newest first, then in the order in which they were queued, as
    /// the `prio_queue` of `limit_list()` orders them.
    queue: BinaryHeap<(i64, Reverse<usize>, ObjectId)>,
    /// How many commits were queued.
    queued: usize,
}

impl SquashWalk<'_> {
    /// Return the flags of `id`.
    fn flags(&self, id: ObjectId) -> u8 {
        self.commits.get(&id).map_or(0, |commit| commit.flags)
    }

    /// Return the time of the committer of `id`, which is known once it was parsed.
    fn time(&self, id: ObjectId) -> i64 {
        self.commits.get(&id).map_or(0, |commit| commit.time)
    }

    /// Return the parents of `id`, which are known once it was parsed.
    fn parents(&self, id: ObjectId) -> Vec<ObjectId> {
        self.commits
            .get(&id)
            .and_then(|commit| commit.parents.clone())
            .unwrap_or_default()
    }

    /// Add `flags` to those of `id`.
    fn add_flags(&mut self, id: ObjectId, flags: u8) {
        self.commits.entry(id).or_default().flags |= flags;
    }

    /// Read the parents and the time of `id` unless they are known already, as `repo_parse_commit()` does.
    fn parse(&mut self, id: ObjectId) -> Result<()> {
        let commit = self.commits.entry(id).or_default();
        if commit.parents.is_none() {
            let parsed = self.repo.find_commit(id)?;
            commit.parents = Some(parsed.parent_ids().map(crate::Id::detach).collect());
            commit.time = parsed.time()?.seconds;
        }
        Ok(())
    }

    /// Queue `id`, which was parsed, as `prio_queue_put()` does.
    fn queue(&mut self, id: ObjectId) {
        self.queue.push((self.time(id), Reverse(self.queued), id));
        self.queued += 1;
    }

    /// Mark the known ancestors of `id` as uninteresting, as `mark_parents_uninteresting()` does: the parents of `id`,
    /// and the parents of those that were parsed, unless they were uninteresting already.
    fn mark_parents_uninteresting(&mut self, id: ObjectId) {
        let mut pending = self.parents(id);
        while let Some(id) = pending.pop() {
            let commit = self.commits.entry(id).or_default();
            if commit.flags & UNINTERESTING != 0 {
                continue;
            }
            commit.flags |= UNINTERESTING;
            if let Some(parents) = &commit.parents {
                pending.extend(parents.iter().copied());
            }
        }
    }

    /// Look at the parents of `id`, as `process_parents()` does: parse them, queue those that weren't seen, and if `id`
    /// is uninteresting, mark them and their known ancestors as uninteresting as well.
    fn process_parents(&mut self, id: ObjectId) -> Result<()> {
        let flags = self.flags(id);
        if flags & ADDED != 0 {
            return Ok(());
        }
        self.add_flags(id, ADDED);
        for parent in self.parents(id) {
            if flags & UNINTERESTING != 0 {
                self.add_flags(parent, UNINTERESTING);
                self.parse(parent)?;
                self.mark_parents_uninteresting(parent);
            } else {
                self.parse(parent)?;
            }
            if self.flags(parent) & SEEN == 0 {
                self.add_flags(parent, SEEN);
                self.queue(parent);
            }
        }
        Ok(())
    }

    /// Return how many more commits to take from the queue after an uninteresting one, as `still_interesting()` does:
    /// none if the queue is empty, [`SLOP`] if its newest commit isn't older than `date`, the time of the last listed
    /// commit, or if it holds an interesting commit, and one less than `slop` otherwise.
    fn still_interesting(&self, date: i64, slop: usize) -> usize {
        let Some((time, _, _)) = self.queue.peek() else {
            return 0;
        };
        if date <= *time || self.queue.iter().any(|(_, _, id)| self.flags(*id) & UNINTERESTING == 0) {
            SLOP
        } else {
            slop - 1
        }
    }
}

/// Return `<kind> '<name>'<suffix>`, as `merge_name()` in `builtin/merge.c` describes a merged commit.
fn described(kind: &str, name: &BStr, suffix: &str) -> BString {
    let mut out = BString::from(kind);
    out.extend_from_slice(b" '");
    out.extend_from_slice(name);
    out.push(b'\'');
    out.extend_from_slice(suffix.as_bytes());
    out
}

/// Return the branch of `name` if `name` has the form `<branch>^...` or `<branch>~<number>`, which `merge_name()` in
/// `builtin/merge.c` checks for, and whether it names an earlier commit than the branch: always with `^`, and with `~`
/// unless the number is zero.
fn early_part(name: &BStr) -> Option<(&BStr, bool)> {
    let carets = name.iter().rev().take_while(|byte| **byte == b'^').count();
    if carets > 0 {
        return Some((name[..name.len() - carets].as_bstr(), true));
    }
    let tilde = name.rfind_byte(b'~')?;
    let number = &name[tilde + 1..];
    if !number.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let early = number.is_empty() || number.iter().any(|digit| *digit != b'0');
    Some((name[..tilde].as_bstr(), early))
}

/// Return `message` without its leading lines that have nothing but whitespace, as `skip_blank_lines()` in `pretty.c`
/// skips them.
fn skip_blank_lines(message: &BStr) -> &BStr {
    let mut rest = message;
    while let Some(line) = rest.lines_with_terminator().next() {
        if !trim_end(line).is_empty() {
            break;
        }
        rest = rest[line.len()..].as_bstr();
    }
    rest
}

/// Return `bytes` without the whitespace at its end, as `strbuf_rtrim()` and `is_blank_line()` remove it.
fn trim_end(bytes: &[u8]) -> &[u8] {
    let len = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .map_or(0, |last| last + 1);
    &bytes[..len]
}

/// Return `value` as the value of a variable of the environment.
fn os_string(value: &BStr) -> Result<OsString> {
    Ok(gix_path::from_bstr(value)?.into_owned().into_os_string())
}

/// Return `true` if the file at `path` exists.
fn exists(path: &Path) -> Result<bool> {
    path.try_exists()
        .or_raise(|| message!("Could not check whether {} exists", path.display()))
}
