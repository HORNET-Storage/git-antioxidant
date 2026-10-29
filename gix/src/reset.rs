//! Reset the index and the worktree to a tree and move `HEAD`, as `git reset` does, with
//! [`Repository::reset()`](crate::Repository::reset()) and [`Repository::reset_paths()`](crate::Repository::reset_paths()).

use gix_error::ClassificationMarker;
use gix_hash::ObjectId;

use crate::bstr::BString;

pub use gix_worktree_state::switch::{Changes, Refusals};

/// How [`Repository::reset()`](crate::Repository::reset()) changes the index and the worktree, as the modes of
/// `git reset` do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// `--soft`: only `HEAD` moves, and the index and the worktree stay as they are. Git refuses it while a merge is in
    /// progress or the index has conflicts.
    Soft,
    /// `--mixed`, Git's default: the entries of the index become those of the tree, and the worktree stays as it is.
    Mixed,
    /// `--hard`: the index and the worktree hold the tree, and local changes are lost along with the untracked files in
    /// the way of the files of the tree.
    Hard,
    /// `--merge`: the index holds the tree, and the files of the entries that change are checked out, which is refused if
    /// they have local changes. The other files keep their local changes.
    Merge,
    /// `--keep`: the index and the worktree switch from the tree of `HEAD` to the tree, which is refused for the paths
    /// that change if they have local changes, and the index holds the tree. Git refuses it while a merge is in
    /// progress or the index has conflicts.
    Keep,
}

impl Mode {
    /// Return the name of this mode in Git's messages: `soft`, `mixed`, `hard`, `merge` or `keep`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Soft => "soft",
            Mode::Mixed => "mixed",
            Mode::Hard => "hard",
            Mode::Merge => "merge",
            Mode::Keep => "keep",
        }
    }
}

/// Options for [`Repository::reset()`](crate::Repository::reset()).
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// The name of the commit as it was given, like `HEAD~1`, for the reflog message `reset: moving to <revision>` and the
    /// messages of errors, or `None` for `HEAD` if no commit is given and for the hexadecimal id of the commit otherwise.
    pub revision: Option<BString>,
    /// The action in front of the messages of the reflog entries of `HEAD` and `ORIG_HEAD`, as `GIT_REFLOG_ACTION` sets
    /// it, which makes them `<action>: updating HEAD` and `<action>: updating ORIG_HEAD`.
    pub reflog_action: Option<BString>,
}

/// What [`Repository::reset()`](crate::Repository::reset()) did.
#[derive(Debug)]
pub struct Outcome {
    /// The commit that `HEAD` points to now, or `None` if `HEAD` was reset on an unborn branch, which stays unborn.
    pub commit_id: Option<ObjectId>,
    /// The commit that `HEAD` pointed to before, which `ORIG_HEAD` holds now, or `None` if `HEAD` was unborn.
    pub previous: Option<ObjectId>,
    /// What changed in the index and the worktree, which is nothing for a soft reset.
    pub changes: Changes,
}

/// The reasons for which `git reset` refuses to reset, which [`Repository::reset()`](crate::Repository::reset()) and
/// [`Repository::reset_paths()`](crate::Repository::reset_paths()) report with Git's messages. Nothing changes then.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A soft or keep reset while `MERGE_HEAD` exists or the index has conflicts.
    InMerge {
        /// The mode of the reset.
        mode: Mode,
    },
    /// A reset of the index or the worktree in a repository without worktree.
    BareRepository {
        /// The mode of the reset.
        mode: Mode,
    },
    /// A keep reset while `HEAD` has no commit.
    InvalidHead,
    /// Resetting the index and the worktree would lose the changes or the untracked files of these paths.
    Refused {
        /// The commit as it was named.
        revision: BString,
        /// The paths that Git refuses, in the groups of its messages.
        refusals: Refusals,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InMerge { mode } => write!(f, "Cannot do a {} reset in the middle of a merge.", mode.as_str()),
            Error::BareRepository { mode: Mode::Mixed } => {
                f.write_str("mixed reset is not allowed in a bare repository")
            }
            Error::BareRepository { .. } => f.write_str("this operation must be run in a work tree"),
            Error::InvalidHead => f.write_str("You do not have a valid HEAD."),
            Error::Refused { revision, refusals } => {
                if let Some(refusal) = first_refusal(refusals) {
                    writeln!(f, "{refusal}")?;
                }
                write!(f, "Could not reset index file to revision '{revision}'.")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InMerge { .. } | Error::Refused { .. } => Some(const { &ClassificationMarker::CONFLICT }),
            Error::BareRepository { .. } | Error::InvalidHead => Some(const { &ClassificationMarker::VALIDATION }),
        }
    }
}

/// Return the message with which Git refuses the first of the paths of `refusals` in the order of paths, as
/// `unpack_trees()` stops at the first path that it refuses, or the message of a keep reset that conflicts refuse.
fn first_refusal(refusals: &Refusals) -> Option<String> {
    let groups: [(&Vec<BString>, &str, &str); 6] = [
        (
            &refusals.staged_changes,
            "Entry '",
            "' would be overwritten by merge. Cannot merge.",
        ),
        (&refusals.unstaged_changes, "Entry '", "' not uptodate. Cannot merge."),
        (
            &refusals.directories_with_untracked_files,
            "Updating '",
            "' would lose untracked files in it",
        ),
        (
            &refusals.current_dir,
            "Refusing to remove '",
            "' since it is the current working directory.",
        ),
        (
            &refusals.untracked_overwritten,
            "Untracked working tree file '",
            "' would be overwritten by merge.",
        ),
        (
            &refusals.untracked_removed,
            "Untracked working tree file '",
            "' would be removed by merge.",
        ),
    ];
    groups
        .iter()
        .flat_map(|(paths, before, after)| paths.iter().map(move |path| (path, *before, *after)))
        .min_by(|a, b| a.0.cmp(b.0))
        .map(|(path, before, after)| format!("{before}{path}{after}"))
        .or_else(|| {
            (!refusals.unmerged.is_empty()).then(|| "Cannot do a keep reset in the middle of a merge.".to_owned())
        })
}
