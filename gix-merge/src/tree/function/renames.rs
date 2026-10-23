//! The renames of each side of a merge, found as Git's merge-ort finds them in `detect_and_process_renames()`, and
//! their effect on the changes of both sides.
//!
//! merge-ort pairs the files that a side deleted with the files that it added: identical files always, and others only
//! where the merge needs to know where a deleted file went, which the [walk](super::walk) of the three trees decides. A
//! pair turns the addition into a [rewrite](Change::Rewrite) of the deleted file, whose deletion then leaves the
//! changes of the side. Where a side removed a directory and more of its files went to one other directory than to any
//! other, merge-ort takes the directory as renamed, and the files that the other side added to it or renamed into it
//! follow it to its new location, unless something is in the way there or several files would end up at the same path.

use std::collections::{BTreeSet, HashMap, hash_map};

use bstr::{BStr, BString, ByteSlice};
use gix_diff::tree_with_rewrites::Change;
use gix_diffcore::rename::{self as diffcore, MAX_SCORE};
use gix_error::{OptionExt, Result, ResultExt, message};
use gix_filter::attributes::{StateRef, search::Outcome as Attributes};
use gix_hash::ObjectId;
use gix_object::{
    FindExt,
    tree::{EntryKind, EntryMode},
};

use super::walk::{self, Pair, PathFacts, Walk};
use crate::tree::{
    Conflict, ConflictIndexEntry, ConflictMapping, DirectoryRenames, Resolution, ResolutionFailure, TreatAsUnresolved,
    utils::{ChangeList, ChangeListRef},
};

/// What [`find()`] leaves for the merge to record.
#[derive(Default)]
pub(super) struct Outcome {
    /// The directory renames whose files went to several directories equally often, followed by the changes that a
    /// directory rename couldn't move.
    pub conflicts: Vec<Conflict>,
    /// The changes that a directory rename moved.
    pub moves: Vec<Move>,
}

/// A change of one side that a directory rename of the other side moved, as merge-ort's
/// `apply_directory_rename_modifications()` moves it.
pub(super) struct Move {
    /// The side of the change, 0 for ours and 1 for theirs.
    side: usize,
    /// The change before it was moved.
    original: Change,
    /// The change at its new location.
    moved: Change,
    /// The directory rename of the other side, as the rewrite of the directory in the merge-base to the directory on
    /// that side.
    directory: Change,
    /// The stages of the new location as far as the changes of the other side decide them, as merge-ort gives them to
    /// the path: the merge-base, ours and theirs.
    entries: [Option<ConflictIndexEntry>; 3],
    /// Whether the move returned a renamed file to its source, which the other side renamed as well. merge-ort then
    /// merges the file at the destination of the rename of the other side and removes the source.
    taken_by_rename: bool,
}

impl Move {
    /// Record the move as merge-ort reports it with `directory_renames`, once the merge recorded
    /// `resolver_conflicts` while it resolved the changes of both sides.
    ///
    /// With [`DirectoryRenames::Move`] the move is [resolved](Resolution::SourceLocationAffectedByRename), as is a move
    /// that returned a renamed file to a source that the other side renamed as well: merge-ort merges the file at the
    /// destination of that rename and removes the source. Otherwise the moved change is in conflict at its new
    /// location, with the stages that merge-ort gives the path: those of a resolved conflict that the merge recorded
    /// for the moved change or at its new location, or else those of the move itself. If such a conflict remains
    /// unresolved, it has these stages already, and the record has none.
    pub(super) fn record(&self, directory_renames: DirectoryRenames, resolver_conflicts: &[Conflict]) -> Conflict {
        let final_location = self.moved.location().to_owned();
        let changes = directory_conflict(self.side, &self.directory, &self.original);
        if directory_renames == DirectoryRenames::Move || self.taken_by_rename {
            return Conflict::with_resolution(
                Resolution::SourceLocationAffectedByRename { final_location },
                changes,
                [None; 3],
            );
        }
        let related = |conflict: &&Conflict| {
            [&conflict.ours, &conflict.theirs]
                .into_iter()
                .any(|change| *change == self.moved || change.location() == final_location.as_bstr())
        };
        let entries = if resolver_conflicts
            .iter()
            .filter(&related)
            .any(|conflict| conflict.is_unresolved(TreatAsUnresolved::git()))
        {
            [None; 3]
        } else {
            resolver_conflicts
                .iter()
                .filter(&related)
                .rev()
                .map(Conflict::entries)
                .find(|entries| entries.iter().any(Option::is_some))
                .map_or(self.entries, |entries| {
                    entries.map(|entry| {
                        entry.map(|entry| ConflictIndexEntry {
                            path_hint: None,
                            ..entry
                        })
                    })
                })
        };
        // The conflict stores its entries in the order of its description, which puts the directory rename first.
        let entries = if self.side == 0 {
            [entries[0], entries[2], entries[1]]
        } else {
            entries
        };
        Conflict::without_resolution(
            ResolutionFailure::SourceLocationAffectedByRename { final_location },
            changes,
            entries,
        )
    }
}

/// Return the location that each change moved by one of `moves` had before the move, by the side of the change and its
/// location after the move, as merge-ort keeps it in the `pathnames` of the conflict at the new location.
pub(super) fn locations_before_moves(moves: &[Move]) -> [HashMap<&BStr, &BStr>; 2] {
    let mut locations: [HashMap<&BStr, &BStr>; 2] = Default::default();
    for moved in moves {
        locations[moved.side].insert(moved.moved.location(), moved.original.location());
    }
    locations
}

/// What is known about the path to which the directory renames of the other side would move files of a side.
#[derive(Default)]
struct Collision {
    /// The paths of the files of the side that would be moved there.
    sources: BTreeSet<BString>,
    /// Why the files can't be moved there, once that was found.
    reported: Option<Blocked>,
}

/// Why a directory rename can't move a file to its new location, as merge-ort's `handle_path_level_conflicts()`
/// decides.
#[derive(Clone, Copy)]
enum Blocked {
    /// Something is in the way at the new location.
    InTheWay,
    /// Several files would be moved to the new location.
    Collision,
}

/// Find the renames of each side of the merge of `trees`, the merge-base, ours and theirs, as merge-ort does, apply
/// them to the `changes` of each side, which have no rewrites yet, and [order](schedule()) the changes of each side.
///
/// `objects` provides the trees and blobs, and `diff_resource_cache` the attributes and diff drivers that decide
/// which blobs are binary when the similarity of files is estimated. `rewrites` sets the similarity that a rename
/// needs, with `None` pairing identical files only, how many files have their similarity estimated, and whether files
/// with empty blobs are paired. Copies are never detected. `directory_renames` decides whether the directory renames
/// of each side move the changes of the other side.
pub(super) fn find(
    trees: [&gix_hash::oid; 3],
    changes: &mut [ChangeList; 2],
    objects: &impl gix_object::FindObjectOrHeader,
    diff_resource_cache: &mut gix_diff::blob::Platform,
    rewrites: gix_diff::Rewrites,
    directory_renames: DirectoryRenames,
) -> Result<Outcome> {
    let mut outcome = Outcome::default();
    let [ours, theirs] = &*changes;
    if !may_have_renames(ours) && !may_have_renames(theirs) {
        changes.iter_mut().for_each(schedule);
        return Ok(outcome);
    }
    let Walk { sides, mut paths } = walk::walk(trees[0], trees[1], trees[2], objects)?;
    let change_of_pair: [Vec<Option<usize>>; 2] =
        std::array::from_fn(|side| changes_of_pairs(&sides[side].pairs, &changes[side]));
    // The changes that leave each side: the deletions that renames paired, and the directories that the side added
    // which only held files that a directory rename moved away.
    let mut removed: [Vec<bool>; 2] = std::array::from_fn(|side| vec![false; changes[side].len()]);

    let mut attributes = Attributes::default();
    attributes.initialize_with_selection(&Default::default(), ["diff"]);
    let mut blobs = MergeBlobs {
        objects,
        attr_stack: &mut diff_resource_cache.attr_stack,
        drivers: diff_resource_cache.filter.drivers(),
        attributes,
    };
    let options = diffcore::merge::Options {
        min_score: rewrites.percentage.map_or(MAX_SCORE, |percentage| {
            (percentage.clamp(0.0, 1.0) * MAX_SCORE as f32).round() as u32
        }),
        limit: rewrites.limit,
        rename_empty: rewrites.track_empty,
    };
    let mut directory_counts: [HashMap<BString, HashMap<BString, usize>>; 2] = Default::default();
    for (side, recorded) in sides.iter().enumerate() {
        // As merge-ort does, look for the renames of a side only if the merge needs to know where one of its deleted
        // files went.
        if recorded.pairs.is_empty() || recorded.relevant_sources.is_empty() {
            continue;
        }
        let (deletions, additions): (Vec<usize>, Vec<usize>) =
            (0..recorded.pairs.len()).partition(|&pair| !recorded.pairs[pair].is_addition);
        let sources: Vec<_> = deletions
            .iter()
            .map(|&pair| {
                let pair = &recorded.pairs[pair];
                diffcore::merge::Source {
                    entry: diffcore_entry(pair),
                    relevance: recorded.relevant_sources.get(&pair.location).copied(),
                }
            })
            .collect();
        let destinations: Vec<_> = additions
            .iter()
            .map(|&pair| diffcore_entry(&recorded.pairs[pair]))
            .collect();
        let detected = diffcore::merge::detect(&sources, &destinations, &recorded.dirs_removed, &mut blobs, options)?;
        directory_counts[side] = detected.directory_renames;
        for rename in detected.renames {
            let (Some(deletion), Some(addition)) = (
                change_of_pair[side][deletions[rename.source]],
                change_of_pair[side][additions[rename.destination]],
            ) else {
                continue;
            };
            let Some(rewrite) = rewrite(&changes[side][deletion].inner, &changes[side][addition].inner) else {
                debug_assert!(false, "renames pair the deletion of a file with the addition of a file");
                continue;
            };
            changes[side][addition].inner = rewrite;
            removed[side][deletion] = true;
        }
    }

    if directory_renames != DirectoryRenames::Ignore {
        let mut buf = Vec::new();
        let null = ObjectId::null(trees[0].kind());
        // A directory was renamed to the directory to which more of its files went than to any other, which is a
        // conflict if several directories received that many.
        let mut dir_renames: [HashMap<BString, BString>; 2] = Default::default();
        for (side, counts) in directory_counts.into_iter().enumerate() {
            let mut splits = Vec::new();
            for (directory, destinations) in counts {
                let Some(most) = destinations.values().copied().max().filter(|&most| most > 0) else {
                    continue;
                };
                let mut best = destinations
                    .iter()
                    .filter(|&(_, &count)| count == most)
                    .map(|(target, _)| target);
                match (best.next(), best.next()) {
                    (Some(target), None) => {
                        let target = target.clone();
                        dir_renames[side].insert(directory, target);
                    }
                    _ => splits.push(directory),
                }
            }
            splits.sort();
            for directory in splits {
                let id = tree_at(objects, trees[0], directory.as_bstr(), &mut buf)?.unwrap_or(null);
                let removal = Change::Deletion {
                    location: directory,
                    relation: None,
                    entry_mode: EntryKind::Tree.into(),
                    id,
                };
                let map = if side == 0 {
                    ConflictMapping::Original
                } else {
                    ConflictMapping::Swapped
                };
                outcome.conflicts.push(Conflict::without_resolution(
                    ResolutionFailure::DirectoryRenameSplit,
                    (&removal, &removal, map, ConflictMapping::Original),
                    [None; 3],
                ));
            }
        }
        // A directory that both sides renamed is renamed by neither.
        let [our_renames, their_renames] = &mut dir_renames;
        our_renames.retain(|directory, _| their_renames.remove(directory).is_none());

        // Where the directory renames of the other side would move the files that each side added or renamed.
        let mut collisions: [HashMap<BString, Collision>; 2] = Default::default();
        for (side, collisions) in collisions.iter_mut().enumerate() {
            let renames = &dir_renames[1 - side];
            if renames.is_empty() {
                continue;
            }
            for pair in sides[side].pairs.iter().filter(|pair| pair.is_addition) {
                let path = pair.location.as_bstr();
                if let Some((old, new)) = renamed_directory(path, renames) {
                    collisions
                        .entry(apply_rename(path, old, new))
                        .or_default()
                        .sources
                        .insert(pair.location.clone());
                }
            }
        }

        // Move these files, ours first, as merge-ort's `collect_renames()` does.
        let mut directories: [HashMap<BString, Change>; 2] = Default::default();
        for side in 0..2 {
            let other = 1 - side;
            if dir_renames[other].is_empty() {
                continue;
            }
            for (pair, change) in sides[side].pairs.iter().zip(&change_of_pair[side]) {
                let Some(change) = change.filter(|_| pair.is_addition) else {
                    continue;
                };
                let path = pair.location.as_bstr();
                if collisions[other].contains_key(path) {
                    continue;
                }
                let Some((old_dir, new_dir)) = renamed_directory(path, &dir_renames[other]) else {
                    continue;
                };
                // A file isn't moved into a directory that its own side renamed, as it would then be renamed again.
                if dir_renames[side].contains_key(new_dir) {
                    continue;
                }
                let new_path = apply_rename(path, old_dir, new_dir);
                let source = match &changes[side][change].inner {
                    Change::Rewrite { source_location, .. } => source_location.as_bstr(),
                    change => change.location(),
                };
                let blocked = collisions[side].get_mut(&new_path).and_then(|collision| {
                    if collision.reported.is_none() {
                        collision.reported = if path_in_way(&paths, new_path.as_bstr(), 2 << side, source) {
                            Some(Blocked::InTheWay)
                        } else if collision.sources.len() > 1 {
                            Some(Blocked::Collision)
                        } else {
                            None
                        };
                    }
                    collision.reported
                });
                let directory = match directories[other].entry(old_dir.to_owned()) {
                    hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    hash_map::Entry::Vacant(entry) => {
                        let source_id = tree_at(objects, trees[0], old_dir, &mut buf)?.unwrap_or(null);
                        let id = tree_at(objects, trees[1 + other], new_dir, &mut buf)?.unwrap_or(null);
                        entry.insert(Change::Rewrite {
                            source_location: old_dir.to_owned(),
                            source_entry_mode: EntryKind::Tree.into(),
                            source_relation: None,
                            source_id,
                            diff: None,
                            entry_mode: EntryKind::Tree.into(),
                            id,
                            location: new_dir.to_owned(),
                            relation: None,
                            copy: false,
                        })
                    }
                };
                if let Some(blocked) = blocked {
                    let destination = new_path;
                    let failure = match blocked {
                        Blocked::InTheWay => ResolutionFailure::FileInWayOfDirectoryRename { destination },
                        Blocked::Collision => ResolutionFailure::DirectoryRenameCollision { destination },
                    };
                    outcome.conflicts.push(Conflict::without_resolution(
                        failure,
                        directory_conflict(side, directory, &changes[side][change].inner),
                        [None; 3],
                    ));
                    continue;
                }
                move_path_facts(&mut paths, path, new_path.as_bstr());
                let original = changes[side][change].inner.clone();
                move_to(&mut changes[side][change].inner, new_path);
                // A rename that the move returns to its source is a rename to itself, which merge-ort's
                // `process_renames()` skips, so the file stays at its source as a modification of it. That function
                // takes the renames of both sides by source, ours first, and so still pairs such a rename of theirs
                // with a rename of the same file by ours, as rename/rename(1to2).
                if let Some(modification) = modification_of_source(&changes[side][change].inner)
                    && (side == 0 || !renames_source(&changes[0], modification.location()))
                {
                    changes[side][change].inner = modification;
                }
                outcome.moves.push(Move {
                    side,
                    original,
                    moved: changes[side][change].inner.clone(),
                    directory: directory.clone(),
                    entries: [None; 3],
                    taken_by_rename: false,
                });
            }
        }

        // A directory that a side added and that now holds no file of the side would otherwise still occupy its path.
        for (side, removed) in removed.iter_mut().enumerate() {
            let moved: Vec<&BStr> = outcome
                .moves
                .iter()
                .filter(|moved| moved.side == side)
                .map(|moved| moved.original.location())
                .collect();
            if moved.is_empty() {
                continue;
            }
            let files: BTreeSet<&BStr> = changes[side]
                .iter()
                .zip(removed.iter())
                .filter(|(change, removed)| !**removed && !change.inner.entry_mode().is_tree())
                .map(|(change, _)| change.inner.location())
                .collect();
            for (change, removed) in changes[side].iter().zip(removed.iter_mut()) {
                if let Change::Addition {
                    location, entry_mode, ..
                } = &change.inner
                    && entry_mode.is_tree()
                {
                    let mut prefix = location.clone();
                    prefix.push(b'/');
                    let below = |path: &&BStr| path.starts_with(&prefix);
                    if moved.iter().any(below) && !files.range(prefix.as_bstr()..).next().is_some_and(below) {
                        *removed = true;
                    }
                }
            }
        }
    }

    for (changes, removed) in changes.iter_mut().zip(&removed) {
        let mut removed = removed.iter();
        changes.retain(|_| !removed.next().copied().unwrap_or_default());
        schedule(changes);
    }
    set_entries(&mut outcome.moves, changes);
    Ok(outcome)
}

/// Order the `changes` of a side for the merge: the modifications in the order of the diff, the renames by the location
/// of their source, as merge-ort's `process_renames()` takes them, and then the additions and deletions by location.
///
/// The renames are thus resolved before the additions and deletions at and below their source and destination are
/// matched with the other side. If ours modifies the file `a` while theirs renames it to `a/sub/b` and adds `a/d`,
/// the modification follows the rename to `a/sub/b` before `a/d` meets the file `a`. Additions and deletions at the
/// same location are ordered by id, so that the order doesn't depend on the order of the diff.
fn schedule(changes: &mut ChangeList) {
    fn key(change: &Change) -> (u8, &[u8], &[u8]) {
        match change {
            Change::Modification { .. } => (0, &[], &[]),
            Change::Rewrite { source_location, .. } => (1, source_location.as_slice(), &[]),
            Change::Addition { location, id, .. } | Change::Deletion { location, id, .. } => {
                (2, location.as_slice(), id.as_bytes())
            }
        }
    }
    changes.sort_by(|a, b| key(&a.inner).cmp(&key(&b.inner)));
}

/// Whether `changes` deletes a file and adds one, so that rename detection may pair them.
fn may_have_renames(changes: &ChangeListRef) -> bool {
    let files = || {
        changes
            .iter()
            .map(|change| &change.inner)
            .filter(|change| !change.entry_mode().is_tree())
    };
    files().any(|change| matches!(change, Change::Deletion { .. }))
        && files().any(|change| matches!(change, Change::Addition { .. }))
}

/// The index in `changes` of the deletion or addition of each of the `pairs` of the same side.
fn changes_of_pairs(pairs: &[Pair], changes: &ChangeListRef) -> Vec<Option<usize>> {
    let files: HashMap<(&BStr, bool), usize> = changes
        .iter()
        .enumerate()
        .filter_map(|(index, change)| match &change.inner {
            Change::Addition {
                location, entry_mode, ..
            } if !entry_mode.is_tree() => Some(((location.as_bstr(), true), index)),
            Change::Deletion {
                location, entry_mode, ..
            } if !entry_mode.is_tree() => Some(((location.as_bstr(), false), index)),
            _ => None,
        })
        .collect();
    pairs
        .iter()
        .map(|pair| {
            let index = files.get(&(pair.location.as_bstr(), pair.is_addition)).copied();
            debug_assert!(
                index.is_some(),
                "the changes of a side have each file that the walk paired: {pair:?}"
            );
            index
        })
        .collect()
}

/// The file of `pair` as rename detection takes it.
fn diffcore_entry(pair: &Pair) -> diffcore::Entry<'_> {
    diffcore::Entry {
        location: pair.location.as_bstr(),
        mode: pair.mode,
        id: &pair.id,
    }
}

/// The rename of the file of `deletion` to the file of `addition`.
fn rewrite(deletion: &Change, addition: &Change) -> Option<Change> {
    match (deletion, addition) {
        (
            Change::Deletion {
                location: source_location,
                relation: source_relation,
                entry_mode: source_entry_mode,
                id: source_id,
            },
            Change::Addition {
                location,
                relation,
                entry_mode,
                id,
            },
        ) => Some(Change::Rewrite {
            source_location: source_location.clone(),
            source_entry_mode: *source_entry_mode,
            source_relation: *source_relation,
            source_id: *source_id,
            diff: None,
            entry_mode: *entry_mode,
            id: *id,
            location: location.clone(),
            relation: *relation,
            copy: false,
        }),
        _ => None,
    }
}

/// The changes of a conflict between the directory rename `directory` of one side and the `change` of the other side,
/// `side`, in the order of the merge, with the mapping that describes the directory rename first.
fn directory_conflict<'a>(
    side: usize,
    directory: &'a Change,
    change: &'a Change,
) -> (&'a Change, &'a Change, ConflictMapping, ConflictMapping) {
    if side == 1 {
        (directory, change, ConflictMapping::Original, ConflictMapping::Original)
    } else {
        (change, directory, ConflictMapping::Swapped, ConflictMapping::Original)
    }
}

/// The id of the tree at `path` in the tree `root`, read from `objects`, if there is a tree at `path`.
fn tree_at(
    objects: &dyn gix_object::Find,
    root: &gix_hash::oid,
    path: &BStr,
    buf: &mut Vec<u8>,
) -> Result<Option<ObjectId>> {
    let mut id = root.to_owned();
    if path.is_empty() {
        return Ok(Some(id));
    }
    for component in path.split_str("/") {
        let tree = objects.find_tree(&id, buf)?;
        let Some(entry) = tree
            .entries
            .iter()
            .find(|entry| entry.filename == component.as_bstr() && entry.mode.is_tree())
        else {
            return Ok(None);
        };
        id = entry.oid.to_owned();
    }
    Ok(Some(id))
}

/// The directory rename among `renames` of the nearest directory above `path` that one renames, as merge-ort's
/// `check_dir_renamed()` finds it.
fn renamed_directory<'a>(path: &BStr, renames: &'a HashMap<BString, BString>) -> Option<(&'a BStr, &'a BStr)> {
    let mut directory = path;
    while let Some(slash) = directory.rfind_byte(b'/') {
        directory = directory[..slash].as_bstr();
        if let Some((old, new)) = renames.get_key_value(directory) {
            return Some((old.as_bstr(), new.as_bstr()));
        }
    }
    None
}

/// The location of `path` once its directory `old`, or one above it, was renamed to `new`, as merge-ort's
/// `apply_dir_rename()` computes it.
fn apply_rename(path: &BStr, old: &BStr, new: &BStr) -> BString {
    // A directory renamed into the root leaves no `/` in front of the rest of the path.
    let rest = if new.is_empty() { old.len() + 1 } else { old.len() };
    let mut renamed = new.to_owned();
    renamed.extend_from_slice(&path[rest..]);
    renamed
}

/// Whether something is in the way at `path` of a file that a directory rename moves there from `source` for the side
/// with the bit `side_mask`, as merge-ort's `path_in_way()` decides: the path is merged already, the side has an
/// entry there, or the merge-base has a file there that isn't the source of the moved file.
fn path_in_way(paths: &HashMap<BString, PathFacts>, path: &BStr, side_mask: u8, source: &BStr) -> bool {
    paths.get(path).is_some_and(|facts| {
        facts.clean
            || (side_mask & (facts.filemask | facts.dirmask)) != 0
            || ((facts.filemask & 1) != 0 && source != path)
    })
}

/// Move what is known about the file at `old` to `new`, as merge-ort's `apply_directory_rename_modifications()` moves
/// it: the directories above `new` that the merge doesn't know yet become directories of the side of the file, a
/// directory that remains at `old` keeps only what is known about directories, and the file joins what is known
/// about `new` if something is.
fn move_path_facts(paths: &mut HashMap<BString, PathFacts>, old: &BStr, new: &BStr) {
    let Some(facts) = paths.get(old).copied() else {
        debug_assert!(false, "the walk visited each file that it paired: {old}");
        return;
    };
    let mut directory = new;
    while let Some(slash) = directory.rfind_byte(b'/') {
        directory = directory[..slash].as_bstr();
        if paths.contains_key(directory) {
            break;
        }
        paths.insert(
            directory.to_owned(),
            PathFacts {
                clean: false,
                filemask: 0,
                dirmask: facts.filemask,
                match_mask: 0,
                df_conflict: false,
            },
        );
    }
    let moved = if facts.dirmask == 0 {
        paths.remove(old);
        facts
    } else {
        if let Some(directory) = paths.get_mut(old) {
            directory.filemask = 0;
            directory.clean = true;
        }
        PathFacts { dirmask: 0, ..facts }
    };
    match paths.entry(new.to_owned()) {
        hash_map::Entry::Occupied(mut existing) => {
            let existing = existing.get_mut();
            existing.filemask |= moved.filemask;
            if existing.dirmask != 0 {
                existing.df_conflict = true;
            }
        }
        hash_map::Entry::Vacant(entry) => {
            entry.insert(moved);
        }
    }
}

/// Move `change`, an addition or the destination of a rename, to `new_location`, where it belongs to no directory
/// that its side added.
fn move_to(change: &mut Change, new_location: BString) {
    match change {
        Change::Addition { location, relation, .. } | Change::Rewrite { location, relation, .. } => {
            *location = new_location;
            *relation = None;
        }
        Change::Deletion { .. } | Change::Modification { .. } => {
            debug_assert!(false, "directory renames move additions and renames");
        }
    }
}

/// The modification of its source that `change` amounts to if it is a rename to its own source.
fn modification_of_source(change: &Change) -> Option<Change> {
    match change {
        Change::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            entry_mode,
            id,
            location,
            ..
        } if source_location == location => Some(Change::Modification {
            location: location.clone(),
            previous_entry_mode: *source_entry_mode,
            previous_id: *source_id,
            entry_mode: *entry_mode,
            id: *id,
        }),
        _ => None,
    }
}

/// Whether one of `changes` renames the file at `source`.
fn renames_source(changes: &ChangeListRef, source: &BStr) -> bool {
    changes
        .iter()
        .any(|change| matches!(&change.inner, Change::Rewrite { source_location, .. } if source_location == source))
}

/// Set the entries of each of the `moves` to the stages that merge-ort gives its new location if no conflict of the
/// merge decides them, after `changes` reached their final state: the moved file, the file that the side that
/// renamed the directory has at the new location, and for a moved rename, the file in the merge-base and the version
/// of it that the other side kept, modified or renamed to the same new location, which merge-ort's rename/rename(1to1)
/// gives the new location. If that side deleted the file, the new location has only its merge-base, and if that side
/// renamed it elsewhere, it has neither. A rename that the move returned to its source is a modification of the source
/// by then and gets the same stages, unless the other side renamed the source as well, which takes the move.
fn set_entries(moves: &mut [Move], changes: &[ChangeList; 2]) {
    for renaming in 0..2 {
        let side = 1 - renaming;
        if !moves.iter().any(|moved| moved.side == side) {
            continue;
        }
        let (mut at_location, mut at_source) = (HashMap::new(), HashMap::new());
        for change in changes[renaming]
            .iter()
            .map(|change| &change.inner)
            .filter(|change| !change.entry_mode().is_tree())
        {
            match change {
                Change::Addition { location, .. } => {
                    at_location.insert(location.as_bstr(), change);
                }
                Change::Deletion { location, .. } => {
                    at_source.insert(location.as_bstr(), change);
                }
                Change::Modification { location, .. } => {
                    at_location.insert(location.as_bstr(), change);
                    at_source.insert(location.as_bstr(), change);
                }
                Change::Rewrite {
                    source_location,
                    location,
                    ..
                } => {
                    at_location.insert(location.as_bstr(), change);
                    at_source.insert(source_location.as_bstr(), change);
                }
            }
        }
        for moved in moves.iter_mut().filter(|moved| moved.side == side) {
            let mut entries = [None; 3];
            let (mode, id) = moved.moved.entry_mode_and_id();
            entries[1 + side] = index_entry(mode, id);
            match &moved.moved {
                Change::Rewrite {
                    source_location,
                    source_entry_mode,
                    source_id,
                    ..
                } => {
                    let base = index_entry(*source_entry_mode, source_id);
                    match at_source.get(source_location.as_bstr()) {
                        None => {
                            entries[0] = base;
                            entries[1 + renaming] = base;
                        }
                        Some(Change::Modification { entry_mode, id, .. }) => {
                            entries[0] = base;
                            entries[1 + renaming] = index_entry(*entry_mode, id);
                        }
                        Some(Change::Deletion { .. }) => entries[0] = base,
                        Some(Change::Rewrite {
                            location,
                            entry_mode,
                            id,
                            ..
                        }) if location.as_bstr() == moved.moved.location() => {
                            entries[0] = base;
                            entries[1 + renaming] = index_entry(*entry_mode, id);
                        }
                        Some(Change::Addition { .. } | Change::Rewrite { .. }) => {}
                    }
                }
                Change::Modification {
                    location,
                    previous_entry_mode,
                    previous_id,
                    ..
                } => {
                    let base = index_entry(*previous_entry_mode, previous_id);
                    entries[0] = base;
                    match at_source.get(location.as_bstr()) {
                        None => entries[1 + renaming] = base,
                        Some(Change::Modification { entry_mode, id, .. }) => {
                            entries[1 + renaming] = index_entry(*entry_mode, id);
                        }
                        Some(Change::Rewrite { .. }) => moved.taken_by_rename = true,
                        Some(Change::Addition { .. } | Change::Deletion { .. }) => {}
                    }
                }
                Change::Addition { location, .. } | Change::Deletion { location, .. } => {
                    if let Some(change) = at_location.get(location.as_bstr()) {
                        let (mode, id) = change.entry_mode_and_id();
                        entries[1 + renaming] = index_entry(mode, id);
                    }
                }
            }
            moved.entries = entries;
        }
    }
}

/// The stage of a file with `mode` and `id` at the location of its conflict.
fn index_entry(mode: EntryMode, id: &gix_hash::oid) -> Option<ConflictIndexEntry> {
    Some(ConflictIndexEntry {
        mode,
        id: id.to_owned(),
        path_hint: None,
    })
}

/// The blobs whose similarity rename detection estimates, read as `git diff` reads them: the `diff` attribute of a
/// regular file and the diff drivers decide whether its data is binary.
struct MergeBlobs<'a, O> {
    objects: &'a O,
    attr_stack: &'a mut gix_worktree::Stack,
    /// The configured diff drivers, sorted by name.
    drivers: &'a [gix_diff::blob::Driver],
    /// The `diff` attribute of the file being read.
    attributes: Attributes,
}

impl<O: gix_object::FindObjectOrHeader> MergeBlobs<'_, O> {
    /// Whether Git takes the blob of the file at `location` with `mode` for binary data before reading it:
    /// `Some(true)` for binary data, `Some(false)` for text and `None` if its bytes decide. The `diff` attribute of a
    /// regular file makes it text and `-diff` binary, and `diff=<name>` takes the `binary` setting of that driver. A
    /// name without a configured driver, a regular file without a `diff` attribute and any other file take the
    /// setting of the `default` driver.
    fn binary(&mut self, location: &BStr, mode: EntryMode) -> Result<Option<bool>> {
        let state = if mode.is_blob() {
            self.attr_stack
                .at_entry(location, None, self.objects)
                .or_raise(|| message!("Could not read the attributes of '{location}'"))?
                .matching_attributes(&mut self.attributes);
            self.attributes
                .iter_selected()
                .next()
                .map_or(StateRef::Unspecified, |attribute| attribute.assignment.state)
        } else {
            StateRef::Unspecified
        };
        let drivers = self.drivers;
        let driver = |name: &BStr| {
            drivers
                .binary_search_by(|driver| driver.name.as_bstr().cmp(name))
                .ok()
                .map(|index| &drivers[index])
        };
        Ok(match state {
            StateRef::Set => Some(false),
            StateRef::Unset => Some(true),
            StateRef::Value(name) => driver(name.as_bstr())
                .or_else(|| driver("default".into()))
                .and_then(|driver| driver.is_binary),
            StateRef::Unspecified => driver("default".into()).and_then(|driver| driver.is_binary),
        })
    }
}

impl<O: gix_object::FindObjectOrHeader> diffcore::Blobs for MergeBlobs<'_, O> {
    fn size(&mut self, entry: &diffcore::Entry<'_>) -> Result<u64> {
        let id = entry.id;
        Ok(self
            .objects
            .try_header(id)
            .or_raise(|| message!("Could not find object header for {id}"))?
            .ok_or_raise(|| message!("An object with id {id} could not be found").not_found())?
            .size)
    }

    fn content<'buf>(
        &mut self,
        entry: &diffcore::Entry<'_>,
        buf: &'buf mut Vec<u8>,
    ) -> Result<(&'buf [u8], diffcore::Content)> {
        let binary = self.binary(entry.location, entry.mode)?;
        let data = self.objects.find_blob(entry.id, buf)?.data;
        let content = match binary {
            Some(true) => diffcore::Content::Binary,
            Some(false) => diffcore::Content::Text,
            None => diffcore::Content::from_data(data),
        };
        Ok((data, content))
    }
}
