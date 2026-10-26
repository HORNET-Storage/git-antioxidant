//! The first pass: compare the entries of the index with their files, and walk the worktree for untracked files.

use std::{
    ops::ControlFlow,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::bstr::{BStr, BString};
use gix_dir::entry::Kind;
use gix_error::{Result, ResultExt, message};
use gix_hash::ObjectId;
use gix_index::{AccelerateLookup, Entry, State, entry::Mode};
use gix_status::index_as_worktree::{
    self, Change, EntryStatus, VisitEntry,
    traits::{CompareBlobs, ReadData, SubmoduleStatus},
};

use super::Options;

/// What the comparison found out about an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tracked {
    /// The file of the entry may have changed, so it is read again.
    Changed,
    /// The file of the entry is gone, or became a directory.
    Removed,
    /// The entry is the first stage of a conflict.
    Unmerged,
}

/// What the first pass found.
pub(crate) struct Found {
    /// The entries that need attention, by their position in the index, in order.
    pub(crate) tracked: Vec<(usize, Tracked)>,
    /// The untracked files, symbolic links and repositories, sorted.
    pub(crate) untracked: Vec<BString>,
    /// What the comparison did.
    pub(crate) status: index_as_worktree::Outcome,
    /// What the walk did.
    pub(crate) dirwalk: gix_dir::walk::Outcome,
}

/// What the first pass needs besides the index and the worktree.
pub(crate) struct Context<'a> {
    pub(crate) pathspec: gix_pathspec::Search,
    pub(crate) stack: &'a gix_worktree::Stack,
    pub(crate) filter: &'a gix_filter::Pipeline,
    pub(crate) git_dir_realpath: &'a Path,
    pub(crate) current_dir: &'a Path,
    pub(crate) should_interrupt: &'a AtomicBool,
}

/// Compare the entries of `index` with their files in `worktree` while walking `worktree` for untracked files, on
/// separate threads.
///
/// `lookup` is set if paths are compared case-insensitively.
pub(crate) fn changes<O, H>(
    index: &State,
    lookup: Option<&AccelerateLookup<'_>>,
    worktree: &Path,
    objects: &O,
    repository_head: &H,
    ctx: Context<'_>,
    options: Options,
) -> Result<Found>
where
    O: gix_object::Find + Send + Clone,
    H: Fn(&Path) -> Result<Option<ObjectId>> + Sync,
{
    let Context {
        pathspec,
        stack,
        filter,
        git_dir_realpath,
        current_dir,
        should_interrupt,
    } = ctx;
    let walk = {
        let mut pathspec = pathspec.clone();
        let mut excludes = stack.clone();
        let mut pathspec_attributes = pathspec
            .patterns()
            .any(|pattern| !pattern.attributes.is_empty())
            .then(|| stack.clone());
        let objects = objects.clone();
        move || -> Result<(Vec<BString>, gix_dir::walk::Outcome)> {
            let mut delegate = Untracked {
                paths: Vec::new(),
                should_interrupt,
            };
            let (outcome, _traversal_root) = gix_dir::walk(
                worktree,
                gix_dir::walk::Context {
                    should_interrupt: Some(should_interrupt),
                    git_dir_realpath,
                    current_dir,
                    index,
                    ignore_case_index_lookup: lookup,
                    pathspec: &mut pathspec,
                    pathspec_attributes: &mut |path, case, is_dir, out| {
                        pathspec_attributes.as_mut().is_some_and(|stack| {
                            stack
                                .set_case(case)
                                .at_entry(path, Some(if is_dir { Mode::DIR } else { Mode::FILE }), &objects)
                                .is_ok_and(|platform| platform.matching_attributes(out))
                        })
                    },
                    excludes: Some(&mut excludes),
                    objects: &objects,
                    explicit_traversal_root: Some(worktree),
                },
                walk_options(options.fs),
                &mut delegate,
            )?;
            Ok((delegate.paths, outcome))
        }
    };
    let status = {
        let objects = objects.clone();
        let stack = stack.clone();
        let filter = filter.clone();
        move || -> Result<(Vec<(usize, Tracked)>, index_as_worktree::Outcome)> {
            let mut collector = Collector::default();
            let outcome = gix_status::index_as_worktree(
                index,
                worktree,
                &mut collector,
                ReadAgain,
                HeadChanged {
                    worktree,
                    repository_head,
                },
                objects,
                &mut gix_utils::progress::Discard,
                index_as_worktree::Context {
                    pathspec,
                    stack,
                    filter,
                    should_interrupt,
                },
                index_as_worktree::Options {
                    fs: options.fs,
                    thread_limit: options.thread_limit,
                    stat: options.stat,
                    fscache: options.fscache,
                },
            )?;
            Ok((collector.tracked, outcome))
        }
    };
    let (walk, status) = gix_parallel::join(walk, status);
    let (mut untracked, dirwalk) = walk.or_raise(|| message("Could not walk the worktree for untracked files"))?;
    let (mut tracked, status) = status.or_raise(|| message("Could not compare the index with the worktree"))?;
    tracked.sort_unstable_by_key(|(entry_index, _)| *entry_index);
    untracked.sort_unstable();
    untracked.dedup();
    Ok(Found {
        tracked,
        untracked,
        status,
        dirwalk,
    })
}

/// Options for a walk that finds what `git add --all` adds: untracked files, symbolic links and repositories, without
/// looking into repositories or reporting ignored and tracked paths.
fn walk_options(fs: gix_fs::Capabilities) -> gix_dir::walk::Options<'static> {
    gix_dir::walk::Options {
        precompose_unicode: fs.precompose_unicode,
        ignore_case: fs.ignore_case,
        recurse_repositories: false,
        emit_pruned: false,
        emit_ignored: None,
        for_deletion: None,
        classify_untracked_bare_repositories: false,
        emit_tracked: false,
        emit_untracked: gix_dir::walk::EmissionMode::Matching,
        emit_empty_directories: false,
        emit_collapsed: None,
        symlinks_to_directories_are_ignored_like_directories: false,
        worktree_relative_worktree_dirs: None,
    }
}

/// Collects the entries that the comparison reports.
#[derive(Default)]
struct Collector {
    tracked: Vec<(usize, Tracked)>,
}

impl<'index> VisitEntry<'index> for Collector {
    type ContentChange = ();
    type SubmoduleStatus = ();

    fn visit_entry(
        &mut self,
        _entries: &'index [Entry],
        _entry: &'index Entry,
        entry_index: usize,
        _rela_path: &'index BStr,
        status: EntryStatus<Self::ContentChange, Self::SubmoduleStatus>,
    ) {
        let tracked = match status {
            EntryStatus::Conflict { .. } => Tracked::Unmerged,
            EntryStatus::Change(Change::Removed) => Tracked::Removed,
            // An entry that only needs new stat information is read again as well, which finds it unchanged.
            EntryStatus::Change(_) | EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => Tracked::Changed,
        };
        self.tracked.push((entry_index, tracked));
    }
}

/// Leaves the comparison of content to the second pass, which reads each file whose stat information doesn't show that it
/// is unchanged and compares the id of its blob with that of its entry. That way, a file that changed is read once to be
/// added, instead of once to be compared and once more to be added.
#[derive(Clone, Copy)]
struct ReadAgain;

impl CompareBlobs for ReadAgain {
    type Output = ();

    fn compare_blobs<'a, 'b>(
        &mut self,
        _entry: &Entry,
        _worktree_blob_size: u64,
        _data: impl ReadData<'a>,
        _buf: &mut Vec<u8>,
    ) -> Result<Option<Self::Output>> {
        Ok(Some(()))
    }
}

/// Reports a submodule as changed if its `HEAD` points to another commit than its entry, and as unchanged if its `HEAD`
/// doesn't point to a commit, as Git does.
struct HeadChanged<'a, H> {
    worktree: &'a Path,
    repository_head: &'a H,
}

impl<H> Clone for HeadChanged<'_, H> {
    fn clone(&self) -> Self {
        Self {
            worktree: self.worktree,
            repository_head: self.repository_head,
        }
    }
}

impl<H> SubmoduleStatus for HeadChanged<'_, H>
where
    H: Fn(&Path) -> Result<Option<ObjectId>>,
{
    type Output = ();

    fn status(&mut self, entry: &Entry, rela_path: &BStr) -> Result<Option<Self::Output>> {
        let head = (self.repository_head)(&self.worktree.join(gix_path::from_bstr(rela_path)?))?;
        Ok(head.filter(|head| *head != entry.id).map(|_| ()))
    }
}

/// Collects the untracked files, symbolic links and repositories that the walk reports.
struct Untracked<'a> {
    paths: Vec<BString>,
    should_interrupt: &'a AtomicBool,
}

impl gix_dir::walk::Delegate for Untracked<'_> {
    fn emit(
        &mut self,
        entry: gix_dir::EntryRef<'_>,
        _collapsed_directory_status: Option<gix_dir::entry::Status>,
    ) -> gix_dir::walk::Action {
        if entry.status == gix_dir::entry::Status::Untracked
            && matches!(entry.disk_kind, Some(Kind::File | Kind::Symlink | Kind::Repository))
        {
            self.paths.push(entry.rela_path.into_owned());
        }
        if self.should_interrupt.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}
