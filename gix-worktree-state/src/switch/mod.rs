//! Switch an index and its worktree from one tree to another, as `git read-tree -m -u <old> <new>` does when `git switch`
//! and `git checkout` change branches.
//!
//! [`switch()`] applies the difference between the old tree, usually that of `HEAD`, and the new one to the index and its
//! worktree, keeping what the index and the worktree changed in the paths that the two trees agree on. Before anything
//! changes, each path that the trees change is checked as Git's two-way merge checks it:
//!
//! * an entry that differs from both trees is a staged change, which stays if the new tree has the same entry and is
//!   refused otherwise,
//! * a file that differs from its entry is an unstaged change, which is refused,
//! * an untracked file that isn't ignored is refused if an entry of the new tree would overwrite it, or if it is at the
//!   path of an entry that only the old tree has,
//! * a directory that would be removed or replaced is refused if it contains untracked files that aren't ignored, or the
//!   current working directory.
//!
//! If a path is refused, nothing changes, and [`Outcome::Refused`] lists the refused paths in the groups of Git's messages.
//! Otherwise, the files of removed entries are deleted along with the directories that they leave empty, the files of
//! added and changed entries are checked out, and the index holds the entries of the new tree and the staged changes that
//! stayed. Like Git, the index gets a cache tree that is valid for each directory whose tree is in the object database,
//! and loses its resolve-undo information. The directories of submodules are created, but nothing is checked out in them,
//! as Git does without `--recurse-submodules`.
//!
//! ### Deviation
//!
//! * Some switches that lose data in Git are refused: removing staged entries in a directory that a file of the new tree
//!   replaces, or at the path of a directory that leads to an entry of the new tree; removing a repository or a `.git`
//!   directory in a directory that is removed or replaced, even an ignored one; removing the worktree of a submodule that a
//!   file replaces; and overwriting a file or a symbolic link at the path of a submodule.
//! * In a directory that is removed or replaced, untracked files that are neither regular files nor symbolic links, such as
//!   named pipes, are refused, where Git doesn't see them.
//! * The entries in a directory at the path of an entry that only the old tree has stay, along with their files, where Git
//!   removes both.
//! * Entries behind a symbolic link are unstaged changes, where Git compares the files that the link leads to.
//! * The current working directory is protected anywhere in a directory that would be removed or replaced, where Git only
//!   protects it if it is that directory.
//! * On a case-insensitive file system, a file in the way of an entry is only taken for the file of an entry whose path
//!   differs in case if that entry is removed, where Git takes it for that file if their stat information matches.
//! * Sparse indices and entries that are excluded from a sparse checkout aren't supported.
//! * Git compares the first checkout into a worktree without an index with `HEAD`. Here, the old tree is `None` instead,
//!   which makes every entry of the new tree a new one and doesn't refuse untracked files at the paths that only `HEAD` has.
//! * The entries that stay aren't refreshed. Those whose files may have changed since the index was written record a size
//!   of 0, so that the next comparison reads their files, where Git refreshes the index and only marks the entries whose
//!   files changed.
//! * Interruption is honored until the worktree changes, after which the switch completes.
//! * Files are overwritten in place, so their hard links see the new content and they keep their permissions other than
//!   the executable bit, where Git replaces them with new files.
//!
//! [`switch()`]: crate::switch()

use std::{path::Path, sync::atomic::AtomicBool};

use bstr::BString;

/// What [`switch()`](crate::switch()) needs to compare the worktree with the index, and so does a reset of the index and
/// the worktree.
pub struct Context<'a> {
    /// A stack in the [`AttributesAndIgnoreStack`](gix_worktree::stack::State::AttributesAndIgnoreStack) state, as
    /// [`State::for_add()`](gix_worktree::stack::State::for_add()) creates it for the index, for the attributes that the
    /// filters of the comparison need and the exclude patterns that decide which untracked files may be overwritten or
    /// removed.
    pub stack: gix_worktree::Stack,
    /// The `git_dir` of the repository after [`gix_path::realpath()`], to tell it apart from the repositories nested in
    /// its worktree.
    pub git_dir_realpath: &'a Path,
    /// The current working directory, as `gix_fs::current_dir()` returns it, which is never removed.
    pub current_dir: &'a Path,
    /// A flag that stops the switch as soon as possible once it is set, as long as the worktree didn't change yet.
    pub should_interrupt: &'a AtomicBool,
}

/// Options for [`switch()`](crate::switch()), which a reset of the index and the worktree takes as well.
#[derive(Clone)]
pub struct Options {
    /// How the files of added and changed entries are checked out. Git reads the attributes of a checkout from the
    /// `.gitattributes` files of the new index before those of the worktree, which the
    /// [`IdMappingThenWorktree`](gix_worktree::stack::state::attributes::Source::IdMappingThenWorktree) source of
    /// `attributes` does.
    ///
    /// The capabilities of the file system, the path validation, the thread limit, the stat options and the filters also
    /// apply to the comparison of the worktree with the index. `destination_is_initially_empty`, `overwrite_existing` and
    /// `keep_going` are ignored, as the files in the way are checked before anything changes, and errors of single files
    /// are reported instead of stopping the checkout.
    pub checkout: crate::checkout::Options,
    /// Whether to read the metadata of the files of a directory all at once while comparing the worktree with the index,
    /// which makes the comparison faster on Windows and has no effect elsewhere.
    pub fscache: bool,
}

/// What [`switch()`](crate::switch()) did.
#[derive(Debug)]
pub enum Outcome {
    /// The index and its worktree were switched to the new tree.
    Switched(Changes),
    /// Nothing changed, as the switch would have lost the changes or untracked files of these paths.
    Refused(Refusals),
}

/// What a switch or a reset changed in the index and its worktree.
#[derive(Debug, Default)]
pub struct Changes {
    /// The number of entries that were added.
    pub added: usize,
    /// The number of entries whose object or mode changed.
    pub modified: usize,
    /// The number of entries that were removed.
    pub removed: usize,
    /// The files and directories of removed entries that couldn't be removed and remain as untracked files, in the order of
    /// their paths. Git warns about them.
    pub unremoved: Vec<Unremoved>,
    /// What the checkout of the added and changed entries did, including the files that couldn't be written.
    pub checkout: crate::checkout::Outcome,
    /// The error that stopped the checkout before all files were written, if one did. The index holds the new entries
    /// regardless, and those whose files weren't written show up as changed in the worktree.
    pub checkout_error: Option<gix_error::Error>,
}

/// A file or directory of a removed entry that couldn't be removed.
#[derive(Debug)]
pub struct Unremoved {
    /// The path of the entry.
    pub path: BString,
    /// Why it couldn't be removed.
    pub error: gix_error::Error,
}

/// The paths that kept [`switch()`](crate::switch()) or a reset from changing anything, each list sorted, in the groups of
/// Git's messages.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refusals {
    /// Paths with conflicts, for which Git says that the current index has to be resolved first, and that each of them
    /// needs a merge.
    pub unmerged: Vec<BString>,
    /// Paths whose entries differ from both trees, or are in the way of entries of the new tree. Git lists them together
    /// with `unstaged_changes` as local changes that the checkout would overwrite.
    pub staged_changes: Vec<BString>,
    /// Paths whose files differ from their entries, or have something else in their place.
    pub unstaged_changes: Vec<BString>,
    /// Directories that would be removed or replaced although they contain untracked files, which Git lists as
    /// directories whose untracked files would be lost.
    pub directories_with_untracked_files: Vec<BString>,
    /// Directories that would be removed or replaced although they contain the current working directory.
    pub current_dir: Vec<BString>,
    /// Untracked files that entries of the new tree would overwrite.
    pub untracked_overwritten: Vec<BString>,
    /// Untracked files at the paths of entries that only the old tree has, which Git lists as untracked files that the
    /// checkout would remove.
    pub untracked_removed: Vec<BString>,
}

impl Refusals {
    /// Return `true` if no path was refused.
    pub fn is_empty(&self) -> bool {
        let Refusals {
            unmerged,
            staged_changes,
            unstaged_changes,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        } = self;
        [
            unmerged,
            staged_changes,
            unstaged_changes,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        ]
        .into_iter()
        .all(Vec::is_empty)
    }

    /// Sort each list and remove its duplicates.
    pub(crate) fn finish(mut self) -> Self {
        let Refusals {
            unmerged,
            staged_changes,
            unstaged_changes,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        } = &mut self;
        for paths in [
            unmerged,
            staged_changes,
            unstaged_changes,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        ] {
            paths.sort();
            paths.dedup();
        }
        self
    }
}

pub(crate) mod apply;
pub(crate) mod check;
pub(crate) mod function;
pub(crate) mod path;
pub(crate) mod plan;
