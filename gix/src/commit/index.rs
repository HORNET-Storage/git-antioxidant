//! Commit the index as `git commit` does, with [`Repository::commit_index()`](crate::Repository::commit_index()).

use std::ffi::OsString;

use gix_error::ClassificationMarker;
use gix_hash::ObjectId;

use crate::{bstr::BString, commit::message::Cleanup};

/// Options for [`Repository::commit_index()`](crate::Repository::commit_index()), which follow those of `git commit`.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// The message of the commit, as `-m` or `-F` give it, which is cleaned as [`cleanup`](Self::cleanup) says.
    ///
    /// If `None`, the message is that of `MERGE_MSG`, preceded by `SQUASH_MSG` if both exist, or that of `SQUASH_MSG`,
    /// or else with [`amend`](Self::amend) that of the amended commit, as `git commit --no-edit` takes it. Without any
    /// of them, the message is empty.
    pub message: Option<BString>,
    /// How the message is cleaned, as `--cleanup` says, or `None` for `commit.cleanup`, which defaults to
    /// [`Cleanup::Default`]. No editor is started, so the default cleans up whitespace and keeps comment lines, as Git
    /// does for a message given with `-m`.
    pub cleanup: Option<Cleanup>,
    /// Replace the commit of `HEAD` with a new commit that has its parents, as `--amend` does.
    pub amend: bool,
    /// Allow a commit that records the tree of its first parent, as `--allow-empty` does.
    pub allow_empty: bool,
    /// Allow a message that is empty after cleaning, as `--allow-empty-message` does.
    pub allow_empty_message: bool,
    /// Skip the `pre-commit` and `commit-msg` hooks, as `--no-verify` does.
    pub no_verify: bool,
    /// The author of the commit, as `--author` and `--date` give it, or `None` for the author of the amended commit with
    /// [`amend`](Self::amend) and [`Repository::author()`](crate::Repository::author()) otherwise. With
    /// [`amend`](Self::amend), the current identity resets the author as `--reset-author` does.
    pub author: Option<gix_actor::Signature>,
    /// Whether to sign the commit, as `-S` and `--no-gpg-sign` say, or `None` for `commit.gpgSign`.
    pub sign: Option<bool>,
    /// The action in front of the message of the reflog entries of the commit, as `GIT_REFLOG_ACTION` sets it, or `None`
    /// for Git's `commit`, `commit (initial)`, `commit (amend)` or `commit (merge)`.
    pub reflog_action: Option<BString>,
    /// Clean the message as Git cleans a message that was edited in an editor, as `git commit` does when it starts one,
    /// which it does unless `-m`, `-F` or `--no-edit` give the message: [`Cleanup::Default`] then removes comment lines,
    /// and [`Cleanup::Scissors`] removes the scissors line and everything below it. No editor is started, and the hooks
    /// still learn that none is used. `git merge --continue` commits as `git commit` does with an editor.
    pub edited: bool,
    /// Commit a merge without conflicts as `git merge` commits it, as [`Merge`] describes, instead of as `git commit`
    /// does, which `None` stands for.
    pub merge: Option<Merge>,
}

/// How [`Repository::commit_index()`](crate::Repository::commit_index()) commits a merge without conflicts as `git merge`
/// commits it, with `prepare_to_commit()`, `finish_automerge()` and `finish()` in `builtin/merge.c`, when
/// [`Options::merge`] gives it.
///
/// * The parents are the commit of `HEAD` and [`heads`](Self::heads), as for a merge in progress, whose `MERGE_HEAD` isn't
///   read. `HEAD` has to point to a commit, and [`Options::amend`] is refused.
/// * The tree is that of the index, written before the hooks run, as `write_tree_trivial()` writes it.
/// * The message is that of [`Options::message`], which `$GIT_DIR/MERGE_MSG` receives without cleaning. First
///   `pre-merge-commit` runs, unless [`Options::no_verify`] is set; then `MERGE_HEAD`, `MERGE_MODE` and `MERGE_MSG` are
///   written as `write_merge_heads()` writes them; then `prepare-commit-msg` runs with `MERGE_MSG` and `merge`, and
///   `commit-msg` with `MERGE_MSG` unless [`Options::no_verify`] is set. The hooks receive `GIT_INDEX_FILE`,
///   `GIT_EDITOR=:` and [`hook_env`](Self::hook_env), but no `GIT_AUTHOR_*` variables, which `git merge` doesn't export.
/// * The message is read back from `MERGE_MSG` and cleaned as [`Options::cleanup`] or `commit.cleanup` say, with `#` as
///   comment prefix if `core.commentChar` is `auto`, as `git merge` chooses none. A message that is empty after
///   cleaning is refused unless [`Options::allow_empty_message`] is set.
/// * If a hook fails or the message is refused, the state of the merge is written as `abort_commit()` writes it, for
///   `git commit` to conclude the merge: `MERGE_HEAD`, `MERGE_MODE`, and `MERGE_MSG` with the message as given followed
///   by a line feed.
/// * Otherwise `HEAD`, or its branch, points to the commit with the reflog message
///   [`reflog_message`](Self::reflog_message), then `post-merge` runs with `0` and [`hook_env`](Self::hook_env), and
///   last, the state of the merge goes away as `remove_merge_branch_state()` removes it: `MERGE_HEAD`, `MERGE_RR`,
///   `MERGE_MSG`, `MERGE_MODE` and the pseudo-reference `AUTO_MERGE`. `COMMIT_EDITMSG` isn't written, and `post-commit`
///   doesn't run.
#[derive(Debug, Default, Clone)]
pub struct Merge {
    /// The commits that are merged into `HEAD`, as `MERGE_HEAD` lists them: an annotated tag that is merged with its own
    /// id.
    pub heads: Vec<ObjectId>,
    /// Whether `HEAD` stays a parent even if a merged commit reaches it, as `--no-ff` asks for, which `MERGE_MODE` records.
    pub no_ff: bool,
    /// The message of the reflog entries of the commit, which `git merge` writes as
    /// `<action>: Merge made by the 'ort' strategy.`.
    pub reflog_message: BString,
    /// The variables that `git merge` exports for the hooks it runs, as names with their values: `GIT_REFLOG_ACTION`, and
    /// `GITHEAD_<id>` with the name of each merged commit as it was given.
    pub hook_env: Vec<(OsString, OsString)>,
}

/// What [`Repository::commit_index()`](crate::Repository::commit_index()) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The id of the new commit.
    pub commit_id: ObjectId,
    /// The id of the tree of the commit, which is the tree of the index.
    pub tree_id: ObjectId,
    /// The parents of the commit, in order.
    pub parents: Vec<ObjectId>,
    /// The message of the commit, as it was cleaned.
    pub message: BString,
    /// The reference that points to the new commit: the branch of `HEAD`, or `HEAD` if it is detached.
    pub reference: gix_ref::FullName,
    /// The commit that `HEAD` pointed to before, or `None` if the branch of `HEAD` didn't exist yet.
    pub previous: Option<ObjectId>,
}

/// The reasons for which [`Repository::commit_index()`](crate::Repository::commit_index()) makes no commit where `git commit`
/// refuses or stops, each with Git's message.
///
/// Nothing changed then, except what the hooks that ran changed, and the index whose cache tree was written before the
/// hooks ran, as Git writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The repository has no worktree.
    BareRepository,
    /// The index has unmerged entries, which have to be resolved first.
    UnmergedEntries,
    /// [`Options::amend`] was given, but `HEAD` has no commit.
    NothingToAmend,
    /// [`Options::amend`] was given while a merge is in progress.
    AmendInMerge,
    /// A merge is in progress, as `MERGE_HEAD` exists, or [`Options::merge`] asks to commit one, but `gix` was built
    /// without its `merge` feature, which reads and writes the state of a merge.
    MergeUnsupported,
    /// `commit.gpgSign` or [`Options::sign`] asks for a signature, but `gix` was built without its `command` feature,
    /// which starts the signing program.
    SigningUnsupported,
    /// The commit records the tree of its first parent, or no entry without parent, and it neither is a merge nor amends
    /// one, which [`Options::allow_empty`] would allow.
    NothingToCommit,
    /// The message is empty after cleaning, which [`Options::allow_empty_message`] would allow.
    EmptyMessage,
    /// The hooks of an event that stops the commit if it fails, `pre-commit`, `prepare-commit-msg` or `commit-msg`,
    /// failed with this code.
    HookFailed {
        /// The event of the hooks.
        event: &'static str,
        /// The combined exit code of the hooks, as Git takes it.
        code: i32,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BareRepository => f.write_str("this operation must be run in a work tree"),
            Error::UnmergedEntries => f.write_str("Committing is not possible because you have unmerged files."),
            Error::NothingToAmend => f.write_str("You have nothing to amend."),
            Error::AmendInMerge => f.write_str("You are in the middle of a merge -- cannot amend."),
            Error::MergeUnsupported => f.write_str(
                "A merge is in progress, but committing it needs the `merge` feature of gix to read MERGE_HEAD",
            ),
            Error::SigningUnsupported => f.write_str(
                "The commit is to be signed, but signing needs the `command` feature of gix to start the signing program",
            ),
            Error::NothingToCommit => f.write_str("nothing to commit"),
            Error::EmptyMessage => f.write_str("Aborting commit due to empty commit message."),
            Error::HookFailed { event, code } => write!(f, "The {event} hook failed with exit code {code}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::BareRepository | Error::NothingToAmend | Error::NothingToCommit | Error::EmptyMessage => {
                Some(const { &ClassificationMarker::VALIDATION })
            }
            Error::UnmergedEntries | Error::AmendInMerge => Some(const { &ClassificationMarker::CONFLICT }),
            Error::MergeUnsupported | Error::SigningUnsupported => Some(const { &ClassificationMarker::UNSUPPORTED }),
            Error::HookFailed { .. } => None,
        }
    }
}
