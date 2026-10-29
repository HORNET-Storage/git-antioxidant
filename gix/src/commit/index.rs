//! Commit the index as `git commit` does, with [`Repository::commit_index()`](crate::Repository::commit_index()).

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
    /// A merge is in progress, as `MERGE_HEAD` exists, but `gix` was built without its `merge` feature, which reads it.
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
