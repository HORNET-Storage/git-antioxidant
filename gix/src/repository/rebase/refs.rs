//! Moving `HEAD` and the references of a rebase with the reflog entries of Git, and checking out and resetting the index
//! and the worktree, as `reset_head()` in `reset.c` does for `git rebase`.

use std::io::Write;

use gix_hash::ObjectId;
use gix_ref::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};

use super::Sequencer;
use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    commit::message::normalize_reflog_message,
    error::{ErrorExt, ResultExt, bail, message},
    repository::{commit::LockedIndex, reset::WorktreeContext},
};

impl Sequencer<'_> {
    /// Apply `edits` in their order, as the sequencer updates one reference after the other.
    pub(super) fn apply_edits(&self, edits: impl IntoIterator<Item = RefEdit>) -> Result<()> {
        apply_edits(self.repo, edits)
    }

    /// Check out `onto` with a detached `HEAD` as `checkout_onto()` does with `reset_working_tree()`: the index and the
    /// worktree switch from `HEAD` to it, `ORIG_HEAD` points to `orig_head`, `HEAD` to `onto` with the reflog message
    /// `<action> (start): checkout <onto_name>`, and `post-checkout` runs, whose failure Git ignores. If the checkout
    /// fails, the state of the rebase is removed, as Git removes it.
    pub(super) fn checkout_onto(&self, onto_name: &BStr, onto: ObjectId, orig_head: ObjectId) -> Result<()> {
        #[cfg(feature = "hooks")]
        let hooks = self.repo.hooks()?;
        let head = self.head_commit()?;
        let checkout = || -> Result<()> {
            if let Some(refused) = self.switch_worktree(head, onto)? {
                bail!(message!("{refused}").validation());
            }
            self.apply_edits([gix_sequencer::refs::orig_head(orig_head, self.reflog_action.as_bstr())])?;
            let mut text = BString::from("checkout ");
            text.extend_from_slice(onto_name);
            detach_head(self.repo, onto, self.reflog_message("start", Some(&text)).as_bstr())
        };
        if let Err(err) = checkout() {
            let edits = self.dir.remove()?;
            self.apply_edits(edits)?;
            return Err(err.and_raise(message("could not detach HEAD")));
        }
        #[cfg(feature = "hooks")]
        hooks.post_checkout(
            &head.unwrap_or_else(|| self.repo.object_hash().null()),
            &onto,
            gix_hook::invocation::Checkout::Branch,
        )?;
        Ok(())
    }

    /// Switch the index and the worktree from the tree of the commit `from`, or from nothing, to the tree of the commit
    /// `to`, as the two-way merge of `unpack_trees()` does for `reset_head()` and `checkout_fast_forward()`, or return
    /// Git's message if that would lose the changes and untracked files in the way, which leaves everything as it is.
    pub(super) fn switch_worktree(&self, from: Option<ObjectId>, to: ObjectId) -> Result<Option<String>> {
        let from = from.map(|commit| self.tree_of(commit)).transpose()?;
        let to = self.tree_of(to)?;
        let LockedIndex { lock, mut index } = self.repo.lock_index()?;
        let WorktreeContext {
            worktree,
            git_dir_realpath,
            stack,
            options,
        } = self.repo.worktree_context(&index)?;
        let objects = self
            .repo
            .objects
            .clone()
            .into_arc()
            .or_raise(|| message("Could not share the object database for the checkout"))?;
        match gix_worktree_state::switch(
            &mut index,
            &worktree,
            from.as_deref(),
            &to,
            objects,
            gix_worktree_state::switch::Context {
                stack,
                git_dir_realpath: &git_dir_realpath,
                current_dir: self.repo.current_dir(),
                should_interrupt: self.should_interrupt,
            },
            options,
        )? {
            gix_worktree_state::switch::Outcome::Switched(_) => {}
            gix_worktree_state::switch::Outcome::Refused(refusals) => {
                return Ok(Some(refused(&refusals, "checkout")));
            }
        }
        self.repo.write_locked_index(lock, &index)?;
        Ok(None)
    }

    /// Reset the index and the worktree to the tree of `commit` as `reset_head()` does with `RESET_HEAD_HARD`, without
    /// changing references.
    pub(super) fn reset_hard(&self, commit: ObjectId) -> Result<()> {
        if let Some(refused) = self.reset_worktree(commit, gix_worktree_state::reset::Mode::Hard)? {
            bail!(message!("{refused}").validation());
        }
        Ok(())
    }

    /// Reset the index and the worktree to the tree of `commit` in `mode`, as `unpack_trees()` does for the sequencer,
    /// without changing references, or return Git's message if that would lose changes, which leaves everything as it
    /// is.
    pub(super) fn reset_worktree(
        &self,
        commit: ObjectId,
        mode: gix_worktree_state::reset::Mode,
    ) -> Result<Option<String>> {
        let tree = self.tree_of(commit)?;
        let LockedIndex { lock, mut index } = self.repo.lock_index()?;
        let WorktreeContext {
            worktree,
            git_dir_realpath,
            stack,
            options,
        } = self.repo.worktree_context(&index)?;
        let objects = self
            .repo
            .objects
            .clone()
            .into_arc()
            .or_raise(|| message("Could not share the object database for the reset"))?;
        match gix_worktree_state::reset(
            &mut index,
            &worktree,
            &tree,
            mode,
            objects,
            gix_worktree_state::switch::Context {
                stack,
                git_dir_realpath: &git_dir_realpath,
                current_dir: self.repo.current_dir(),
                should_interrupt: self.should_interrupt,
            },
            options,
        )? {
            gix_worktree_state::reset::Outcome::Reset(_) => {}
            gix_worktree_state::reset::Outcome::Refused(refusals) => {
                return Ok(Some(refused(&refusals, "reset")));
            }
        }
        self.repo.write_locked_index(lock, &index)?;
        Ok(None)
    }

    /// Return the tree of the commit `id`.
    pub(super) fn tree_of(&self, id: ObjectId) -> Result<ObjectId> {
        Ok(self.repo.find_commit(id)?.tree_id()?.detach())
    }
}

/// Apply `edits` of `repo` in their order, as the sequencer updates one reference after the other, with the reflog
/// entries of `HEAD` that Git writes when `HEAD` is detached or points to a branch again, which `gix-ref` doesn't write.
pub(super) fn apply_edits(repo: &Repository, edits: impl IntoIterator<Item = RefEdit>) -> Result<()> {
    for edit in edits {
        let is_head = edit.name.as_bstr() == "HEAD";
        match &edit.change {
            Change::Update {
                new: Target::Symbolic(branch),
                log,
                ..
            } if is_head => attach_head(repo, branch, log.message.as_bstr())?,
            Change::Update {
                new: Target::Object(id),
                log,
                ..
            } if is_head && !edit.deref => detach_head(repo, *id, log.message.as_bstr())?,
            _ => {
                repo.edit_references(Some(edit))?;
            }
        }
    }
    Ok(())
}

/// Point `HEAD` of `repo` to `id` without following it, with the reflog message `log_message`, as `refs_update_ref()`
/// does with `REF_NO_DEREF`: Git logs the previous commit of `HEAD`, and doesn't touch a detached `HEAD` that points to
/// `id` already.
pub(super) fn detach_head(repo: &Repository, id: ObjectId, log_message: &BStr) -> Result<()> {
    let head = repo.head()?;
    let previous = repo.head_id().ok().map(crate::Id::detach);
    if head.is_detached() && previous == Some(id) {
        return Ok(());
    }
    write_head_without_log(repo, Target::Object(id), log_message)?;
    log_head(
        repo,
        previous.unwrap_or_else(|| repo.object_hash().null()),
        id,
        log_message,
    )
}

/// Point `HEAD` of `repo` to the branch `name` with the reflog message `log_message`, as `refs_update_symref()` does,
/// which logs the commit of `HEAD` before and that of the branch.
pub(super) fn attach_head(repo: &Repository, name: &FullName, log_message: &BStr) -> Result<()> {
    let previous = repo.head_id().ok().map(crate::Id::detach);
    write_head_without_log(repo, Target::Symbolic(name.clone()), log_message)?;
    let new = repo
        .try_find_reference(name.as_ref())?
        .map(|mut reference| reference.peel_to_id())
        .transpose()?
        .map(crate::Id::detach);
    let null = repo.object_hash().null();
    log_head(repo, previous.unwrap_or(null), new.unwrap_or(null), log_message)
}

/// Point `HEAD` of `repo` to `target` without following it and without reflog entry.
fn write_head_without_log(repo: &Repository, target: Target, log_message: &BStr) -> Result<()> {
    let mut refs = repo.refs.clone();
    refs.write_reflog = gix_ref::store::WriteReflog::Disable;
    let (file_lock_fail, packed_refs_lock_fail) = repo
        .config
        .lock_timeout()
        .or_raise(|| message("Could not interpret the lock timeouts of references"))?;
    refs.transaction()
        .prepare(
            Some(RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: log_message.to_owned(),
                    },
                    expected: PreviousValue::Any,
                    new: target,
                },
                name: FullName::try_from("HEAD").or_error()?,
                deref: false,
            }),
            file_lock_fail,
            packed_refs_lock_fail,
        )?
        .commit(None)?;
    Ok(())
}

/// Append the entry from `previous` to `new` with `log_message` to the reflog of `HEAD`, as the references of Git write
/// it, unless reflogs are disabled.
fn log_head(repo: &Repository, previous: ObjectId, new: ObjectId, log_message: &BStr) -> Result<()> {
    if repo.refs.write_reflog == gix_ref::store::WriteReflog::Disable {
        return Ok(());
    }
    let committer = repo
        .committer()
        .ok_or_else(|| message("Committer identity is not configured").validation_error())??;
    let mut line = format!("{previous} {new} ").into_bytes();
    committer.trim().write_to(&mut line).or_error()?;
    let log_message = normalize_reflog_message(log_message);
    if !log_message.is_empty() {
        line.push(b'\t');
        line.extend_from_slice(&log_message);
    }
    line.push(b'\n');
    let path = repo.current_dir().join(repo.git_dir()).join("logs").join("HEAD");
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

/// Return the message with which Git refuses a checkout or reset of `operation` that would lose the changes or untracked
/// files of the paths of `refusals`, as `setup_unpack_trees_porcelain()` words them, followed by `Aborting`.
fn refused(refusals: &gix_worktree_state::switch::Refusals, operation: &str) -> String {
    let mut out = String::new();
    let mut local_changes: Vec<&BString> = refusals
        .staged_changes
        .iter()
        .chain(&refusals.unstaged_changes)
        .collect();
    local_changes.sort();
    local_changes.dedup();
    let action = if operation == "checkout" {
        "switch branches"
    } else {
        operation
    };
    let groups: [(Vec<&BString>, String, Option<String>); 6] = [
        (
            refusals.unmerged.iter().collect(),
            "you need to resolve your current index first".into(),
            None,
        ),
        (
            local_changes,
            format!("Your local changes to the following files would be overwritten by {operation}:"),
            Some(format!("Please commit your changes or stash them before you {action}.")),
        ),
        (
            refusals.directories_with_untracked_files.iter().collect(),
            "Updating the following directories would lose untracked files in them:".into(),
            None,
        ),
        (
            refusals.current_dir.iter().collect(),
            "Refusing to remove the current working directory:".into(),
            None,
        ),
        (
            refusals.untracked_removed.iter().collect(),
            format!("The following untracked working tree files would be removed by {operation}:"),
            Some(format!("Please move or remove them before you {action}.")),
        ),
        (
            refusals.untracked_overwritten.iter().collect(),
            format!("The following untracked working tree files would be overwritten by {operation}:"),
            Some(format!("Please move or remove them before you {action}.")),
        ),
    ];
    for (paths, header, footer) in groups.iter().filter(|(paths, _, _)| !paths.is_empty()) {
        out.push_str(header);
        out.push('\n');
        for path in paths {
            out.push('\t');
            out.push_str(&path.to_str_lossy());
            out.push('\n');
        }
        if let Some(footer) = footer {
            out.push_str(footer);
            out.push('\n');
        }
    }
    out.push_str("Aborting");
    out
}
