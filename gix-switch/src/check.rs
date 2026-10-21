//! Check the paths that the switch changes before anything changes, as Git's two-way merge does, and collect those whose
//! changes or untracked files the switch would lose.

use std::{
    fs::Metadata,
    ops::ControlFlow,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bstr::{BStr, BString, ByteSlice};
use gix_dir::entry::{Kind, Property, Status};
use gix_error::{ErrorExt, Result, ResultExt, message};
use gix_index::{
    AccelerateLookup, Entry, State,
    entry::{Flags, Mode},
};
use gix_status::index_as_worktree::{
    self, Change, EntryStatus, VisitEntry,
    traits::{FastEq, SubmoduleStatus},
};

use crate::{
    Options, Refusals, check_interrupt,
    path::{self, LeadingPath, LeadingPaths},
    plan::{Action, Plan},
};

/// What the checks need.
pub(crate) struct Checker<'a, O> {
    pub(crate) index: &'a State,
    /// The lookup of entries by paths that differ in case, if the file system ignores case.
    pub(crate) lookup: Option<&'a AccelerateLookup<'a>>,
    pub(crate) worktree: &'a Path,
    pub(crate) objects: &'a O,
    pub(crate) stack: gix_worktree::Stack,
    /// A pathspec that matches all paths.
    pub(crate) pathspec: gix_pathspec::Search,
    pub(crate) git_dir_realpath: &'a Path,
    pub(crate) current_dir: &'a Path,
    /// The current working directory relative to the worktree, if it is in the worktree.
    pub(crate) cwd: Option<&'a BStr>,
    pub(crate) should_interrupt: &'a AtomicBool,
    pub(crate) options: &'a Options,
}

impl<O> Checker<'_, O>
where
    O: gix_object::Find + Send + Clone,
{
    /// Check the paths of `plan`, and add those that are refused to `refusals`.
    pub(crate) fn check(mut self, plan: &Plan, refusals: &mut Refusals) -> Result<()> {
        let mut leading = LeadingPaths::new(self.worktree);
        let mut compared = Comparison::new(self.index);
        for (position, (change, action)) in plan.changes.iter().zip(&plan.actions).enumerate() {
            check_interrupt(self.should_interrupt)?;
            let rela_path = change.path.as_bstr();
            match *action {
                Action::Keep => {}
                Action::Delete(entry_index) | Action::Update(entry_index, _) => {
                    match leading.check(rela_path)? {
                        LeadingPath::Directories => {}
                        // The file is gone along with its directory, which Git doesn't consider a change.
                        LeadingPath::Missing => continue,
                        LeadingPath::NotADirectory(_) => {
                            refusals.unstaged_changes.push(rela_path.to_owned());
                            continue;
                        }
                    }
                    let entry = &self.index.entries()[entry_index];
                    if !entry.mode.is_submodule() {
                        compared.push(entry, rela_path, position);
                    } else if let Action::Update(_, new) = *action {
                        self.check_submodule(rela_path, new.mode, refusals)?;
                    }
                }
                Action::Add(new) => self.check_absent(&mut leading, plan, rela_path, new.mode, true, refusals)?,
                Action::Absent(old) => self.check_absent(&mut leading, plan, rela_path, old.mode, false, refusals)?,
            }
        }
        self.compare(compared, plan, refusals)
    }

    /// Compare the files of the entries in `compared` with them, and refuse the paths whose files changed, as Git's
    /// `verify_uptodate()` does.
    fn compare(&mut self, compared: Comparison, plan: &Plan, refusals: &mut Refusals) -> Result<()> {
        if compared.positions.is_empty() {
            return Ok(());
        }
        let checkout = &self.options.checkout;
        let mut changed = Changed::default();
        gix_status::index_as_worktree(
            &compared.index,
            self.worktree,
            &mut changed,
            FastEq,
            NoSubmodules,
            O::clone(self.objects),
            &mut gix_utils::progress::Discard,
            index_as_worktree::Context {
                pathspec: self.pathspec.clone(),
                stack: self.stack.clone(),
                filter: checkout.filters.clone(),
                should_interrupt: self.should_interrupt,
            },
            index_as_worktree::Options {
                fs: checkout.fs,
                thread_limit: checkout.thread_limit,
                stat: checkout.stat_options,
                fscache: self.options.fscache,
            },
        )
        .or_raise(|| message("Could not compare the worktree with the index"))?;
        check_interrupt(self.should_interrupt)?;

        changed.entries.sort_unstable();
        for (entry_index, removed) in changed.entries {
            let position = compared.positions[entry_index];
            let rela_path = plan.changes[position].path.as_bstr();
            if !removed {
                refusals.unstaged_changes.push(rela_path.to_owned());
                continue;
            }
            let refused = match self.metadata(rela_path)? {
                // The file is gone, which Git doesn't consider a change.
                None => continue,
                // A directory replaced the file of an entry that is removed. Git checks whether it could remove the
                // directory before it finds that the entry changed, and its exclude patterns take the directory for the
                // file of the entry. Nothing is removed either way.
                Some(metadata) if metadata.is_dir() && matches!(plan.actions[position], Action::Delete(_)) => {
                    if self.is_excluded(rela_path, compared.index.entries()[entry_index].mode)? {
                        &mut refusals.unstaged_changes
                    } else if self.has_untracked(rela_path, false)? {
                        &mut refusals.directories_with_untracked_files
                    } else if self.is_cwd(rela_path) {
                        &mut refusals.current_dir
                    } else {
                        &mut refusals.unstaged_changes
                    }
                }
                Some(_) => &mut refusals.unstaged_changes,
            };
            refused.push(rela_path.to_owned());
        }
        Ok(())
    }

    /// Check the worktree of the submodule at `rela_path`, whose entry becomes one of `new_mode`. Git leaves the worktree
    /// of a submodule alone, unless the new entry isn't a submodule and replaces it.
    fn check_submodule(&self, rela_path: &BStr, new_mode: Mode, refusals: &mut Refusals) -> Result<()> {
        let Some(metadata) = self.metadata(rela_path)? else {
            return Ok(());
        };
        if !metadata.is_dir() {
            // The checkout would replace the file with the directory of the submodule.
            refusals.unstaged_changes.push(rela_path.to_owned());
        } else if !new_mode.is_submodule() {
            if !self.is_empty_directory(rela_path)? {
                refusals.directories_with_untracked_files.push(rela_path.to_owned());
            } else if self.is_cwd_in(rela_path) {
                refusals.current_dir.push(rela_path.to_owned());
            }
        }
        Ok(())
    }

    /// Check that nothing is lost at `rela_path`, which has no entry, as Git's `verify_absent()` does. If `added`, an
    /// entry of `mode` is added at it. Otherwise, only the old tree has an entry of `mode` at it.
    fn check_absent(
        &mut self,
        leading: &mut LeadingPaths<'_>,
        plan: &Plan,
        rela_path: &BStr,
        mode: Mode,
        added: bool,
        refusals: &mut Refusals,
    ) -> Result<()> {
        match leading.check(rela_path)? {
            LeadingPath::Directories => {}
            LeadingPath::Missing => return Ok(()),
            LeadingPath::NotADirectory(len) => {
                // A file or a symbolic link is at the path of a directory that leads to `rela_path`.
                let leading_path = rela_path[..len].as_bstr();
                if !self.is_removed(plan, leading_path) && !self.is_excluded(leading_path, Mode::FILE)? {
                    untracked(refusals, added).push(leading_path.to_owned());
                }
                return Ok(());
            }
        }
        let Some(metadata) = self.metadata(rela_path)? else {
            return Ok(());
        };
        if !metadata.is_dir() {
            if !self.is_removed(plan, rela_path) && !self.is_excluded(rela_path, mode)? {
                untracked(refusals, added).push(rela_path.to_owned());
            }
        } else if !mode.is_submodule() {
            // A directory may be the worktree of a submodule, which Git leaves alone.
            if added {
                // The directory is replaced, along with everything in it.
                if self.has_untracked(rela_path, true)? {
                    refusals.directories_with_untracked_files.push(rela_path.to_owned());
                } else if self.is_cwd_in(rela_path) {
                    refusals.current_dir.push(rela_path.to_owned());
                }
            } else if !self.is_excluded(rela_path, mode)? {
                // The directory stays, but Git refuses it if it has untracked files or is the current working directory.
                if self.has_untracked(rela_path, false)? {
                    refusals.directories_with_untracked_files.push(rela_path.to_owned());
                } else if self.is_cwd(rela_path) {
                    refusals.current_dir.push(rela_path.to_owned());
                }
            }
        }
        Ok(())
    }

    /// Return `true` if the file at `rela_path` belongs to an entry that is removed: the entry at that path, or on a
    /// case-insensitive file system, an entry whose path only differs in case.
    fn is_removed(&self, plan: &Plan, rela_path: &BStr) -> bool {
        let entry_index = self.index.entry_index_by_path(rela_path).ok().or_else(|| {
            let entry = self.index.entry_by_path_icase(rela_path, true, self.lookup?)?;
            self.index.entry_index_by_path(entry.path(self.index)).ok()
        });
        entry_index.is_some_and(|entry_index| plan.is_deleted(entry_index))
    }

    /// Return `true` if the exclude patterns ignore an item of `mode` at `rela_path`.
    fn is_excluded(&mut self, rela_path: &BStr, mode: Mode) -> Result<bool> {
        let platform = self
            .stack
            .at_entry(rela_path, Some(mode), self.objects)
            .or_raise(|| message!("Could not read the exclude patterns of '{rela_path}'"))?;
        Ok(platform.is_excluded())
    }

    /// Return `true` if the directory at `rela_path` has untracked files, symbolic links or repositories that aren't
    /// ignored, as Git's `verify_clean_subdirectory()` finds them. If the directory is removed along with everything in
    /// it, which `for_deletion` says, other untracked items such as named pipes, ignored repositories, a `.git` that
    /// makes the directory a repository, and the worktrees of submodules count as well.
    fn has_untracked(&mut self, rela_path: &BStr, for_deletion: bool) -> Result<bool> {
        if for_deletion && self.has_submodule_worktrees(rela_path)? {
            return Ok(true);
        }
        let root = self.worktree.join(gix_path::from_bstr(rela_path)?);
        let mut delegate = Untracked {
            found: false,
            for_deletion,
            should_interrupt: self.should_interrupt,
        };
        let walk = gix_dir::walk(
            self.worktree,
            gix_dir::walk::Context {
                should_interrupt: Some(self.should_interrupt),
                git_dir_realpath: self.git_dir_realpath,
                current_dir: self.current_dir,
                index: self.index,
                ignore_case_index_lookup: self.lookup,
                pathspec: &mut self.pathspec.clone(),
                pathspec_attributes: &mut |_, _, _, _| false,
                excludes: Some(&mut self.stack),
                objects: self.objects,
                explicit_traversal_root: Some(&root),
            },
            walk_options(self.options.checkout.fs, for_deletion),
            &mut delegate,
        );
        check_interrupt(self.should_interrupt)?;
        walk.or_raise(|| message!("Could not look for untracked files in '{rela_path}'"))?;
        Ok(delegate.found)
    }

    /// Return `true` if a submodule of the index in the directory at `rela_path` has a worktree that isn't empty, or a
    /// file at its path.
    fn has_submodule_worktrees(&self, rela_path: &BStr) -> Result<bool> {
        let mut directory = rela_path.to_owned();
        directory.push(b'/');
        let Some(range) = self.index.prefixed_entries_range(directory.as_bstr()) else {
            return Ok(false);
        };
        for entry in &self.index.entries()[range] {
            if !entry.mode.is_submodule() {
                continue;
            }
            let submodule = entry.path(self.index);
            match self.metadata(submodule)? {
                Some(metadata) if metadata.is_dir() => {
                    if !self.is_empty_directory(submodule)? {
                        return Ok(true);
                    }
                }
                Some(_) => return Ok(true),
                None => {}
            }
        }
        Ok(false)
    }

    /// Return the metadata of what is at `rela_path` without following a symbolic link, or `None` if nothing is.
    fn metadata(&self, rela_path: &BStr) -> Result<Option<Metadata>> {
        match std::fs::symlink_metadata(self.worktree.join(gix_path::from_bstr(rela_path)?)) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(None),
            Err(err) => Err(err.and_raise(message!("Could not read the metadata of '{rela_path}'"))),
        }
    }

    /// Return `true` if the directory at `rela_path` is empty.
    fn is_empty_directory(&self, rela_path: &BStr) -> Result<bool> {
        let mut entries = std::fs::read_dir(self.worktree.join(gix_path::from_bstr(rela_path)?))
            .or_raise(|| message!("Could not read the directory '{rela_path}'"))?;
        Ok(entries.next().is_none())
    }

    /// Return `true` if the current working directory is the directory at `rela_path`.
    fn is_cwd(&self, rela_path: &BStr) -> bool {
        self.cwd
            .is_some_and(|cwd| path::is_at(cwd, rela_path, self.options.checkout.fs.ignore_case))
    }

    /// Return `true` if the current working directory is the directory at `rela_path`, or in it.
    fn is_cwd_in(&self, rela_path: &BStr) -> bool {
        self.cwd
            .is_some_and(|cwd| path::is_at_or_in(cwd, rela_path, self.options.checkout.fs.ignore_case))
    }
}

/// The list of `refusals` for untracked files that an entry would overwrite if it is `added`, or that the removal of an
/// entry would remove otherwise.
fn untracked(refusals: &mut Refusals, added: bool) -> &mut Vec<BString> {
    if added {
        &mut refusals.untracked_overwritten
    } else {
        &mut refusals.untracked_removed
    }
}

/// The entries whose files are compared with them, in an index of their own.
struct Comparison {
    index: State,
    /// The position of the change of each entry of `index`.
    positions: Vec<usize>,
}

impl Comparison {
    fn new(index: &State) -> Self {
        let mut compared = State::new(index.object_hash());
        compared.set_timestamp(index.timestamp());
        Comparison {
            index: compared,
            positions: Vec::new(),
        }
    }

    /// Compare the file of `entry` at `rela_path`, whose change is at `position`. Entries are pushed in the order of their
    /// paths.
    fn push(&mut self, entry: &Entry, rela_path: &BStr, position: usize) {
        // Entries that are known to be up to date or assumed to be unchanged are compared nonetheless, as Git compares them
        // before it changes their files.
        let flags = entry
            .flags
            .difference(Flags::UPTODATE | Flags::ASSUME_VALID | Flags::FSMONITOR_VALID);
        self.index
            .dangerously_push_entry(entry.stat, entry.id, flags, entry.mode, rela_path);
        self.positions.push(position);
    }
}

/// Collects the entries of the comparison whose files changed, by their index in it, and whether their files are gone or
/// became directories.
#[derive(Default)]
struct Changed {
    entries: Vec<(usize, bool)>,
}

impl<'index> VisitEntry<'index> for Changed {
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
        let removed = match status {
            // Only the stat information of the file changed.
            EntryStatus::NeedsUpdate(_) => return,
            EntryStatus::Change(Change::Removed) => true,
            EntryStatus::Change(_) | EntryStatus::Conflict { .. } | EntryStatus::IntentToAdd => false,
        };
        self.entries.push((entry_index, removed));
    }
}

/// Reports no submodule as changed, as the comparison has none.
#[derive(Clone, Copy)]
struct NoSubmodules;

impl SubmoduleStatus for NoSubmodules {
    type Output = ();

    fn status(&mut self, _entry: &Entry, _rela_path: &BStr) -> Result<Option<Self::Output>> {
        Ok(None)
    }
}

/// Options for a walk that finds the untracked files, symbolic links and repositories of a directory, without looking
/// into repositories. For a deletion, it also looks for repositories in ignored directories, and reports them along with
/// a `.git`.
fn walk_options(fs: gix_fs::Capabilities, for_deletion: bool) -> gix_dir::walk::Options<'static> {
    gix_dir::walk::Options {
        precompose_unicode: fs.precompose_unicode,
        ignore_case: fs.ignore_case,
        recurse_repositories: false,
        emit_pruned: for_deletion,
        emit_ignored: for_deletion.then_some(gix_dir::walk::EmissionMode::Matching),
        for_deletion: for_deletion
            .then_some(gix_dir::walk::ForDeletionMode::FindNonBareRepositoriesInIgnoredDirectories),
        classify_untracked_bare_repositories: false,
        emit_tracked: false,
        emit_untracked: gix_dir::walk::EmissionMode::Matching,
        emit_empty_directories: false,
        emit_collapsed: None,
        symlinks_to_directories_are_ignored_like_directories: false,
        worktree_relative_worktree_dirs: None,
    }
}

/// Stops a walk at the first item that makes a directory unsafe to remove.
struct Untracked<'a> {
    found: bool,
    for_deletion: bool,
    should_interrupt: &'a AtomicBool,
}

impl gix_dir::walk::Delegate for Untracked<'_> {
    fn emit(
        &mut self,
        entry: gix_dir::EntryRef<'_>,
        _collapsed_directory_status: Option<Status>,
    ) -> gix_dir::walk::Action {
        let found = match entry.status {
            Status::Untracked => match entry.disk_kind {
                Some(Kind::File | Kind::Symlink | Kind::Repository) => true,
                // Git doesn't see named pipes and the like, which are only lost if the directory is removed.
                Some(Kind::Untrackable) => self.for_deletion,
                Some(Kind::Directory) | None => false,
            },
            Status::Ignored(_) => self.for_deletion && matches!(entry.disk_kind, Some(Kind::Repository)),
            Status::Pruned | Status::Tracked => false,
        };
        if found || (self.for_deletion && matches!(entry.property, Some(Property::DotGit))) {
            self.found = true;
        }
        if self.found || self.should_interrupt.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}
