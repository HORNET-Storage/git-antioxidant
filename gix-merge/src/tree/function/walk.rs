//! The walk of the merge-base and both sides that Git's merge-ort performs before it detects renames, in its
//! `collect_merge_info()`.
//!
//! The three trees are visited together, directory by directory and in the order of Git's `traverse_trees()`. For each
//! side the walk records the files that the side deleted or added, in the order in which rename detection considers
//! them, the deleted files whose new location rename detection has to find, and the directories that the side removed.
//! A directory that only one side changed is visited only after the main walk, and only if rename detection on that
//! side needs its files, which also decides the order in which its files are recorded. The walk also records what is
//! known about each visited path before renames are processed, which the application of directory renames consults.

use std::{cmp::Ordering, collections::HashMap};

use bstr::{BStr, BString, ByteSlice};
use gix_diffcore::{
    rename::merge::{DirectoryRelevance, SourceRelevance},
    strmap,
};
use gix_error::Result;
use gix_hash::ObjectId;
use gix_object::{FindExt, tree};

/// The mask of all three versions of a path. The masks of merge-ort give the merge-base the bit 1, ours the bit 2 and
/// theirs the bit 4.
const ALL: u8 = 0b111;

/// A file that one side of the merge deleted or added, as merge-ort queues it for rename detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pair {
    /// The path of the file.
    pub location: BString,
    /// The mode of the file in the merge-base if the side deleted it, or on the side if the side added it.
    pub mode: tree::EntryMode,
    /// The id of the file in the merge-base if the side deleted it, or on the side if the side added it.
    pub id: ObjectId,
    /// Whether the side added the file rather than deleted it.
    pub is_addition: bool,
}

/// What the walk recorded for one side of the merge.
#[derive(Debug, Default)]
pub(super) struct Side {
    /// The files that the side deleted or added, in the order of merge-ort's queue of them.
    pub pairs: Vec<Pair>,
    /// The deleted files whose new location rename detection has to find, with the reason.
    pub relevant_sources: HashMap<BString, SourceRelevance>,
    /// The directories that the side removed, with how much their new location matters. Below a removed directory
    /// whose location matters, a directory to which the other side added a file is [`DirectoryRelevance::ForSelf`]
    /// even if the side kept it.
    pub dirs_removed: HashMap<BString, DirectoryRelevance>,
}

/// What the merge knows about a path before it processes renames, as merge-ort keeps it in its `paths` map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PathFacts {
    /// Whether the path is merged already: its three versions are equal, or the sides have the same file, or one side
    /// has the file of the merge-base, or a directory that only one side changed was merged in favor of that side.
    pub clean: bool,
    /// The bits of the versions that are files, symbolic links or submodules at the path.
    pub filemask: u8,
    /// The bits of the versions that are directories at the path.
    pub dirmask: u8,
    /// The bits of versions that are equal: 7 if all three are, 3 or 5 if ours or theirs is the merge-base's version,
    /// 6 if the sides are equal and 0 otherwise. It is limited to `filemask` once the entries of a directory are
    /// visited, and cleared once a directory is merged in favor of one side.
    pub match_mask: u8,
    /// Whether a directory and a non-directory meet at the path.
    pub df_conflict: bool,
}

/// The result of [`walk()`].
#[derive(Debug, Default)]
pub(super) struct Walk {
    /// What was recorded for ours, at index 0, and for theirs, at index 1.
    pub sides: [Side; 2],
    /// Every visited path.
    pub paths: HashMap<BString, PathFacts>,
}

/// Walk the trees of the merge-base, `base`, and of the sides, `ours` and `theirs`, as merge-ort's
/// `collect_merge_info()` does, reading the trees from `objects`.
pub(super) fn walk(
    base: &gix_hash::oid,
    ours: &gix_hash::oid,
    theirs: &gix_hash::oid,
    objects: &dyn gix_object::Find,
) -> Result<Walk> {
    let mut walker = Walker {
        objects,
        buf: Vec::new(),
        walk: Walk::default(),
        deferred: Default::default(),
        trivial_merges_okay: [true; 2],
    };
    let root = walker.directory(
        BString::default(),
        0,
        [Some(base.to_owned()), Some(ours.to_owned()), Some(theirs.to_owned())],
    )?;
    walker.visit(root)?;
    walker.visit_deferred()?;
    Ok(walker.walk)
}

/// The state of [`walk()`].
struct Walker<'a> {
    objects: &'a dyn gix_object::Find,
    buf: Vec<u8>,
    walk: Walk,
    /// The directories that were skipped for ours and theirs, in the order in which they were skipped, as merge-ort's
    /// `possible_trivial_merges` holds them.
    deferred: [Vec<Deferred>; 2],
    /// Whether directories may be skipped for ours and theirs, merge-ort's `trivial_merges_okay`.
    trivial_merges_okay: [bool; 2],
}

/// A directory that only one side changed, skipped by the main walk.
struct Deferred {
    path: BString,
    /// The trees of the versions that are directories at `path`.
    trees: [Option<ObjectId>; 3],
    /// merge-ort's `dir_rename_mask` when the directory was skipped.
    dir_rename_mask: u8,
}

/// A directory whose entries are being visited.
struct Directory {
    /// The path of the directory, which is empty for the root.
    path: BString,
    /// merge-ort's `dir_rename_mask` while the entries of the directory are visited: 0, the bit of the side that kept
    /// a directory that the other side removed, or 7 where the location of such a directory matters.
    dir_rename_mask: u8,
    /// The entries of the merge-base, ours and theirs, each empty if the version at the path isn't a directory.
    entries: [Vec<tree::Entry>; 3],
    /// The entries that are visited together, as indices into `entries`, in the order of the visit.
    groups: Vec<[Option<usize>; 3]>,
    /// The index of the next group to visit.
    next: usize,
}

/// The masks of a visited path.
#[derive(Clone, Copy)]
struct Masks {
    filemask: u8,
    dirmask: u8,
    match_mask: u8,
}

impl Walker<'_> {
    /// Read the directory at `path` from `trees`, those of the merge-base, ours and theirs where they have a directory,
    /// to visit its entries with `dir_rename_mask`.
    fn directory(&mut self, path: BString, dir_rename_mask: u8, trees: [Option<ObjectId>; 3]) -> Result<Directory> {
        let mut entries: [Vec<tree::Entry>; 3] = Default::default();
        for (entries, id) in entries.iter_mut().zip(trees) {
            if let Some(id) = id {
                *entries = self.objects.find_tree(&id, &mut self.buf)?.into_owned().entries;
            }
        }
        let groups = groups(entries.each_ref().map(Vec::as_slice));
        // Where only the side that kept a removed directory has a file, the location of the directory matters for all
        // of its entries, as merge-ort learns by reading them all first (`traverse_trees_wrapper()`).
        let dir_rename_mask = if matches!(dir_rename_mask, 2 | 4)
            && groups.iter().any(|group| {
                let (mask, dirmask) = masks(&versions(&entries, group));
                (mask & !dirmask) == dir_rename_mask
            }) {
            ALL
        } else {
            dir_rename_mask
        };
        Ok(Directory {
            path,
            dir_rename_mask,
            entries,
            groups,
            next: 0,
        })
    }

    /// Visit the entries of `directory` and of the directories below it, depth-first as merge-ort recurses into them.
    fn visit(&mut self, directory: Directory) -> Result<()> {
        let mut stack = vec![directory];
        while let Some(directory) = stack.last_mut() {
            let Some(group) = directory.groups.get(directory.next).copied() else {
                stack.pop();
                continue;
            };
            directory.next += 1;
            if let Some(below) = self.visit_entry(directory, group)? {
                stack.push(below);
            }
        }
        Ok(())
    }

    /// Visit the entries of `group` in `directory`, which have the same name, as merge-ort's
    /// `collect_merge_info_callback()` does, and return the directory at their path if its entries are visited next.
    fn visit_entry(&mut self, directory: &Directory, group: [Option<usize>; 3]) -> Result<Option<Directory>> {
        let versions = versions(&directory.entries, &group);
        let (mask, dirmask) = masks(&versions);
        let filemask = mask & !dirmask;
        let equal = |a: usize, b: usize| matches!((versions[a], versions[b]), (Some(x), Some(y)) if x.mode.kind() == y.mode.kind() && x.oid == y.oid);
        let (ours_is_base, theirs_is_base, sides_are_equal) = (equal(0, 1), equal(0, 2), equal(1, 2));
        let match_mask = match (ours_is_base, theirs_is_base, sides_are_equal) {
            (true, true, _) => ALL,
            (true, false, _) => 3,
            (false, true, _) => 5,
            (false, false, true) => 6,
            (false, false, false) => 0,
        };
        let name = versions
            .iter()
            .flatten()
            .next()
            .expect("a group has an entry")
            .filename
            .as_bstr();
        let path = join(directory.path.as_bstr(), name);

        if (ours_is_base && theirs_is_base) || (filemask == ALL && (sides_are_equal || ours_is_base || theirs_is_base))
        {
            self.walk.paths.insert(
                path,
                PathFacts {
                    clean: true,
                    filemask,
                    dirmask,
                    match_mask,
                    df_conflict: false,
                },
            );
            return Ok(None);
        }

        let mut dir_rename_mask = directory.dir_rename_mask;
        self.collect_rename_info(
            &versions,
            directory.path.as_bstr(),
            path.as_bstr(),
            Masks {
                filemask,
                dirmask,
                match_mask,
            },
            &mut dir_rename_mask,
        );
        let mut facts = PathFacts {
            clean: false,
            filemask,
            dirmask,
            match_mask,
            df_conflict: filemask != 0 && dirmask != 0,
        };
        if dirmask == 0 {
            self.walk.paths.insert(path, facts);
            return Ok(None);
        }

        // The side whose directory may hold additions, if the other side kept the merge-base's directory or neither
        // the merge-base nor the other side has one.
        let mut side = match (ours_is_base, theirs_is_base) {
            (true, _) => 2,
            (false, true) => 1,
            (false, false) => 0,
        };
        if filemask == 0 && matches!(dirmask, 2 | 4) {
            facts.match_mask = ALL - dirmask;
            side = usize::from(dirmask / 2);
        }
        let trees = versions.map(|version| version.filter(|entry| entry.mode.is_tree()).map(|entry| entry.oid));
        if dir_rename_mask != ALL && side != 0 && self.trivial_merges_okay[side - 1] {
            self.walk.paths.insert(path.clone(), facts);
            self.deferred[side - 1].push(Deferred {
                path,
                trees,
                dir_rename_mask,
            });
            return Ok(None);
        }
        facts.match_mask &= filemask;
        self.walk.paths.insert(path.clone(), facts);
        self.directory(path, dir_rename_mask, trees).map(Some)
    }

    /// Record the deletions and additions at `path` in `directory`, whose `versions` have the given masks, and the
    /// directories that a side removed, updating `dir_rename_mask` for the entries of a directory at `path`, as
    /// merge-ort's `collect_rename_info()` and `add_pair()` do.
    fn collect_rename_info(
        &mut self,
        versions: &[Option<&tree::Entry>; 3],
        directory: &BStr,
        path: &BStr,
        masks: Masks,
        dir_rename_mask: &mut u8,
    ) {
        let Masks {
            filemask,
            dirmask,
            match_mask,
        } = masks;
        if *dir_rename_mask != ALL && matches!(dirmask, 3 | 5) {
            *dir_rename_mask = dirmask & !1;
        }
        if matches!(dirmask, 1 | 3 | 5) {
            let relevance = if *dir_rename_mask == ALL {
                DirectoryRelevance::ForAncestor
            } else {
                DirectoryRelevance::NotRelevant
            };
            for side in [1, 2] {
                if (dirmask & (1 << side)) == 0 {
                    self.walk.sides[side - 1]
                        .dirs_removed
                        .insert(path.to_owned(), relevance);
                }
            }
        }
        if *dir_rename_mask == ALL && matches!(filemask, 2 | 4) {
            let other_side = 3 - usize::from(filemask / 2);
            self.walk.sides[other_side - 1]
                .dirs_removed
                .insert(directory.to_owned(), DirectoryRelevance::ForSelf);
        }
        if filemask == 0 || filemask == ALL {
            return;
        }

        for side in [1, 2] {
            let side_bit = 1 << side;
            let (version, is_addition) = match ((filemask & 1) != 0, (filemask & side_bit) != 0) {
                (true, false) => (versions[0], false),
                (false, true) => (versions[side], true),
                _ => continue,
            };
            let version = version.expect("the masks have a bit for each present version");
            let recorded = &mut self.walk.sides[side - 1];
            if !is_addition {
                let relevance = if (match_mask & filemask) == 0 {
                    Some(SourceRelevance::Content)
                } else if *dir_rename_mask == ALL {
                    Some(SourceRelevance::Location)
                } else {
                    None
                };
                if let Some(relevance) = relevance {
                    recorded.relevant_sources.insert(path.to_owned(), relevance);
                }
            }
            recorded.pairs.push(Pair {
                location: path.to_owned(),
                mode: version.mode,
                id: version.oid,
                is_addition,
            });
        }
    }

    /// Visit the directories that the main walk skipped, as merge-ort's `handle_deferred_entries()` does, ours first.
    /// If rename detection on a side has no relevant source, it needs none of their files, and each of them is merged in
    /// favor of that side. Otherwise, each is visited in the order in which Git's hash map of them lists their paths.
    fn visit_deferred(&mut self) -> Result<()> {
        for side in [1, 2] {
            let trivial = self.walk.sides[side - 1].relevant_sources.is_empty();
            self.trivial_merges_okay[side - 1] = trivial;
            let deferred = std::mem::take(&mut self.deferred[side - 1]);
            for index in strmap::iteration_order(deferred.iter().map(|deferred| deferred.path.as_slice())) {
                let Deferred {
                    path,
                    trees,
                    dir_rename_mask,
                } = &deferred[index];
                if trivial {
                    self.merge_trivially(path.as_bstr(), side);
                    continue;
                }
                let facts = self
                    .walk
                    .paths
                    .get_mut(path.as_bstr())
                    .expect("skipped directories are recorded");
                facts.match_mask &= facts.filemask;
                let directory = self.directory(path.clone(), *dir_rename_mask, *trees)?;
                self.visit(directory)?;
            }
            // Directories are skipped again only while directories may be skipped for this side.
            for Deferred { path, .. } in std::mem::take(&mut self.deferred[side - 1]) {
                self.merge_trivially(path.as_bstr(), side);
            }
        }
        Ok(())
    }

    /// Merge the skipped directory at `path` in favor of `side`, as merge-ort's `resolve_trivial_directory_merge()`
    /// does.
    fn merge_trivially(&mut self, path: &BStr, side: usize) {
        let facts = self.walk.paths.get_mut(path).expect("skipped directories are recorded");
        debug_assert_eq!(
            facts.match_mask,
            if side == 1 { 5 } else { 3 },
            "the other side has the merge-base's version"
        );
        facts.dirmask &= !facts.match_mask;
        facts.filemask &= !facts.match_mask;
        facts.match_mask = 0;
        facts.clean = !facts.df_conflict || facts.dirmask != 0;
        facts.df_conflict = false;
    }
}

/// The entries of `group` in `entries`, those of the merge-base, ours and theirs.
fn versions<'a>(entries: &'a [Vec<tree::Entry>; 3], group: &[Option<usize>; 3]) -> [Option<&'a tree::Entry>; 3] {
    std::array::from_fn(|version| group[version].map(|index| &entries[version][index]))
}

/// The bits of the present `versions` and of those that are directories.
fn masks(versions: &[Option<&tree::Entry>; 3]) -> (u8, u8) {
    let (mut mask, mut dirmask) = (0, 0);
    for (bit, version) in versions.iter().enumerate() {
        if let Some(version) = version {
            mask |= 1 << bit;
            if version.mode.is_tree() {
                dirmask |= 1 << bit;
            }
        }
    }
    (mask, dirmask)
}

/// The path of the entry `name` in the directory at `directory`.
fn join(directory: &BStr, name: &BStr) -> BString {
    let mut path = Vec::with_capacity(directory.len() + 1 + name.len());
    if !directory.is_empty() {
        path.extend_from_slice(directory);
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path.into()
}

/// The entries of `trees`, those of the merge-base, ours and theirs, that are visited together, as indices into each,
/// in the order in which Git's `traverse_trees()` presents them.
///
/// Each step takes the smallest name among the first entries not yet visited, and then the entry of that name from each
/// tree. As Git sorts a directory of a tree as if its name ended in `/`, a directory `t` can follow `t-1` and `t.c`,
/// so the search continues past entries that start with the name if the byte that follows it sorts before `/`. An
/// entry visited before the entries in front of it is skipped once these were visited.
fn groups(trees: [&[tree::Entry]; 3]) -> Vec<[Option<usize>; 3]> {
    let mut cursors: [Cursor; 3] = Default::default();
    let mut groups = Vec::new();
    loop {
        let mut first: Option<&BStr> = None;
        for (cursor, entries) in cursors.iter_mut().zip(trees) {
            if let Some(head) = cursor.head(entries) {
                let name = entries[head].filename.as_bstr();
                if first.is_none_or(|first| name < first) {
                    first = Some(name);
                }
            }
        }
        let Some(first) = first else {
            break;
        };
        let group: [Option<usize>; 3] = std::array::from_fn(|tree| cursors[tree].find(trees[tree], first));
        for (cursor, index) in cursors.iter_mut().zip(group) {
            if let Some(index) = index {
                cursor.visited(index);
            }
        }
        groups.push(group);
    }
    groups
}

/// The position of [`groups()`] in one tree.
#[derive(Default)]
struct Cursor {
    /// The first entry that may not have been visited yet.
    head: usize,
    /// The entries after `head` that were visited, which `head` passes once it reaches them.
    visited_early: Vec<usize>,
}

impl Cursor {
    /// The first entry of `entries` that wasn't visited, if there is one.
    fn head(&mut self, entries: &[tree::Entry]) -> Option<usize> {
        while self.visited_early.contains(&self.head) {
            self.head += 1;
        }
        (self.head < entries.len()).then_some(self.head)
    }

    /// The entry of `entries` named `name`, searched from the head as Git's `extended_entry_extract()` does.
    fn find(&self, entries: &[tree::Entry], name: &BStr) -> Option<usize> {
        for (index, entry) in entries.iter().enumerate().skip(self.head) {
            match name.cmp(entry.filename.as_bstr()) {
                Ordering::Equal => return Some(index),
                Ordering::Greater => {}
                Ordering::Less if entry.filename.starts_with(name) && entry.filename[name.len()] < b'/' => {}
                Ordering::Less => return None,
            }
        }
        None
    }

    /// Remember that the entry at `index` was visited.
    fn visited(&mut self, index: usize) {
        if index == self.head {
            self.head += 1;
        } else {
            self.visited_early.push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use gix_object::tree::{self, EntryKind};

    use super::groups;

    fn entries(names: &[(&str, EntryKind)]) -> Vec<tree::Entry> {
        names
            .iter()
            .map(|&(name, kind)| tree::Entry {
                mode: kind.into(),
                filename: name.into(),
                oid: gix_hash::Kind::Sha1.null(),
            })
            .collect()
    }

    #[test]
    fn a_directory_behind_entries_that_start_with_its_name_is_visited_with_a_file_of_that_name() {
        // The example of Git's `extended_entry_extract()`.
        let base = entries(&[
            ("t-1", EntryKind::Blob),
            ("t-2", EntryKind::Blob),
            ("t", EntryKind::Tree),
            ("t=1", EntryKind::Blob),
        ]);
        let ours = entries(&[("t", EntryKind::Blob)]);
        assert_eq!(
            groups([base.as_slice(), ours.as_slice(), &[]]),
            [
                [Some(2), Some(0), None],
                [Some(0), None, None],
                [Some(1), None, None],
                [Some(3), None, None],
            ]
        );
    }

    #[test]
    fn names_that_no_entry_starts_with_are_visited_in_the_order_of_their_bytes() {
        let base = entries(&[("a", EntryKind::Blob), ("c", EntryKind::Tree)]);
        let ours = entries(&[("b", EntryKind::Blob), ("c", EntryKind::Tree)]);
        let theirs = entries(&[("a", EntryKind::Blob), ("d", EntryKind::Blob)]);
        assert_eq!(
            groups([base.as_slice(), ours.as_slice(), theirs.as_slice()]),
            [
                [Some(0), None, Some(0)],
                [None, Some(0), None],
                [Some(1), Some(1), None],
                [None, None, Some(1)],
            ]
        );
    }

    #[test]
    fn a_directory_is_found_in_one_tree_while_the_other_has_entries_before_it() {
        let base = entries(&[("t-1", EntryKind::Blob), ("t", EntryKind::Tree)]);
        let ours = entries(&[("t", EntryKind::Tree)]);
        let theirs = entries(&[("t", EntryKind::Blob), ("t-1", EntryKind::Blob)]);
        assert_eq!(
            groups([base.as_slice(), ours.as_slice(), theirs.as_slice()]),
            [[Some(1), Some(0), Some(0)], [Some(0), None, Some(1)]]
        );
    }
}
