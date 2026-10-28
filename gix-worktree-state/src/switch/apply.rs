//! Make the index of the new tree, remove the files of the removed entries, and check out those of the added and changed
//! ones.

use std::{collections::BTreeSet, path::Path, sync::atomic::AtomicBool};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, Result, bail, message};
use gix_index::{
    State,
    entry::{Flags, Mode, Stat, stat},
};

use crate::{
    checkout,
    switch::{
        Changes, Options, Unremoved,
        path::{self, LeadingPath, LeadingPaths},
        plan::{Action, Plan, checked_out},
    },
};

/// Return the index that `index` becomes with `plan`: removed entries and resolved conflicts are gone, changed entries
/// have the object and mode of the new tree, and added entries and those that resolve conflicts are new. The entries that
/// change record a size of 0 until their files are checked out, and so do the entries that stay if their files may have
/// changed since `index` was written, as its timestamp becomes the time at which the switch or reset began. The cache
/// tree is repaired as Git repairs it, and the resolve-undo information is dropped.
pub(crate) fn result_index<O>(index: &State, plan: &Plan, objects: &O, stat_options: stat::Options) -> Result<State>
where
    O: gix_object::Exists,
{
    let mut result = index.clone();
    let mut cache_tree = result.remove_tree();
    let entries = result.entries_mut();
    for (change, action) in plan.changes.iter().zip(&plan.actions) {
        match *action {
            Action::Delete(entry_index) => entries[entry_index].flags.insert(Flags::REMOVE),
            Action::Update(entry_index, new) => {
                let entry = &mut entries[entry_index];
                entry.id = new.id;
                entry.mode = new.mode;
                entry.stat = Stat::default();
                entry.flags = Flags::empty();
            }
            Action::Resolve(start, end) => {
                for entry in &mut entries[start..end] {
                    entry.flags.insert(Flags::REMOVE);
                }
            }
            Action::Keep | Action::Add(_) | Action::Absent(_) | Action::Restore(_) => continue,
        }
        invalidate(cache_tree.as_mut(), change.path.as_bstr());
    }
    result.remove_entries(|_, _, entry| entry.flags.contains(Flags::REMOVE));

    let mut added = false;
    for (change, action) in plan.changes.iter().zip(&plan.actions) {
        let new = match *action {
            Action::Add(new) => new,
            Action::Resolve(..) => match change.new {
                Some(new) => new,
                None => continue,
            },
            Action::Keep | Action::Delete(_) | Action::Update(..) | Action::Absent(_) | Action::Restore(_) => continue,
        };
        invalidate(cache_tree.as_mut(), change.path.as_bstr());
        result.dangerously_push_entry(Stat::default(), new.id, Flags::empty(), new.mode, change.path.as_bstr());
        added = true;
    }
    if added {
        result.sort_entries();
    }

    smudge_racy_entries(&mut result, stat_options);
    let tree = result.repair_cache_tree(cache_tree.as_ref(), objects)?;
    result.set_tree(tree);
    result.remove_resolve_undo();
    Ok(result)
}

/// Invalidate the nodes of `cache_tree` along `path`, if there is a cache tree.
pub(crate) fn invalidate(cache_tree: Option<&mut gix_index::extension::Tree>, path: &BStr) {
    if let Some(cache_tree) = cache_tree {
        cache_tree.invalidate_path(path);
    }
}

/// Make the entries of `index` record a size of 0 if their files may have changed since `index` was written, as their
/// modification times aren't older than its timestamp, so that the next comparison reads their files once the timestamp
/// becomes a later time. Entries that are assumed to be unchanged, those of submodules and those that record a size of 0
/// already stay as they are.
pub(crate) fn smudge_racy_entries(index: &mut State, stat_options: stat::Options) {
    let timestamp = index.timestamp();
    for entry in index.entries_mut() {
        if !entry.mode.is_submodule()
            && !entry.flags.contains(Flags::ASSUME_VALID)
            && entry.stat.size != 0
            && entry.stat.is_racy(timestamp, stat_options)
        {
            entry.stat.size = 0;
        }
    }
}

/// Remove the files of the entries of `index` that `plan` removes, and check out the files of the added, changed and
/// restored entries of `result`, which receive the stat information of their new files.
pub(crate) fn update_worktree<O>(
    index: &State,
    result: &mut State,
    plan: &Plan,
    worktree: &Path,
    objects: O,
    cwd: Option<&BStr>,
    options: &Options,
) -> Changes
where
    O: gix_object::Find + Send + Clone,
{
    let unremoved = remove_files(index, plan, worktree, cwd, options.checkout.fs.ignore_case);
    let (checkout, checkout_error) = check_out(result, plan, worktree, objects, options);
    let (mut added, mut modified, mut removed) = (0, 0, 0);
    for (change, action) in plan.changes.iter().zip(&plan.actions) {
        match action {
            Action::Add(_) => added += 1,
            Action::Update(..) => modified += 1,
            Action::Delete(_) => removed += 1,
            Action::Resolve(..) if change.new.is_some() => modified += 1,
            Action::Resolve(..) => removed += 1,
            Action::Keep | Action::Absent(_) | Action::Restore(_) => {}
        }
    }
    Changes {
        added,
        modified,
        removed,
        unremoved,
        checkout,
        checkout_error,
    }
}

/// Remove the files of the entries of `index` that `plan` removes, and then the directories that this leaves empty, as
/// Git does, except for the current working directory `cwd` and the directories that lead to it.
fn remove_files(index: &State, plan: &Plan, worktree: &Path, cwd: Option<&BStr>, ignore_case: bool) -> Vec<Unremoved> {
    let mut unremoved = Vec::new();
    let mut leading = LeadingPaths::new(worktree);
    let mut directories = BTreeSet::new();
    for (change, action) in plan.changes.iter().zip(&plan.actions) {
        let entry_index = match *action {
            Action::Delete(entry_index) => entry_index,
            Action::Resolve(start, _) if change.new.is_none() => start,
            _ => continue,
        };
        let rela_path = change.path.as_bstr();
        let is_submodule = index.entries()[entry_index].mode.is_submodule();
        match remove_file(&mut leading, worktree, rela_path, is_submodule, cwd, ignore_case) {
            Ok(true) => {
                if let Some(slash) = rela_path.rfind_byte(b'/') {
                    directories.insert(rela_path[..slash].as_bstr().to_owned());
                }
            }
            Ok(false) => {}
            Err(error) => unremoved.push(Unremoved {
                path: rela_path.to_owned(),
                error,
            }),
        }
    }
    remove_empty_directories(worktree, directories, cwd, ignore_case);
    unremoved
}

/// Remove the file of the entry at `rela_path`, or the directory of a submodule if it `is_submodule`, as Git's
/// `unlink_entry()` does. Return `true` if it is gone, or `false` if it is left alone because the directories that lead
/// to it aren't directories.
fn remove_file(
    leading: &mut LeadingPaths<'_>,
    worktree: &Path,
    rela_path: &BStr,
    is_submodule: bool,
    cwd: Option<&BStr>,
    ignore_case: bool,
) -> Result<bool> {
    if leading.check(rela_path)? != LeadingPath::Directories {
        return Ok(false);
    }
    let file = worktree.join(gix_path::from_bstr(rela_path)?);
    let removed = if is_submodule {
        if cwd.is_some_and(|cwd| path::is_at_or_in(cwd, rela_path, ignore_case)) {
            bail!("The current working directory is in the submodule at '{rela_path}'");
        }
        // Only an empty directory is removed, as Git removes it without `--recurse-submodules`.
        std::fs::remove_dir(&file)
    } else {
        std::fs::symlink_metadata(&file).and_then(|metadata| {
            if metadata.file_type().is_symlink() {
                gix_fs::symlink::remove(&file)
            } else {
                std::fs::remove_file(&file)
            }
        })
    };
    match removed {
        Ok(()) => Ok(true),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(true),
        Err(err) => Err(err.and_raise(message!("Could not remove '{rela_path}'"))),
    }
}

/// Remove `directories` and then their parents, deepest first, as long as they are empty, except for the current working
/// directory `cwd` and the directories that lead to it.
fn remove_empty_directories(
    worktree: &Path,
    mut directories: BTreeSet<BString>,
    cwd: Option<&BStr>,
    ignore_case: bool,
) {
    while let Some(directory) = directories.pop_last() {
        if cwd.is_some_and(|cwd| path::is_at_or_in(cwd, directory.as_bstr(), ignore_case)) {
            continue;
        }
        let Ok(rela_path) = gix_path::from_bstr(directory.as_bstr()) else {
            continue;
        };
        if std::fs::remove_dir(worktree.join(rela_path)).is_err() {
            continue;
        }
        if let Some(slash) = directory.rfind_byte(b'/') {
            directories.insert(directory[..slash].as_bstr().to_owned());
        }
    }
}

/// Check out the files of the entries that `plan` adds, changes or restores, and copy their new stat information to
/// `result`.
///
/// They are checked out from an index of their own, which also has the `.gitattributes` files of `result` that don't
/// change, excluded from the checkout, so that the attributes of the checkout come from all of them.
fn check_out<O>(
    result: &mut State,
    plan: &Plan,
    worktree: &Path,
    objects: O,
    options: &Options,
) -> (checkout::Outcome, Option<gix_error::Error>)
where
    O: gix_object::Find + Send + Clone,
{
    let mut files = State::new(result.object_hash());
    for (change, action) in plan.changes.iter().zip(&plan.actions) {
        if let Some(new) = checked_out(change, *action) {
            files.dangerously_push_entry(Stat::default(), new.id, Flags::empty(), new.mode, change.path.as_bstr());
        }
    }
    if files.entries().is_empty() {
        return Default::default();
    }
    let ignore_case = options.checkout.fs.ignore_case;
    let mut attributes = 0;
    for entry in result.entries() {
        let rela_path = entry.path(result);
        let name = rela_path
            .rfind_byte(b'/')
            .map_or(rela_path, |slash| rela_path[slash + 1..].as_bstr());
        let is_attributes = if ignore_case {
            name.eq_ignore_ascii_case(b".gitattributes")
        } else {
            name == b".gitattributes".as_bstr()
        };
        if is_attributes && entry.mode == Mode::FILE && !plan.checks_out(rela_path) {
            files.dangerously_push_entry(entry.stat, entry.id, Flags::SKIP_WORKTREE, entry.mode, rela_path);
            attributes += 1;
        }
    }
    files.sort_entries();

    let mut checkout_options = options.checkout.clone();
    checkout_options.destination_is_initially_empty = false;
    checkout_options.overwrite_existing = true;
    checkout_options.keep_going = true;
    // The worktree changed already, so the checkout isn't interrupted.
    let (mut outcome, error) = match crate::checkout(
        &mut files,
        worktree,
        objects,
        &gix_utils::progress::Discard,
        &gix_utils::progress::Discard,
        &AtomicBool::new(false),
        checkout_options,
    ) {
        Ok(outcome) => (outcome, None),
        Err(err) => (checkout::Outcome::default(), Some(err)),
    };
    // The checkout counts the files that it skips, which are the `.gitattributes` files that it only reads.
    outcome.files_updated = outcome.files_updated.saturating_sub(attributes);
    for entry in files.entries() {
        if entry.flags.contains(Flags::SKIP_WORKTREE) {
            continue;
        }
        if let Ok(entry_index) = result.entry_index_by_path(entry.path(&files)) {
            result.entries_mut()[entry_index].stat = entry.stat;
        }
    }
    (outcome, error)
}
