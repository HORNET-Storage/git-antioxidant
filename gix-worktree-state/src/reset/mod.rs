//! Reset an index and its worktree to a tree, as `git reset` does in each of its modes, beside
//! [`switch()`](crate::switch()).
//!
//! `git reset` changes the index and the worktree as its mode says before it moves `HEAD`, which is left to the caller
//! along with `ORIG_HEAD`, the reflog and the removal of the state of a merge:
//!
//! * `--soft` changes neither the index nor the worktree. Git refuses it in the middle of a merge, while the index has
//!   conflicts or `MERGE_HEAD` exists, saying "Cannot do a soft reset in the middle of a merge."
//! * `--mixed` makes the entries of the index those of the tree without changing the worktree, and so does
//!   `git reset <tree-ish> -- <pathspec>` for the paths of its pathspec. [`mixed()`] does both.
//! * `--hard` makes the index and the worktree hold the tree, losing local changes and the untracked files in the way.
//! * `--merge` makes the index hold the tree and checks out the files of the entries that it changes, keeping the local
//!   changes of the other files. It refuses an entry that changes if its file has local changes, and untracked files
//!   in the way of the files that it checks out.
//! * `--keep` switches the index and the worktree from the tree of `HEAD` to the tree as [`switch()`](crate::switch())
//!   does, refusing what it refuses, and then makes the index hold the tree, so that the staged changes that the switch
//!   keeps become changes of the worktree.
//!
//! [`reset()`](crate::reset()) does the last three as `reset_index()` in `builtin/reset.c` does them, with
//! `oneway_merge()` and `twoway_merge()` of `unpack-trees.c`, and [`mixed()`] does the mixed reset as
//! `read_from_tree()` does it.
//!
//! ### Refusals
//!
//! If Git refuses a reset, nothing changes and [`Outcome::Refused`] lists the paths in the groups of [`Refusals`]. Git
//! names the first of them in the order of paths with the sentence of its group, and then says
//! `Could not reset index file to revision '<rev>'.`:
//!
//! * `staged_changes`: `Entry '<path>' would be overwritten by merge. Cannot merge.`
//! * `unstaged_changes`: `Entry '<path>' not uptodate. Cannot merge.`
//! * `directories_with_untracked_files`: `Updating '<path>' would lose untracked files in it`
//! * `current_dir`: `Refusing to remove '<path>' since it is the current working directory.`
//! * `untracked_overwritten`: `Untracked working tree file '<path>' would be overwritten by merge.`
//! * `untracked_removed`: `Untracked working tree file '<path>' would be removed by merge.`
//! * `unmerged`: the conflicts that refuse a keep reset, for which Git says
//!   `Cannot do a keep reset in the middle of a merge.` without naming them. Git also refuses a keep reset while
//!   `MERGE_HEAD` exists, which the caller checks.
//!
//! A hard reset only refuses to remove the current working directory, and a mixed reset refuses nothing.
//!
//! ### Deviation
//!
//! * The [deviations of a switch](mod@crate::switch#deviation) apply to merge and keep resets as well.
//! * Merge and keep resets compare the files whose stat information differs from that of their entries with them, and
//!   only refuse those whose content, mode or kind changed. Git doesn't refresh the index before these resets, and
//!   refuses all of those files unless the index was refreshed.
//! * A hard reset refuses to remove the current working directory anywhere in a directory that a file replaces, where Git
//!   only refuses it if it is that directory, and otherwise removes it, or fails to remove it on a host that can't remove
//!   the current directory of a process.
//! * A hard reset checks out the files of the entries that stay again if their content, mode or kind changed, where Git
//!   also checks out those of which only the stat information changed.
//! * The entries of a mixed reset aren't refreshed afterwards: those that change record a size of 0, so that the next
//!   comparison reads their files, where Git refreshes the index and lists the files with unstaged changes.
//! * A mixed reset keeps the resolve-undo information, but doesn't record the stages of the conflicts that it resolves
//!   in it, where Git records them.
//! * Sparse indices and entries that are excluded from a sparse checkout aren't supported, by mixed resets neither.

use gix_hash::ObjectId;

use crate::switch::{Changes, Refusals};

/// How [`reset()`](crate::reset()) changes the index and its worktree, as the modes of `git reset` that change the
/// worktree do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `git reset --hard`: the index and the worktree hold the tree. Local changes are lost, and so are the untracked
    /// files and directories in the way of the files of the tree. The files of the entries that stay are checked out
    /// again if they changed.
    Hard,
    /// `git reset --merge`: the index holds the tree, and the files of the entries that change are checked out. An entry
    /// that changes is refused if its file has local changes, and so are untracked files in the way of the files that are
    /// checked out. The other files keep their local changes, and conflicts are resolved to the entries of the tree.
    Merge,
    /// `git reset --keep`: the index and the worktree are switched from `head_tree` to the tree, and the index holds the
    /// tree afterwards. A path that the trees change is refused if it has staged or local changes, as a switch refuses
    /// it, and so is an index with conflicts.
    Keep {
        /// The tree of `HEAD`, from which the index and the worktree were made. Git refuses a keep reset without `HEAD`.
        head_tree: ObjectId,
    },
}

/// What [`reset()`](crate::reset()) did.
#[derive(Debug)]
pub enum Outcome {
    /// The index and its worktree were reset to the tree.
    Reset(Changes),
    /// Nothing changed, as Git refuses to reset these paths.
    Refused(Refusals),
}

pub(crate) mod function;
pub use function::mixed;
