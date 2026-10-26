//! Add the changes of a worktree to its index, as `git add --all` does.
//!
//! [`add_all()`] makes the entries of an index match the files of its worktree. Files that changed are added again,
//! files that were deleted are removed, and files that aren't tracked yet are added unless they are ignored. A directory
//! that is a repository, like the worktree of a submodule, is added as the commit its `HEAD` points to.
//!
//! This happens in two passes. The first compares the entries of the index with their files while it walks the
//! worktree for files that aren't tracked yet. The second reads the files that changed or are new, converts them as
//! `.gitattributes` and the configuration ask for, and writes their blobs. Both passes use multiple threads, and the index
//! only changes once both of them succeeded, so an error or an interruption leaves it as it was.
//!
//! The timestamp of the index becomes the time at which [`add_all()`] began. Entries whose files were modified at or
//! after that time record a size of 0, and so do the entries outside of the pathspec whose files may have changed since
//! the index was last written, as these aren't read. That way the next comparison reads these files instead of trusting
//! their stat information.
//!
//! ### Deviation
//!
//! * Pathspecs aren't checked: one that matches no path, or names a path in a submodule or beyond a symbolic link,
//!   selects nothing, where Git fails without changing the index.
//! * Ignored files that a pathspec names are left out silently, where Git also leaves them out but reports them and
//!   exits with an error after updating the index.
//! * Sparse indices and entries that are excluded from a sparse checkout aren't supported.
//! * Entries whose files only changed their stat information keep the cache tree valid along their paths, where Git
//!   invalidates it.
//! * Entries record a size of 0 whenever their files may have changed after they were read or weren't read at all, as
//!   described above, where Git reads racily clean files when it writes the index and only marks those that changed.
//! * On Windows, a pathspec that doesn't match a directory that became a symbolic link can leave the entries behind the
//!   link in place, where Git removes them.
//! * Files are read into memory in full before their blobs are written.
//! * Warnings about line endings that a checkout would change are traced instead of printed if the filter pipeline is
//!   configured to warn about them.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use filetime::FileTime;
use gix_error::{Result, bail};
use gix_hash::ObjectId;
use gix_index::{State, entry::Flags};

mod edit;
mod find;
mod read;

/// What [`add_all()`] needs to read the worktree.
pub struct Context<'a> {
    /// The paths to add. An empty pathspec matches all paths, as `git add --all` without paths does.
    pub pathspec: gix_pathspec::Search,
    /// A stack in the [`AttributesAndIgnoreStack`](gix_worktree::stack::State::AttributesAndIgnoreStack) state, as
    /// [`State::for_add()`](gix_worktree::stack::State::for_add()) creates it, for the attributes that `filter` needs
    /// and the exclude patterns that decide which files are ignored.
    pub stack: gix_worktree::Stack,
    /// The filters that convert files to what Git stores.
    pub filter: gix_filter::Pipeline,
    /// The `git_dir` of the repository after [`gix_path::realpath()`], to tell it apart from the repositories nested in
    /// its worktree.
    pub git_dir_realpath: &'a Path,
    /// The current working directory, as `gix_fs::current_dir()` returns it.
    pub current_dir: &'a Path,
    /// A flag that stops the operation as soon as possible once it is set, leaving the index unchanged.
    pub should_interrupt: &'a AtomicBool,
}

/// Options for [`add_all()`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The capabilities of the file system that the worktree is on, which decide the modes of files and how paths are
    /// compared.
    pub fs: gix_fs::Capabilities,
    /// How the stat information of entries is compared with that of their files.
    pub stat: gix_index::entry::stat::Options,
    /// The number of threads to use at most, or `None` to use one per logical core.
    pub thread_limit: Option<usize>,
    /// Whether to read the metadata of the files of a directory all at once while comparing the index with the worktree,
    /// which makes the comparison faster on Windows and has no effect elsewhere.
    pub fscache: bool,
}

/// What [`add_all()`] changed in the index.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The number of paths that were added.
    pub added: usize,
    /// The number of paths whose entries changed, in content, mode or type, or whose conflict was resolved.
    pub modified: usize,
    /// The number of paths that were removed.
    pub removed: usize,
    /// The number of entries whose files didn't change, but whose stat information was updated.
    pub refreshed: usize,
    /// The number of blobs that were written because the object database didn't have them yet.
    pub written_blobs: usize,
    /// What the comparison of the index with the worktree did.
    pub status: gix_status::index_as_worktree::Outcome,
    /// What the walk through the worktree did.
    pub dirwalk: gix_dir::walk::Outcome,
}

/// Make the entries of `index` match the files in `worktree`, its root, as `git add --all` does, and return what changed.
///
/// Within the paths that the pathspec of `ctx` matches,
///
/// * entries whose files changed are replaced by entries for the files as they are now, and entries whose files were
///   deleted are removed,
/// * files that aren't tracked and aren't ignored are added, removing the entries that are in their way, such as a file
///   at the path of a directory that now contains a new file,
/// * conflicts are resolved with the file in the worktree, or removed if it was deleted.
///
/// Blobs are written to `objects` unless it has them already. A directory that is a repository, such as the worktree of a
/// submodule, is added as the commit that `repository_head` returns for its absolute path. `repository_head` returns
/// `None` if the directory isn't a repository or its `HEAD` doesn't point to a commit, which leaves the entry of a
/// submodule as it is and removes that of a file that became a directory, as Git does.
///
/// Entries that are assumed to be unchanged are left alone. The timestamp of `index` becomes the time at which this
/// function began, and its cache tree is invalidated along the paths whose entries changed in content, mode or type, or
/// were added or removed.
///
/// ### Errors
///
/// `index` is left unchanged if an error occurs or the operation is interrupted. Indices that are sparse or have entries
/// that are excluded from a sparse checkout are refused, and so is a stack that doesn't provide both attributes and
/// exclude patterns. Like Git, this fails if a path is neither a regular file, a symbolic link nor a directory, and if an
/// untracked repository or a conflicting submodule has no commit checked out.
pub fn add_all<O, H>(
    index: &mut State,
    worktree: &Path,
    objects: O,
    repository_head: H,
    ctx: Context<'_>,
    options: Options,
) -> Result<Outcome>
where
    O: gix_object::Find + gix_object::Exists + gix_object::Write + Send + Clone,
    H: Fn(&Path) -> Result<Option<ObjectId>> + Sync,
{
    if index.is_sparse()
        || index
            .entries()
            .iter()
            .any(|entry| entry.flags.contains(Flags::SKIP_WORKTREE))
    {
        bail!("Indices with entries that are excluded from a sparse checkout can't be added to".unsupported());
    }
    if !matches!(
        ctx.stack.state(),
        gix_worktree::stack::State::AttributesAndIgnoreStack { .. }
    ) {
        bail!("The stack has to provide both attributes and exclude patterns".validation());
    }
    let _span = gix_trace::coarse!("gix::add::plumbing::add_all()", entries = index.entries().len());
    let start = FileTime::now();
    let Context {
        pathspec,
        stack,
        filter,
        git_dir_realpath,
        current_dir,
        should_interrupt,
    } = ctx;

    let (edits, outcome) = {
        let index = &*index;
        let lookup = options.fs.ignore_case.then(|| index.prepare_icase_backing());
        let find::Found {
            tracked,
            untracked,
            status,
            dirwalk,
        } = find::changes(
            index,
            lookup.as_ref(),
            worktree,
            &objects,
            &repository_head,
            find::Context {
                pathspec,
                stack: &stack,
                filter: &filter,
                git_dir_realpath,
                current_dir,
                should_interrupt,
            },
            options,
        )?;
        if should_interrupt.load(Ordering::Relaxed) {
            bail!("Adding the changes of the worktree was interrupted".cancelled());
        }
        let work = edit::plan(index, lookup.as_ref(), &tracked, untracked);
        let (updates, written_blobs) = read::all(
            read::Shared {
                index,
                worktree,
                objects,
                repository_head: &repository_head,
                stack,
                filter,
                fs: options.fs,
            },
            &work,
            options.thread_limit,
            should_interrupt,
        )?;
        let edits = edit::Edits::new(index, work, updates, start, options.stat);
        let outcome = Outcome {
            written_blobs,
            status,
            dirwalk,
            ..edits.count(index)
        };
        (edits, outcome)
    };
    edits.apply(index, start);
    Ok(outcome)
}
