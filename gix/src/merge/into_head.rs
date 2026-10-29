//! Merge a commit into `HEAD` as `git merge` does, with [`Repository::merge_into_head()`](crate::Repository::merge_into_head()),
//! and conclude, abort or quit a merge that stopped before its commit as `git merge --continue`, `--abort` and `--quit`
//! do, with [`Repository::merge_continue()`](crate::Repository::merge_continue()),
//! [`Repository::merge_abort()`](crate::Repository::merge_abort()) and
//! [`Repository::merge_quit()`](crate::Repository::merge_quit()).

use gix_error::ClassificationMarker;
use gix_hash::ObjectId;

use crate::{bstr::BString, commit::message::Cleanup};

pub use gix_worktree_state::switch::{Changes, Refusals};

/// Whether `git merge` moves `HEAD` to the merged commit when `HEAD` is one of its ancestors, instead of making a merge
/// commit, as `--ff`, `--no-ff` and `--ff-only` say, and `merge.ff` configures.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FastForward {
    /// `--ff`, the default: fast-forward when `HEAD` is an ancestor of the merged commit, and make a merge commit
    /// otherwise.
    #[default]
    Allow,
    /// `--no-ff`: make a merge commit even when `HEAD` could be fast-forwarded. Git also makes one for an annotated tag
    /// that isn't the tag that `refs/tags/<name of the tag>` holds, unless [`Only`](Self::Only) is asked for.
    Never,
    /// `--ff-only`: fast-forward when `HEAD` is an ancestor of the merged commit, and refuse to merge otherwise.
    Only,
}

/// Options for [`Repository::merge_into_head()`](crate::Repository::merge_into_head()), which follow those of `git merge`.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// How the merge may fast-forward, as `--ff`, `--no-ff` or `--ff-only` say, or `None` for `merge.ff`, which defaults
    /// to [`FastForward::Allow`].
    pub fast_forward: Option<FastForward>,
    /// Stop before the commit of a merge without conflicts, as `--no-commit` does, and leave the state of the merge for
    /// [`merge_continue()`](crate::Repository::merge_continue()) or `git commit` to conclude it. A fast-forward makes
    /// no commit, so it still happens, as in Git.
    pub no_commit: bool,
    /// Change the index and the worktree as the merge does, but make no commit, leave `HEAD` where it is and write no
    /// state of a merge, only `$GIT_DIR/SQUASH_MSG` for the commit that `git commit` makes then, as `--squash` does.
    pub squash: bool,
    /// The message of the merge commit, as `-m` gives it, or `None` for Git's default message, like
    /// `Merge branch 'topic'`.
    pub message: Option<BString>,
    /// How the message of the merge commit is cleaned, as `--cleanup` says, or `None` for `commit.cleanup`. No editor is
    /// started, so [`Cleanup::Default`] cleans up whitespace and keeps comment lines, as `git merge` does without editor.
    pub cleanup: Option<Cleanup>,
    /// Merge a commit that shares no history with `HEAD`, as `--allow-unrelated-histories` does.
    pub allow_unrelated_histories: bool,
    /// Skip the `pre-merge-commit` and `commit-msg` hooks, as `--no-verify` does.
    pub no_verify: bool,
    /// Whether to sign the merge commit, as `-S` and `--no-gpg-sign` say, or `None` for `commit.gpgSign`.
    pub sign: Option<bool>,
    /// The action in front of the reflog messages of the merge and in `GIT_REFLOG_ACTION` for the hooks, as
    /// `GIT_REFLOG_ACTION` sets it, or `None` for Git's `merge <name>`.
    pub reflog_action: Option<BString>,
    /// How the trees are merged, as `-X` changes it, or `None` for
    /// [`tree_merge_options()`](crate::Repository::tree_merge_options()).
    pub tree_merge: Option<crate::merge::tree::Options>,
}

/// What [`Repository::merge_into_head()`](crate::Repository::merge_into_head()) did.
#[derive(Debug)]
pub enum Outcome {
    /// `HEAD` contains the merged commit already, so nothing changed but the removal of the state of an earlier merge, as
    /// Git says `Already up to date.`.
    UpToDate,
    /// The index and the worktree switched to the tree of the merged commit, and `HEAD`, or its branch, points to the
    /// merged commit now, as `HEAD` was one of its ancestors or an unborn branch. With [`Options::squash`], `HEAD`
    /// stayed where it was.
    FastForward {
        /// The commit that `HEAD` pointed to before, or `None` if its branch was unborn.
        previous: Option<ObjectId>,
        /// The merged commit.
        commit_id: ObjectId,
        /// What changed in the index and the worktree.
        changes: Changes,
    },
    /// The merged tree was applied to the index and the worktree, and committed unless it has conflicts or
    /// [`Options::no_commit`] or [`Options::squash`] were given. With conflicts, as `applied` tells, the index holds
    /// their stages and the worktree their markers, where Git says
    /// `Automatic merge failed; fix conflicts and then commit the result.`.
    Merged {
        /// What applying the merged tree did, with the tree and whether it has conflicts.
        applied: crate::merge::apply::Outcome,
        /// The merge commit, or `None` if the merge stopped before it.
        commit: Option<crate::commit::index::Outcome>,
    },
}

/// The reasons for which `git merge` refuses or stops, which
/// [`Repository::merge_into_head()`](crate::Repository::merge_into_head()),
/// [`Repository::merge_continue()`](crate::Repository::merge_continue()),
/// [`Repository::merge_abort()`](crate::Repository::merge_abort()) and
/// [`Repository::merge_quit()`](crate::Repository::merge_quit()) report with Git's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The repository has no worktree.
    BareRepository,
    /// The index has unmerged entries, which have to be resolved first.
    UnmergedEntries,
    /// A merge is in progress, as `MERGE_HEAD` exists, which has to be concluded first.
    MergeInProgress,
    /// A cherry-pick is in progress, as `CHERRY_PICK_HEAD` exists, which has to be concluded first.
    CherryPickInProgress,
    /// [`Options::squash`] was given while the merge may not fast-forward, as `--no-ff` or `merge.ff` say.
    SquashWithNoFastForward,
    /// [`Options::squash`] was given while `HEAD` is an unborn branch.
    SquashIntoUnborn,
    /// The merge may not fast-forward, as `--no-ff` or `merge.ff` say, while `HEAD` is an unborn branch.
    NoFastForwardIntoUnborn,
    /// The name of the merged commit names no commit.
    NotSomethingWeCanMerge {
        /// The name as it was given.
        name: BString,
    },
    /// `HEAD` and the merged commit have no common ancestor, which [`Options::allow_unrelated_histories`] would allow.
    UnrelatedHistories,
    /// [`FastForward::Only`] was asked for, but `HEAD` isn't an ancestor of the merged commit.
    NotPossibleToFastForward,
    /// The index differs from the tree of `HEAD` at these paths, which a merge that isn't a fast-forward refuses before
    /// it starts.
    LocalChanges {
        /// The paths whose staged changes differ from `HEAD`, in the order of Git's message.
        paths: Vec<BString>,
    },
    /// A fast-forward, also onto an unborn branch, would lose the changes or the untracked files of these paths.
    FastForwardRefused {
        /// The refused paths in the groups of Git's messages.
        refusals: Refusals,
    },
    /// Applying the merged tree to the index and the worktree would lose the changes or the untracked files of these
    /// paths.
    MergeRefused {
        /// The refused paths in the groups of Git's messages.
        refusals: Refusals,
    },
    /// The merge was applied without conflicts, but its commit wasn't made for `reason`, a failing hook or an empty
    /// message. The state of the merge stays for `git commit` to conclude it. As in Git, the message names only an
    /// empty message as the reason, which is the source of the error.
    NotCommitted {
        /// Why the commit wasn't made.
        reason: crate::commit::index::Error,
    },
    /// [`Repository::merge_continue()`](crate::Repository::merge_continue()) found no `MERGE_HEAD`.
    NoMergeInProgress,
    /// [`Repository::merge_abort()`](crate::Repository::merge_abort()) found no `MERGE_HEAD`.
    NoMergeToAbort,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BareRepository => f.write_str("this operation must be run in a work tree"),
            Error::UnmergedEntries => f.write_str(
                "Merging is not possible because you have unmerged files.\nExiting because of an unresolved conflict.",
            ),
            Error::MergeInProgress => f.write_str(
                "You have not concluded your merge (MERGE_HEAD exists).\nPlease, commit your changes before you merge.",
            ),
            Error::CherryPickInProgress => f.write_str(
                "You have not concluded your cherry-pick (CHERRY_PICK_HEAD exists).\nPlease, commit your changes before you merge.",
            ),
            Error::SquashWithNoFastForward => f.write_str("options '--squash' and '--no-ff.' cannot be used together"),
            Error::SquashIntoUnborn => f.write_str("Squash commit into empty head not supported yet"),
            Error::NoFastForwardIntoUnborn => {
                f.write_str("Non-fast-forward commit does not make sense into an empty head")
            }
            Error::NotSomethingWeCanMerge { name } => write!(f, "merge: {name} - not something we can merge"),
            Error::UnrelatedHistories => f.write_str("refusing to merge unrelated histories"),
            Error::NotPossibleToFastForward => f.write_str("Not possible to fast-forward, aborting."),
            Error::LocalChanges { paths } => {
                f.write_str("Your local changes to the following files would be overwritten by merge:\n ")?;
                for path in paths {
                    write!(f, " {path}")?;
                }
                f.write_str("\nMerge with strategy ort failed.")
            }
            Error::FastForwardRefused { refusals } => crate::merge::apply::write_refusals(f, refusals),
            Error::MergeRefused { refusals } => {
                crate::merge::apply::write_refusals(f, refusals)?;
                f.write_str("\nMerge with strategy ort failed.")
            }
            Error::NotCommitted { reason } => {
                if matches!(reason, crate::commit::index::Error::EmptyMessage) {
                    f.write_str("Empty commit message.\n")?;
                }
                f.write_str("Not committing merge; use 'git commit' to complete the merge.")
            }
            Error::NoMergeInProgress => f.write_str("There is no merge in progress (MERGE_HEAD missing)."),
            Error::NoMergeToAbort => f.write_str("There is no merge to abort (MERGE_HEAD missing)."),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::BareRepository
            | Error::SquashWithNoFastForward
            | Error::SquashIntoUnborn
            | Error::NoFastForwardIntoUnborn
            | Error::NoMergeInProgress
            | Error::NoMergeToAbort => Some(const { &ClassificationMarker::VALIDATION }),
            Error::NotSomethingWeCanMerge { .. } => Some(const { &ClassificationMarker::NOT_FOUND }),
            Error::UnmergedEntries
            | Error::MergeInProgress
            | Error::CherryPickInProgress
            | Error::UnrelatedHistories
            | Error::NotPossibleToFastForward
            | Error::LocalChanges { .. }
            | Error::FastForwardRefused { .. }
            | Error::MergeRefused { .. } => Some(const { &ClassificationMarker::CONFLICT }),
            Error::NotCommitted { reason } => Some(reason),
        }
    }
}
