//! The references that Git writes beside the state of a rebase or of a cherry-pick or revert sequence, with the edits
//! that write and remove them as `sequencer.c` does.
//!
//! The functions here return edits for the caller to apply with a transaction of its reference store, which is how
//! `gix` writes them with its committer for the reference logs.
//!
//! - `REBASE_HEAD` names the commit at which a rebase stopped. `write_rebase_head()` writes it when a rebase stops at an
//!   `edit` or because a command couldn't apply its commit (`make_patch()`), and when a command with a commit is
//!   rescheduled. `pick_commits()` removes it before each command, and `finish_rebase()` when the rebase ends or is
//!   aborted.
//! - `ORIG_HEAD` names the commit that the rebased branch pointed to. `reset_head()` writes it with `RESET_ORIG_HEAD`
//!   when a rebase checks out its `onto` at the start (`checkout_onto()`, and `builtin/rebase.c` for the
//!   fast-forward). Git doesn't remove it.
//! - `CHERRY_PICK_HEAD` names the commit that `pick`, `reword` or `edit` merged when the merge succeeded or ended in
//!   conflicts, for `git commit` to use it (`do_pick_commit()`). Git removes it when a commit is made from it, when a
//!   commit is dropped as already upstream, when `git rebase --continue` finds nothing to commit
//!   (`commit_staged_changes()`), and with the rest of the merge state (`remove_branch_state()`) when a rebase is skipped
//!   or aborted.
//! - `REVERT_HEAD` names the commit of a `revert` whose commit wasn't made, and is removed like `CHERRY_PICK_HEAD`.
//! - `AUTO_MERGE` names the tree that a merge with conflicts wrote. The sequencer removes it before each command, after
//!   each commit that it makes, and when the rebase ends.
//! - `refs/rewritten/<label>` holds the commit of a `label`, and is removed with the state (`sequencer_remove_state()`).

use bstr::{BStr, BString, ByteSlice};
use gix_error::{Result, ResultExt, message};
use gix_hash::ObjectId;
use gix_ref::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};

/// The name of the reference to the commit at which a rebase stopped.
pub const REBASE_HEAD: &str = "REBASE_HEAD";
/// The name of the reference to the commit that the rebased branch pointed to before the rebase.
pub const ORIG_HEAD: &str = "ORIG_HEAD";
/// The name of the reference to the commit that is being cherry-picked.
pub const CHERRY_PICK_HEAD: &str = "CHERRY_PICK_HEAD";
/// The name of the reference to the commit that is being reverted.
pub const REVERT_HEAD: &str = "REVERT_HEAD";
/// The name of the reference to the tree that a merge with conflicts wrote.
pub const AUTO_MERGE: &str = "AUTO_MERGE";
/// The prefix of the references that hold the commits of `label` commands.
pub const REWRITTEN_PREFIX: &str = "refs/rewritten/";

/// Return the edit that points `REBASE_HEAD` at `commit` with the reference log message `rebase`, as
/// `write_rebase_head()` writes it.
pub fn rebase_head(commit: ObjectId) -> RefEdit {
    update_no_deref(REBASE_HEAD, commit, "rebase")
}

/// Return the edit that points `CHERRY_PICK_HEAD` at `commit` without a reference log message, as `do_pick_commit()`
/// writes it.
pub fn cherry_pick_head(commit: ObjectId) -> RefEdit {
    update_no_deref(CHERRY_PICK_HEAD, commit, "")
}

/// Return the edit that points `REVERT_HEAD` at `commit` without a reference log message, as `do_pick_commit()` writes
/// it.
pub fn revert_head(commit: ObjectId) -> RefEdit {
    update_no_deref(REVERT_HEAD, commit, "")
}

/// Return the edit that points `ORIG_HEAD` at `commit` with the reference log message `<reflog_action>: updating
/// ORIG_HEAD`, as `reset_head()` writes it for `RESET_ORIG_HEAD`. `reflog_action` is `rebase` unless
/// `GIT_REFLOG_ACTION` says otherwise.
pub fn orig_head(commit: ObjectId, reflog_action: &BStr) -> RefEdit {
    let mut message = BString::from(reflog_action.as_bytes());
    message.extend_from_slice(b": updating ORIG_HEAD");
    RefEdit::update(full_name(ORIG_HEAD), commit, PreviousValue::Any, message).with_deref(true)
}

/// Return the edit that removes the reference `name`, one of the names of this module, without following it if it is
/// symbolic, as `refs_delete_ref()` with `REF_NO_DEREF` removes it.
pub fn delete(name: &str) -> Result<RefEdit> {
    let name = FullName::try_from(name).or_raise(|| message!("invalid reference name '{name}'"))?;
    Ok(RefEdit::delete_with_log(name, PreviousValue::Any, RefLog::AndReference))
}

/// Return the edits that remove the references of a sequence that stopped for a merge, as `remove_branch_state()` and
/// `sequencer_post_commit_cleanup()` remove them when a rebase is skipped or aborted: `CHERRY_PICK_HEAD`,
/// `REVERT_HEAD` and `AUTO_MERGE`. The files `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE` and `SQUASH_MSG`, which
/// Git removes with them, are the caller's to remove.
pub fn delete_branch_state() -> Vec<RefEdit> {
    [CHERRY_PICK_HEAD, REVERT_HEAD, AUTO_MERGE]
        .into_iter()
        .map(|name| RefEdit::delete_with_log(full_name(name), PreviousValue::Any, RefLog::AndReference))
        .collect()
}

/// Return the name of the reference of the `label` command with `label`, as `do_label()` and `lookup_label()` name it.
pub fn label_name(label: &BStr) -> Result<FullName> {
    let mut name = BString::from(REWRITTEN_PREFIX);
    name.extend_from_slice(label);
    FullName::try_from(name.clone()).or_raise(|| message!("illegal label name: '{}'", label))
}

/// Return the edit of the `label` command that points the reference of `label` at `head` with the reference log message
/// `rebase (label) '<label>'`, as `do_label()` writes it, or an error for the label `#`, which `merge` couldn't name.
pub fn label(label: &BStr, head: ObjectId) -> Result<RefEdit> {
    if label == "#" {
        return Err(message!("illegal label name: '{}'", label).validation_error());
    }
    let mut log_message = BString::from("rebase (label) '");
    log_message.extend_from_slice(label);
    log_message.push(b'\'');
    Ok(RefEdit::update(label_name(label)?, head, PreviousValue::Any, log_message).with_deref(true))
}

/// Return the edit that sets the branch that a rebase updates to `after` if it still points to `before`, with the
/// reference log message `rewritten during rebase`, as `do_update_refs()` writes each reference of `update-refs`. A
/// `before` that is the null id expects the reference not to exist, as Git's transaction does.
pub fn update_ref(name: FullName, before: ObjectId, after: ObjectId) -> RefEdit {
    let expected = if before.is_null() {
        PreviousValue::MustNotExist
    } else {
        PreviousValue::MustExistAndMatch(Target::Object(before))
    };
    RefEdit::update(name, after, expected, "rewritten during rebase").with_deref(true)
}

/// Return the edit that removes the reference `name` along with its reference log, as `sequencer_remove_state()`
/// removes each reference of `refs-to-delete`.
pub fn cleanup(name: FullName) -> RefEdit {
    RefEdit::new(
        name,
        Change::Delete {
            expected: PreviousValue::Any,
            log: RefLog::AndReference,
        },
    )
    .with_deref(true)
}

/// Return an edit that sets `name` to `commit` with `message` without following it if it is symbolic, as
/// `refs_update_ref()` with `REF_NO_DEREF` sets it.
fn update_no_deref(name: &'static str, commit: ObjectId, message: &str) -> RefEdit {
    RefEdit::update_with_log(
        full_name(name),
        commit,
        PreviousValue::Any,
        LogChange {
            message: message.into(),
            ..Default::default()
        },
    )
}

/// Return the name of one of the references of this module, which are valid.
fn full_name(name: &'static str) -> FullName {
    FullName::try_from(name).expect("the names of this module are valid")
}
