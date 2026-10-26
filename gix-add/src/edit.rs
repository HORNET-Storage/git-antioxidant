//! Plan the work of the second pass, and turn what it found into edits of the index.

use std::ops::Range;

use bstr::{BStr, BString, ByteSlice};
use filetime::FileTime;
use gix_hash::ObjectId;
use gix_index::{
    AccelerateLookup, State,
    entry::{Flags, Mode, Stat, stat},
};

use crate::{
    Outcome,
    find::Tracked,
    read::{Update, Work},
};

/// Decide which paths the second pass reads, given the `tracked` entries that the comparison reported, by their position
/// in `index`, and the `untracked` paths that the walk found.
///
/// `lookup` is set if paths are compared case-insensitively.
pub(crate) fn plan(
    index: &State,
    lookup: Option<&AccelerateLookup<'_>>,
    tracked: &[(usize, Tracked)],
    untracked: Vec<BString>,
) -> Vec<Work> {
    let entries = index.entries();
    let removed: Vec<usize> = tracked
        .iter()
        .filter(|(_, tracked)| *tracked == Tracked::Removed)
        .map(|(entry_index, _)| *entry_index)
        .collect();
    let mut replaced = Vec::new();
    let mut untracked_work = Vec::with_capacity(untracked.len());
    for mut path in untracked {
        match entries_at(index, path.as_bstr(), lookup) {
            // Paths that have entries are added through these. Only a file or symbolic link that became a repository is
            // reported as untracked by the walk, and is added in place of its entry, as Git does.
            Some(range) => {
                if range.len() == 1 && removed.binary_search(&range.start).is_ok() {
                    replaced.push(range.start);
                    untracked_work.push(Work::Untracked {
                        path: entries[range.start].path(index).to_owned(),
                        replaces: Some(range.start),
                    });
                }
            }
            None => {
                if let Some(lookup) = lookup {
                    adjust_directory_case(index, lookup, &mut path);
                }
                untracked_work.push(Work::Untracked { path, replaces: None });
            }
        }
    }
    replaced.sort_unstable();

    let mut work = Vec::with_capacity(tracked.len() + untracked_work.len());
    for &(entry_index, tracked) in tracked {
        match tracked {
            Tracked::Changed => work.push(Work::Tracked(entry_index)),
            // A file that is gone or became a directory is checked again: the directory may be a repository with a
            // commit, which is added in its place.
            Tracked::Removed => {
                if replaced.binary_search(&entry_index).is_err() {
                    work.push(Work::Tracked(entry_index));
                }
            }
            Tracked::Unmerged => {
                let path = entries[entry_index].path(index);
                let stages = entries[entry_index..]
                    .iter()
                    .take_while(|entry| entry.path(index) == path)
                    .count();
                work.push(Work::Unmerged(entry_index..entry_index + stages));
            }
        }
    }
    work.append(&mut untracked_work);
    work
}

/// Return the positions of the entries at `path`, whose path only has to match case-insensitively if `lookup` is set.
fn entries_at(index: &State, path: &BStr, lookup: Option<&AccelerateLookup<'_>>) -> Option<Range<usize>> {
    match lookup {
        Some(lookup) => index
            .entry_by_path_icase(path, true, lookup)
            .and_then(|entry| index.entry_range(entry.path(index))),
        None => index.entry_range(path),
    }
}

/// Spell the directories of the untracked `path` as the index spells them, as Git does on case-insensitive file systems,
/// so that a file that is added to `dir` joins the entries of the directory `Dir`.
fn adjust_directory_case(index: &State, lookup: &AccelerateLookup<'_>, path: &mut BString) {
    let mut end = 0;
    while let Some(slash) = path[end..].find_byte(b'/') {
        end += slash;
        if let Some(entry) = index.entry_closest_to_directory_or_directory_icase(path[..end].as_bstr(), true, lookup) {
            path[..end].copy_from_slice(&entry.path(index)[..end]);
        }
        end += 1;
    }
}

/// A change of an entry.
#[derive(Debug, Clone, Copy)]
enum Edit {
    /// Remove the entry.
    Remove,
    /// Give the entry the mode, object and stat information of its file, which also resolves its conflict.
    Replace { mode: Mode, id: ObjectId, stat: Stat },
    /// Update the stat information of an entry whose file didn't change.
    Refresh(Stat),
}

impl Edit {
    /// Of the edits of an entry, the one with the lowest priority applies: a removal wins over a replacement, which wins
    /// over a refresh.
    fn priority(self) -> u8 {
        match self {
            Edit::Remove => 0,
            Edit::Replace { .. } => 1,
            Edit::Refresh(_) => 2,
        }
    }
}

/// An entry for a path that wasn't tracked.
struct Addition {
    path: BString,
    mode: Mode,
    id: ObjectId,
    stat: Stat,
}

/// The changes of an index.
pub(crate) struct Edits {
    /// At most one edit per entry, by the position of the entry, in order.
    edits: Vec<(usize, Edit)>,
    /// The entries to add.
    additions: Vec<Addition>,
    /// The positions of the entries without edits whose files may have changed since the index was last written, which
    /// record a size of 0 so that the next comparison reads them.
    smudge: Vec<usize>,
}

impl Edits {
    /// Turn the `updates` that the second pass found for each item of `work` into the changes of `index`.
    ///
    /// Entries whose files were modified at or after `start` record a size of 0, so that the next comparison reads them
    /// again instead of trusting their stat information. So do the entries outside of the pathspec whose files may have
    /// changed since `index` was last written, as these weren't read and the timestamp of `index` becomes `start`.
    pub(crate) fn new(
        index: &State,
        work: Vec<Work>,
        updates: Vec<Update>,
        start: FileTime,
        stat_options: stat::Options,
    ) -> Self {
        let entries = index.entries();
        let racy = |mode: Mode, mut stat: Stat| {
            if !mode.is_submodule() && stat.is_racy(start, stat_options) {
                stat.size = 0;
            }
            stat
        };
        let mut edits = Vec::new();
        let mut additions = Vec::new();
        for (work, update) in work.into_iter().zip(updates) {
            match work {
                Work::Tracked(entry_index) => match update {
                    Update::Gone => edits.push((entry_index, Edit::Remove)),
                    Update::Unchanged => {}
                    Update::Entry { mode, id, stat } => {
                        let entry = &entries[entry_index];
                        let stat = racy(mode, stat);
                        let edit =
                            if mode == entry.mode && id == entry.id && !entry.flags.contains(Flags::INTENT_TO_ADD) {
                                Edit::Refresh(stat)
                            } else {
                                Edit::Replace { mode, id, stat }
                            };
                        edits.push((entry_index, edit));
                    }
                },
                Work::Unmerged(range) => match update {
                    Update::Entry { mode, id, stat } => {
                        let stat = racy(mode, stat);
                        edits.push((range.start, Edit::Replace { mode, id, stat }));
                        edits.extend((range.start + 1..range.end).map(|entry_index| (entry_index, Edit::Remove)));
                    }
                    Update::Gone | Update::Unchanged => {
                        edits.extend(range.map(|entry_index| (entry_index, Edit::Remove)));
                    }
                },
                Work::Untracked { path, replaces } => match (update, replaces) {
                    (Update::Entry { mode, id, stat }, Some(entry_index)) => {
                        let stat = racy(mode, stat);
                        edits.push((entry_index, Edit::Replace { mode, id, stat }));
                    }
                    (Update::Entry { mode, id, stat }, None) => additions.push(Addition {
                        path,
                        mode,
                        id,
                        stat: racy(mode, stat),
                    }),
                    (Update::Gone | Update::Unchanged, Some(entry_index)) => edits.push((entry_index, Edit::Remove)),
                    (Update::Gone | Update::Unchanged, None) => {}
                },
            }
        }
        for addition in &additions {
            remove_entries_in_the_way(index, addition.path.as_bstr(), &mut edits);
        }
        edits.sort_unstable_by_key(|&(entry_index, edit)| (entry_index, edit.priority()));
        edits.dedup_by_key(|(entry_index, _)| *entry_index);

        let timestamp = index.timestamp();
        let mut edited = edits.iter().map(|&(entry_index, _)| entry_index).peekable();
        let mut smudge = Vec::new();
        for (entry_index, entry) in entries.iter().enumerate() {
            if edited.next_if_eq(&entry_index).is_some() {
                continue;
            }
            if !entry.mode.is_submodule()
                && !entry.flags.contains(Flags::ASSUME_VALID)
                && entry.stat.size != 0
                && entry.stat.is_racy(timestamp, stat_options)
            {
                smudge.push(entry_index);
            }
        }
        Edits {
            edits,
            additions,
            smudge,
        }
    }

    /// Count the paths that are added, modified and removed, and the entries that are refreshed.
    pub(crate) fn count(&self, index: &State) -> Outcome {
        let entries = index.entries();
        let mut outcome = Outcome {
            added: self.additions.len(),
            ..Outcome::default()
        };
        for edits in self
            .edits
            .chunk_by(|(a, _), (b, _)| entries[*a].path(index) == entries[*b].path(index))
        {
            if edits.iter().any(|(_, edit)| matches!(edit, Edit::Replace { .. })) {
                outcome.modified += 1;
            } else if edits.iter().any(|(_, edit)| matches!(edit, Edit::Remove)) {
                outcome.removed += 1;
            } else {
                outcome.refreshed += 1;
            }
        }
        outcome
    }

    /// Apply the changes to `index`, whose timestamp becomes `start`, and invalidate its cache tree along the paths whose
    /// entries changed.
    pub(crate) fn apply(self, index: &mut State, start: FileTime) {
        let mut cache_tree = index.remove_tree();
        let (entries, paths) = index.entries_mut_and_pathbacking();
        for (entry_index, edit) in self.edits {
            let entry = &mut entries[entry_index];
            match edit {
                Edit::Refresh(stat) => entry.stat = stat,
                Edit::Replace { mode, id, stat } => {
                    entry.mode = mode;
                    entry.id = id;
                    entry.stat = stat;
                    entry.flags = Flags::empty();
                    invalidate(cache_tree.as_mut(), entry.path_in(paths));
                }
                Edit::Remove => {
                    entry.flags.insert(Flags::REMOVE);
                    invalidate(cache_tree.as_mut(), entry.path_in(paths));
                }
            }
        }
        for entry_index in self.smudge {
            entries[entry_index].stat.size = 0;
        }
        index.remove_entries(|_, _, entry| entry.flags.contains(Flags::REMOVE));

        let has_additions = !self.additions.is_empty();
        for Addition { path, mode, id, stat } in self.additions {
            invalidate(cache_tree.as_mut(), path.as_bstr());
            index.dangerously_push_entry(stat, id, Flags::empty(), mode, path.as_bstr());
        }
        if has_additions {
            index.sort_entries();
        }
        if let Some(cache_tree) = cache_tree {
            index.set_tree(cache_tree);
        }
        index.set_timestamp(start);
    }
}

/// Remove the entries that are in the way of an entry at `path`, as Git does when it adds one: those in a directory at
/// `path`, and those at the paths of the directories that lead to `path`.
fn remove_entries_in_the_way(index: &State, path: &BStr, edits: &mut Vec<(usize, Edit)>) {
    let mut directory = path.to_owned();
    directory.push(b'/');
    if let Some(range) = index.prefixed_entries_range(directory.as_bstr()) {
        edits.extend(range.map(|entry_index| (entry_index, Edit::Remove)));
    }
    let mut end = 0;
    while let Some(slash) = path[end..].find_byte(b'/') {
        end += slash;
        if let Some(range) = index.entry_range(path[..end].as_bstr()) {
            edits.extend(range.map(|entry_index| (entry_index, Edit::Remove)));
        }
        end += 1;
    }
}

/// Invalidate the nodes of `cache_tree` along `path`, if there is a cache tree.
fn invalidate(cache_tree: Option<&mut gix_index::extension::Tree>, path: &BStr) {
    if let Some(cache_tree) = cache_tree {
        cache_tree.invalidate_path(path);
    }
}
