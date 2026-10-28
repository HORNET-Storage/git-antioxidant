//! The tracker pairs files among the changes of a diff as Git's diffcore-rename does, with the finder of
//! [`rename`]: renames as `git diff -M` finds them, and renames and copies as `git diff -C` finds them.
//!
//! Before that, unlike Git, it pairs directories that were deleted and added by their identity, along with the files
//! in them, and afterwards a deleted and an added directory if a file was renamed from one to the other without change
//! and kept its directory relative to them.

use std::{
    collections::HashSet,
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
};

use bstr::{BStr, ByteSlice};
use gix_error::{ErrorExt, OptionExt, Result, ResultExt, message};
use gix_filter::{attributes::StateRef, pipeline::convert::ToGitOutcome};
use gix_object::tree::{EntryKind, EntryMode};

use crate::{
    Rewrites,
    blob::{DiffLineStats, ResourceKind, platform::prepare_diff::Operation},
    rewrites::{
        CopySource, Outcome, Tracker,
        rename::{self, MAX_SCORE, copies},
        tracker::visit::SourceKind,
    },
    tree::visit::{Action, ChangeId, Relation},
};

/// The kind of a change.
#[derive(Debug, Copy, Clone, Ord, PartialOrd, PartialEq, Eq)]
pub enum ChangeKind {
    /// The change represents the *deletion* of an item.
    Deletion,
    /// The change represents the *modification* of an item.
    Modification,
    /// The change represents the *addition* of an item.
    Addition,
}

/// A trait providing all functionality to abstract over the concept of a change, as seen by the [`Tracker`].
pub trait Change: Clone {
    /// Return the hash of the object behind this change for identification.
    ///
    /// Note that this is the id of the object as stored in `git`, i.e. it must have gone through workspace
    /// conversions. What matters is that the IDs are comparable.
    fn id(&self) -> &gix_hash::oid;
    /// Return the relation that this change may have with other changes.
    ///
    /// It allows to associate a directory with its children that are added or removed at the same moment.
    /// Note that this is ignored for modifications.
    ///
    /// If rename-tracking should always be on leaf-level, this should be set to `None` consistently.
    /// Note that trees will never be looked up by their `id` as their children are assumed to be passed in
    /// with the respective relationship.
    ///
    /// Also note that the tracker only sees what's given to it, it will not lookup trees or match paths itself.
    fn relation(&self) -> Option<Relation>;
    /// Return the kind of this change.
    fn kind(&self) -> ChangeKind;
    /// Return more information about the kind of entry affected by this change.
    fn entry_mode(&self) -> EntryMode;
    /// Return the id of the change along with its mode.
    fn id_and_entry_mode(&self) -> (&gix_hash::oid, EntryMode);
    /// Return the id and the mode of the entry before the change, which is what a
    /// [modification](ChangeKind::Modification) is the source of a copy with, as Git copies from the old state of a
    /// modified file.
    ///
    /// The default returns [`Self::id_and_entry_mode()`], which suits changes whose id is that of the old state.
    fn previous_id_and_entry_mode(&self) -> (&gix_hash::oid, EntryMode) {
        self.id_and_entry_mode()
    }
}

/// A set of tracked items allows to figure out their relations by figuring out their similarity.
pub(crate) struct Item<T> {
    /// The underlying raw change
    change: T,
    /// That slice into the backing for paths.
    path: Range<usize>,
    /// If true, this item was already emitted, i.e. seen by the caller.
    emitted: bool,
    /// If true, this item is a file of the source tree that didn't change, which was added only as a source of copies,
    /// and that is never emitted itself.
    unchanged: bool,
}

impl<T: Change> Item<T> {
    fn location<'a>(&self, backing: &'a [u8]) -> &'a BStr {
        backing[self.path.clone()].as_ref()
    }

    fn entry_mode_compatible(&self, other: EntryMode) -> bool {
        use EntryKind::*;
        matches!(
            (other.kind(), self.change.entry_mode().kind()),
            (Blob | BlobExecutable, Blob | BlobExecutable) | (Link, Link) | (Tree, Tree) | (Commit, Commit)
        )
    }

    fn is_source_candidate(&self) -> bool {
        !self.emitted && matches!(self.change.kind(), ChangeKind::Deletion)
    }

    fn is_source_for_destination_of(&self, dest_item_mode: EntryMode) -> bool {
        self.entry_mode_compatible(dest_item_mode) && self.is_source_candidate()
    }

    /// The id and mode of this item as the source of a rename or copy: the old state of a modified file.
    fn source_id_and_entry_mode(&self) -> (&gix_hash::oid, EntryMode) {
        if self.change.kind() == ChangeKind::Modification && !self.unchanged {
            self.change.previous_id_and_entry_mode()
        } else {
            self.change.id_and_entry_mode()
        }
    }
}

/// A module with types used in the user-callback in [Tracker::emit()](crate::rewrites::Tracker::emit()).
pub mod visit {
    use bstr::BStr;
    use gix_object::tree::EntryMode;

    use crate::blob::DiffLineStats;

    /// The source of a rewrite, rename or copy.
    #[derive(Debug, Clone, PartialEq, PartialOrd)]
    pub struct Source<'a, T> {
        /// The kind of entry, which for a copy of a modified file is its mode before the modification.
        pub entry_mode: EntryMode,
        /// The hash of the state of the source as seen in the object database, which for a copy of a modified file is
        /// its id before the modification.
        pub id: gix_hash::ObjectId,
        /// Further specify what kind of source this is.
        pub kind: SourceKind,
        /// The repository-relative location of this entry.
        pub location: &'a BStr,
        /// The change that was registered as source.
        pub change: &'a T,
        /// If the source and the destination aren't identical, how many lines would need to change to turn this source
        /// into the destination, along with their similarity as Git estimates it. It's `None` for identical files and
        /// for binary files, which have no lines.
        pub diff: Option<DiffLineStats>,
    }

    /// Further identify the kind of [Source].
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub enum SourceKind {
        /// This is the source of an entry that was renamed, as `source` was renamed to `destination`.
        Rename,
        /// This is the source of a copy, as `source` was copied into `destination`.
        Copy,
    }

    /// A change along with a location.
    #[derive(Debug, Clone)]
    pub struct Destination<'a, T: Clone> {
        /// The change at the given `location`.
        pub change: T,
        /// The repository-relative location of this destination.
        pub location: &'a BStr,
    }
}

/// Lifecycle
impl<T: Change> Tracker<T> {
    /// Create a new instance with `rewrites` configuration.
    pub fn new(rewrites: Rewrites) -> Self {
        Tracker {
            items: vec![],
            path_backing: vec![],
            rewrites,
            child_renames: Default::default(),
        }
    }
}

/// build state and find matches.
impl<T: Change> Tracker<T> {
    /// We may refuse the push if that information isn't needed for what we have to track.
    pub fn try_push_change(&mut self, change: T, location: &BStr) -> Option<T> {
        let change_kind = change.kind();
        if let (None, ChangeKind::Modification) = (self.rewrites.copies, change_kind) {
            return Some(change);
        }

        let entry_kind = change.entry_mode().kind();
        let relation = change
            .relation()
            .filter(|_| matches!(change_kind, ChangeKind::Addition | ChangeKind::Deletion));
        if let (None, EntryKind::Tree) = (relation, entry_kind) {
            return Some(change);
        }

        let start = self.path_backing.len();
        self.path_backing.extend_from_slice(location);
        let path = start..self.path_backing.len();

        self.items.push(Item {
            path,
            change,
            emitted: false,
            unchanged: false,
        });
        None
    }

    /// Can only be called once effectively as it alters its own state to assure each item is only emitted once.
    ///
    /// `cb(destination, source)` is called for each item, either with `Some(source)` if it's
    /// the destination of a copy or rename, or with `None` for source if no relation to other
    /// items in the tracked set exist, which is like saying 'no rename or rewrite or copy' happened.
    /// Note that directories with [relation](Relation) will be emitted if there is a match, along with all their matching
    /// child-items which are similarly bundled as rename. If `cb` returns [`Break`](std::ops::ControlFlow::Break), it
    /// isn't called anymore.
    ///
    /// Files are paired as Git pairs them, with [`rename::detect()`], or with [`rename::copies::detect()`] if copies
    /// are tracked, in which case the source of a copy of a modified file is its state before the modification, and
    /// the modified file is still emitted as such. Directories were paired by their identity before that.
    ///
    /// `objects` is used to access blob data for similarity checks if required and is taken directly from the object database.
    /// The similarity is estimated from the data as Git stores it, read from the object database, or from the
    /// worktree if `diff_cache` has a [worktree root](crate::blob::pipeline::WorktreeRoots) for the side of the file,
    /// in which case it is converted with the worktree filters of `diff_cache`. Its attributes and diff drivers decide
    /// whether data is binary.
    ///
    /// `diff_cache` is a way to retain a cache of resources that are prepared for rapid diffing, and it also controls
    /// the diff-algorithm (provided no user-algorithm is set). It counts the lines that change between the files of
    /// pairs that aren't identical.
    /// Note that we control a few options of `diff_cache` to assure it will ignore external commands.
    /// Note that we do not control how the `diff_cache` converts resources, it's left to the caller to decide
    /// if it should look at what's stored in `git`, or in the working tree, along with all diff-specific conversions.
    ///
    /// `push_source_tree(push_fn: push(change, location))` is a function that is called when the entire tree of the source
    /// should be added as modifications by calling `push` repeatedly to use for perfect copy tracking. Note that `push`
    /// will panic if `change` is not a modification, and it's valid to not call `push` at all. The pushed files are
    /// sources of copies as `git diff --find-copies-harder` takes them, and they are never emitted themselves.
    pub fn emit<PushSourceTreeFn, E>(
        &mut self,
        mut cb: impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
        diff_cache: &mut crate::blob::Platform,
        objects: &impl gix_object::FindObjectOrHeader,
        push_source_tree: PushSourceTreeFn,
    ) -> Result<Outcome>
    where
        PushSourceTreeFn: FnMut(&mut dyn FnMut(T, &BStr)) -> std::result::Result<(), E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.child_renames.clear();
        diff_cache.options.skip_internal_diff_if_external_is_configured = false;

        let mut out = Outcome {
            options: self.rewrites,
            ..Default::default()
        };
        if self.has_work()
            && self
                .match_pairs(&mut cb, &mut out, diff_cache, objects, push_source_tree)?
                .is_break()
        {
            self.items.clear();
            return Ok(out);
        }

        self.items
            .sort_by(|a, b| a.location(&self.path_backing).cmp(b.location(&self.path_backing)));
        for item in self.items.drain(..).filter(|item| !item.emitted) {
            if cb(
                visit::Destination {
                    location: item.location(&self.path_backing),
                    change: item.change,
                },
                None,
            )
            .is_break()
            {
                break;
            }
        }
        Ok(out)
    }
}

impl<T: Change> Tracker<T> {
    /// Whether there may be pairs: an addition, and a deletion or, if copies are tracked, a modification or the source
    /// tree to take sources from.
    fn has_work(&self) -> bool {
        let (mut has_deletion, mut has_modification, mut has_addition) = (false, false, false);
        for item in &self.items {
            match item.change.kind() {
                ChangeKind::Deletion => has_deletion = true,
                ChangeKind::Modification => has_modification = true,
                ChangeKind::Addition => has_addition = true,
            }
        }
        has_addition
            && (has_deletion
                || self.rewrites.copies.is_some_and(|copies| {
                    has_modification || copies.source == CopySource::FromSetOfModifiedFilesAndAllSources
                }))
    }

    /// Pair directories by their identity, then files as Git does, then the directories that renames of files link,
    /// and emit each pair, stopping if `cb` asks to.
    fn match_pairs<PushSourceTreeFn, E>(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
        out: &mut Outcome,
        diff_cache: &mut crate::blob::Platform,
        objects: &impl gix_object::FindObjectOrHeader,
        mut push_source_tree: PushSourceTreeFn,
    ) -> Result<Action>
    where
        PushSourceTreeFn: FnMut(&mut dyn FnMut(T, &BStr)) -> std::result::Result<(), E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        fn is_parent(change: &impl Change) -> bool {
            matches!(change.relation(), Some(Relation::Parent(_)))
        }
        fn is_tree(change: &impl Change) -> bool {
            change.entry_mode().is_tree()
        }

        if self
            .rewrites
            .copies
            .is_some_and(|copies| copies.source == CopySource::FromSetOfModifiedFilesAndAllSources)
        {
            push_source_tree(&mut |change, location| {
                if self.try_push_change(change, location).is_none() {
                    // make sure these aren't viable to be emitted anymore.
                    let item = self.items.last_mut().expect("just pushed");
                    item.emitted = true;
                    item.unchanged = true;
                }
            })
            .or_raise(|| {
                message("Could not obtain exhaustive item set to use as possible sources for copy detection")
            })?;
        }
        self.sort_items_by_id_and_location();

        // Rewrites by directory (without local changes) can be pruned out quickly,
        // by finding only parents, their counterpart, and then all children can be matched by
        // relationship ID.
        if self.match_pairs_by_identity(cb, is_parent).is_break() {
            return Ok(Action::Break(()));
        }
        // The other directories are paired by identity as well, which can link their parents.
        if self.match_pairs_by_identity(cb, is_tree).is_break() {
            return Ok(Action::Break(()));
        }
        if self.match_files(cb, out, diff_cache, objects)?.is_break() {
            return Ok(Action::Break(()));
        }
        Ok(self.match_renamed_directories(cb))
    }

    /// Sort `items` primarily by their id, so identity-based lookups can be found quickly by
    /// partitioning (see `find_match`). Same-id items are then ordered by location, change-kind,
    /// relation and entry-mode - a total order over each item's observable identity - so that
    /// matching is deterministic and independent of the order in which changes were pushed, which
    /// the parallel dirwalk and index-traversal threads leave nondeterministic
    /// (see <https://github.com/GitoxideLabs/gitoxide/issues/1832>). Any items that still compare
    /// equal are identical in every observable field and are thus interchangeable for matching.
    fn sort_items_by_id_and_location(&mut self) {
        self.items.sort_by(|a, b| {
            a.change
                .id()
                .cmp(b.change.id())
                .then_with(|| a.location(&self.path_backing).cmp(b.location(&self.path_backing)))
                .then_with(|| a.change.kind().cmp(&b.change.kind()))
                .then_with(|| a.change.relation().cmp(&b.change.relation()))
                .then_with(|| a.change.entry_mode().cmp(&b.change.entry_mode()))
        });
    }

    /// Pair each unemitted addition for which `filter` holds with an unemitted deletion of the same id, and emit the
    /// pair. Pairs of parents are followed by the pairs of their children, and pairs of children are remembered to
    /// pair their parents later.
    fn match_pairs_by_identity(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
        filter: fn(&T) -> bool,
    ) -> Action {
        let mut dest_ofs = 0;
        let mut num_checks = 0;
        let max_checks = {
            let limit = self.rewrites.limit.saturating_pow(2);
            // There can be trees with a lot of entries and pathological search behaviour, as they can be repeated
            // and then have a lot of similar hashes. This also means we have to search a lot of candidates which
            // can be too slow despite best attempts. So play it save and detect such cases 'roughly' by amount of items.
            if self.items.len() < 100_000 { 0 } else { limit }
        };
        // Items only stop being possible sources while matching, so this is a superset of the sources for all
        // destinations. Destinations then only need to be compared to these instead of to all items, which matters
        // when there are many more destinations than sources.
        let source_candidates: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| item.is_source_candidate().then_some(idx))
            .collect();
        if source_candidates.is_empty() {
            return Action::Continue(());
        }

        while let Some((mut dest_idx, dest)) = self.items[dest_ofs..].iter().enumerate().find_map(|(idx, item)| {
            (!item.emitted && matches!(item.change.kind(), ChangeKind::Addition) && filter(&item.change))
                .then_some((idx, item))
        }) {
            dest_idx += dest_ofs;
            dest_ofs = dest_idx + 1;
            let src = find_match(
                &self.items,
                &source_candidates,
                dest,
                dest_idx,
                &self.path_backing,
                &mut num_checks,
            )
            .map(|(src_idx, src)| {
                let (id, entry_mode) = src.change.id_and_entry_mode();
                let id = id.to_owned();
                let location = src.location(&self.path_backing);
                (
                    visit::Source {
                        entry_mode,
                        id,
                        kind: SourceKind::Rename,
                        location,
                        change: &src.change,
                        diff: None,
                    },
                    src_idx,
                )
            });
            if max_checks != 0 && num_checks > max_checks {
                gix_trace::warn!(
                    "Cancelled rename matching as there were too many iterations ({num_checks} > {max_checks})"
                );
                return Action::Continue(());
            }
            let Some((src, src_idx)) = src else {
                continue;
            };
            let location = dest.location(&self.path_backing);
            let change = dest.change.clone();
            let dest = visit::Destination { change, location };
            let relations = src.change.relation().zip(dest.change.relation());
            let res = cb(dest, Some(src));

            self.items[dest_idx].emitted = true;
            self.items[src_idx].emitted = true;

            if res.is_break() {
                return Action::Break(());
            }

            match relations {
                Some((Relation::Parent(src), Relation::Parent(dst))) => {
                    if self.emit_child_renames_matching_identity(cb, src, dst).is_break() {
                        return Action::Break(());
                    }
                }
                Some((Relation::ChildOfParent(src), Relation::ChildOfParent(dst))) => {
                    self.child_renames
                        .entry((src, dst))
                        .or_default()
                        .push((src_idx, dest_idx));
                }
                _ => {}
            }
        }
        Action::Continue(())
    }

    /// Pair the files among the unemitted additions with the files that they were renamed or copied from, as Git does
    /// with [`rename::detect()`], or [`rename::copies::detect()`] if copies are tracked, and emit each pair in the
    /// order of the locations of the destinations. Identical renames of children are remembered to pair their parents
    /// later.
    fn match_files(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
        out: &mut Outcome,
        diff_cache: &mut crate::blob::Platform,
        objects: &impl gix_object::FindObjectOrHeader,
    ) -> Result<Action> {
        // The pairs of items as `(source, destination, score, is_copy)`.
        let pairs: Vec<(usize, usize, u32, bool)> = {
            let (path_backing, track_empty) = (&self.path_backing, self.rewrites.track_empty);
            let (destinations, destination_entries): (Vec<usize>, Vec<_>) = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| !item.emitted && item.change.kind() == ChangeKind::Addition)
                .filter_map(|(idx, item)| {
                    let entry = file_entry(item, item.change.id_and_entry_mode(), path_backing, track_empty)?;
                    Some((idx, entry))
                })
                .unzip();
            if destinations.is_empty() {
                return Ok(Action::Continue(()));
            }

            let mut sources = Vec::new();
            for (idx, item) in self.items.iter().enumerate() {
                let kind = match item.change.kind() {
                    ChangeKind::Deletion if !item.emitted => copies::SourceKind::Deleted,
                    // A deleted file that a directory took along remains a source of copies.
                    ChangeKind::Deletion if self.rewrites.copies.is_some() => copies::SourceKind::Modified,
                    ChangeKind::Modification if item.unchanged => copies::SourceKind::Unchanged,
                    ChangeKind::Modification => copies::SourceKind::Modified,
                    _ => continue,
                };
                if let Some(entry) = file_entry(item, item.source_id_and_entry_mode(), path_backing, track_empty) {
                    sources.push((idx, entry, kind));
                }
            }
            // A file of the source tree is a source only as unchanged, so it isn't a source twice if it was deleted or
            // modified.
            let changed: HashSet<&BStr> = sources
                .iter()
                .filter(|(_, _, kind)| *kind != copies::SourceKind::Unchanged)
                .map(|(_, entry, _)| entry.location)
                .collect();
            sources
                .retain(|(_, entry, kind)| *kind != copies::SourceKind::Unchanged || !changed.contains(entry.location));

            let mut blobs = Blobs {
                destinations: if diff_cache.filter.roots.is_unset() {
                    HashSet::new()
                } else {
                    destination_entries.iter().map(|entry| entry.location).collect()
                },
                diff_cache: &mut *diff_cache,
                objects,
                attributes: {
                    let mut attributes = gix_filter::attributes::search::Outcome::default();
                    attributes.initialize_with_selection(&Default::default(), Some("diff"));
                    attributes
                },
            };
            match self.rewrites.copies {
                None => {
                    let source_entries: Vec<_> = sources.iter().map(|(_, entry, _)| *entry).collect();
                    let options = rename::Options {
                        min_score: min_score(self.rewrites.percentage),
                        limit: self.rewrites.limit,
                    };
                    let outcome = rename::detect(&source_entries, &destination_entries, &mut blobs, options)?;
                    out.num_similarity_checks += outcome.estimates;
                    out.num_similarity_checks_skipped_for_rename_tracking_due_to_limit = outcome.skipped_estimates;
                    outcome
                        .renames
                        .into_iter()
                        .map(|rename| {
                            (
                                sources[rename.source].0,
                                destinations[rename.destination],
                                rename.score,
                                false,
                            )
                        })
                        .collect()
                }
                Some(copies) => {
                    let copy_sources: Vec<_> = sources
                        .iter()
                        .map(|&(_, entry, kind)| copies::Source { entry, kind })
                        .collect();
                    let options = copies::Options {
                        rename_min_score: min_score(self.rewrites.percentage),
                        copy_min_score: min_score(copies.percentage),
                        limit: self.rewrites.limit,
                    };
                    let outcome = copies::detect(&copy_sources, &destination_entries, &mut blobs, options)?;
                    out.num_similarity_checks += outcome.estimates;
                    out.num_similarity_checks_skipped_for_copy_tracking_due_to_limit = outcome.skipped_estimates;
                    outcome
                        .pairs
                        .into_iter()
                        .map(|pair| {
                            (
                                sources[pair.source].0,
                                destinations[pair.destination],
                                pair.score,
                                pair.copy,
                            )
                        })
                        .collect()
                }
            }
        };

        for (src_idx, dst_idx, score, copy) in pairs {
            let (src_item, dst_item) = (&self.items[src_idx], &self.items[dst_idx]);
            let (src_id, src_mode) = src_item.source_id_and_entry_mode();
            let (dst_id, dst_mode) = dst_item.change.id_and_entry_mode();
            let (src_location, dst_location) = (
                src_item.location(&self.path_backing),
                dst_item.location(&self.path_backing),
            );
            let identical = src_id == dst_id;
            let diff = if identical {
                None
            } else {
                line_stats(
                    diff_cache,
                    objects,
                    (src_id, src_mode, src_location),
                    (dst_id, dst_mode, dst_location),
                    score,
                )?
            };
            let relations = if identical && !copy {
                src_item.change.relation().zip(dst_item.change.relation())
            } else {
                None
            };
            let res = cb(
                visit::Destination {
                    change: dst_item.change.clone(),
                    location: dst_location,
                },
                Some(visit::Source {
                    entry_mode: src_mode,
                    id: src_id.to_owned(),
                    kind: if copy { SourceKind::Copy } else { SourceKind::Rename },
                    location: src_location,
                    change: &src_item.change,
                    diff,
                }),
            );

            // The source of a copy remains, to be emitted as itself if it was modified.
            self.items[dst_idx].emitted = true;
            if !copy {
                self.items[src_idx].emitted = true;
            }
            if res.is_break() {
                return Ok(Action::Break(()));
            }
            if let Some((Relation::ChildOfParent(src), Relation::ChildOfParent(dst))) = relations {
                self.child_renames
                    .entry((src, dst))
                    .or_default()
                    .push((src_idx, dst_idx));
            }
        }
        Ok(Action::Continue(()))
    }

    /// Emit the children of `src_parent_id` and `dst_parent_id` as pairs of exact matches, which are assumed
    /// as `src` and `dst` were an exact match (so all children have to match exactly).
    /// Note that we intentionally do not record them as their parents will be emitted, too.
    fn emit_child_renames_matching_identity(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
        src_parent_id: ChangeId,
        dst_parent_id: ChangeId,
    ) -> Action {
        debug_assert_ne!(
            src_parent_id, dst_parent_id,
            "src and destination directories must be distinct"
        );
        let (mut src_items, mut dst_items) = (Vec::with_capacity(1), Vec::with_capacity(1));
        for item in self.items.iter_mut().filter(|item| !item.emitted) {
            match item.change.relation() {
                Some(Relation::ChildOfParent(id)) if id == src_parent_id => {
                    src_items.push((item.change.id().to_owned(), item));
                }
                Some(Relation::ChildOfParent(id)) if id == dst_parent_id => {
                    dst_items.push((item.change.id().to_owned(), item));
                }
                _ => continue,
            }
        }

        for ((src_id, src_item), (dst_id, dst_item)) in src_items.into_iter().zip(dst_items) {
            // Since the parent items are already identical by ID, we know that the children will also match, we just
            // double-check to still have a chance to be correct in case some of that goes wrong.
            if src_id == dst_id
                && filename(src_item.location(&self.path_backing)) == filename(dst_item.location(&self.path_backing))
            {
                let entry_mode = src_item.change.entry_mode();
                let location = src_item.location(&self.path_backing);
                let src = visit::Source {
                    entry_mode,
                    id: src_id,
                    kind: SourceKind::Rename,
                    location,
                    change: &src_item.change,
                    diff: None,
                };
                let location = dst_item.location(&self.path_backing);
                let change = dst_item.change.clone();
                let dst = visit::Destination { change, location };
                let res = cb(dst, Some(src));

                src_item.emitted = true;
                dst_item.emitted = true;

                if res.is_break() {
                    return res;
                }
            } else {
                gix_trace::warn!(
                    "Children of parents with change-id {src_parent_id} and {dst_parent_id} were not equal, even though their parents claimed to be"
                );
                break;
            }
        }
        Action::Continue(())
    }

    /// Find directories with relation id that haven't been emitted yet and store them for lookup.
    /// Then use the previously stored emitted renames with relation id to learn which directories they 'link'
    /// and emit them, too.
    /// Note that this works whenever top-level directories are renamed because they are always added and deleted,
    /// and we only match those. One rewrite preserving the relative directory path is enough.
    fn match_renamed_directories(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_, T>, Option<visit::Source<'_, T>>) -> Action,
    ) -> Action {
        fn unemitted_directory_matching_relation_id<T: Change>(items: &[Item<T>], child_id: ChangeId) -> Option<usize> {
            items.iter().position(|i| {
                !i.emitted && matches!(i.change.relation(), Some(Relation::Parent(pid)) if pid == child_id)
            })
        }
        for ((deleted_child_id, added_child_id), children) in &self.child_renames {
            let Some(src_idx) = unemitted_directory_matching_relation_id(&self.items, *deleted_child_id) else {
                continue;
            };
            let Some(dst_idx) = unemitted_directory_matching_relation_id(&self.items, *added_child_id) else {
                // This could go wrong in case there are mismatches, so be defensive here.
                // But generally, we'd expect the destination item to exist.
                continue;
            };

            let (src_item, dst_item) = (&self.items[src_idx], &self.items[dst_idx]);
            if !children.iter().any(|&(src_child_idx, dst_child_idx)| {
                let relative_directory = |child_idx: usize, parent: &Item<T>| {
                    let child = &self.items[child_idx];
                    child
                        .location(&self.path_backing)
                        .strip_prefix(parent.location(&self.path_backing).as_bytes())
                        .and_then(|path| path.strip_prefix(b"/"))
                        .map(|path| {
                            if child.change.entry_mode().is_tree() {
                                path
                            } else {
                                // File names may change without renaming their containing directory.
                                &path[..path.rfind_byte(b'/').unwrap_or(0)]
                            }
                        })
                };
                let src_relative = relative_directory(src_child_idx, src_item);
                let dst_relative = relative_directory(dst_child_idx, dst_item);
                src_relative.is_some() && src_relative == dst_relative
            }) {
                continue;
            }
            let entry_mode = src_item.change.entry_mode();
            let location = src_item.location(&self.path_backing);
            let src = visit::Source {
                entry_mode,
                id: src_item.change.id().to_owned(),
                kind: SourceKind::Rename,
                location,
                change: &src_item.change,
                diff: None,
            };
            let location = dst_item.location(&self.path_backing);
            let change = dst_item.change.clone();
            let dst = visit::Destination { change, location };
            let res = cb(dst, Some(src));

            self.items[src_idx].emitted = true;
            self.items[dst_idx].emitted = true;

            if res.is_break() {
                return res;
            }
        }
        Action::Continue(())
    }
}

fn filename(path: &BStr) -> &BStr {
    path.rfind_byte(b'/').map_or(path, |idx| path[idx + 1..].as_bstr())
}

/// The file of `item` with `id` and `mode` as the finder takes it, or `None` if it is no file to pair: like Git, empty
/// files are left out unless `track_empty` is set.
fn file_entry<'a, T: Change>(
    item: &Item<T>,
    (id, mode): (&'a gix_hash::oid, EntryMode),
    path_backing: &'a [u8],
    track_empty: bool,
) -> Option<rename::Entry<'a>> {
    (!mode.is_tree() && (track_empty || !id.is_empty_blob())).then(|| rename::Entry {
        location: item.location(path_backing),
        mode,
        id,
    })
}

/// The similarity, out of [`MAX_SCORE`], that the finder needs for `percentage`: identical files only for `None`, and
/// a share of [`MAX_SCORE`] otherwise, of which 0 stands for 50% as `-M0%` does in Git.
fn min_score(percentage: Option<f32>) -> u32 {
    percentage.map_or(MAX_SCORE, |percentage| {
        (percentage.clamp(0.0, 1.0) * MAX_SCORE as f32).round() as u32
    })
}

/// Find `item` in our set of items by identity, ignoring `item_idx` to avoid finding ourselves, among the
/// `source_candidates` that weren't emitted yet and whose mode is compatible. Like Git, a source with the file name of
/// `item` is preferred, and otherwise the first is taken.
fn find_match<'a, T: Change>(
    items: &'a [Item<T>],
    source_candidates: &[usize],
    item: &Item<T>,
    item_idx: usize,
    path_backing: &[u8],
    num_checks: &mut usize,
) -> Option<(usize, &'a Item<T>)> {
    let (item_id, item_mode) = item.change.id_and_entry_mode();
    // Items are sorted by id, so the candidates with the same id are a contiguous range of `source_candidates`.
    let start = source_candidates.partition_point(|&idx| items[idx].change.id() < item_id);
    let end = source_candidates.partition_point(|&idx| items[idx].change.id() <= item_id);
    let item_name = filename(item.location(path_backing));
    let mut fallback = None;
    for &src_idx in &source_candidates[start..end] {
        let src = &items[src_idx];
        *num_checks += 1;
        if src_idx == item_idx || !src.is_source_for_destination_of(item_mode) {
            continue;
        }
        // Like Git, prefer a source whose file name matches the destination's to keep
        // renames of equally-named files together when contents are identical.
        if filename(src.location(path_backing)) == item_name {
            return Some((src_idx, src));
        }
        fallback.get_or_insert((src_idx, src));
    }
    fallback
}

/// Count the lines that change between the files of a pair that aren't identical, `source` and `destination`, each
/// given as its id, mode and location, whose `score` Git estimated. Binary files have no lines to count.
fn line_stats(
    diff_cache: &mut crate::blob::Platform,
    objects: &impl gix_object::FindObjectOrHeader,
    (source_id, source_mode, source_location): (&gix_hash::oid, EntryMode, &BStr),
    (destination_id, destination_mode, destination_location): (&gix_hash::oid, EntryMode, &BStr),
    score: u32,
) -> Result<Option<DiffLineStats>> {
    diff_cache
        .set_resource(
            destination_id.to_owned(),
            destination_mode.kind(),
            destination_location,
            ResourceKind::NewOrDestination,
            objects,
        )
        .or_raise(|| message("Could not set destination for similarity checking"))?;
    diff_cache
        .set_resource(
            source_id.to_owned(),
            source_mode.kind(),
            source_location,
            ResourceKind::OldOrSource,
            objects,
        )
        .or_raise(|| message("Could not set source for similarity checking"))?;
    let prep = diff_cache
        .prepare_diff()
        .or_raise(|| message("Could not prepare resources for similarity checking"))?;
    Ok(match prep.operation {
        Operation::InternalDiff { algorithm } => {
            let tokens = crate::blob::InternedInput::new(prep.old.intern_source(), prep.new.intern_source());
            let diff = crate::blob::Diff::compute(algorithm, &tokens);
            Some(DiffLineStats {
                removals: diff.count_removals(),
                insertions: diff.count_additions(),
                before: tokens.before.len(),
                after: tokens.after.len(),
                similarity: score as f32 / MAX_SCORE as f32,
            })
        }
        Operation::ExternalCommand { .. } => {
            unreachable!("we have disabled this possibility with an option")
        }
        Operation::SourceOrDestinationIsBinary => None,
    })
}

/// The blobs of the files whose similarity the finder estimates, read as Git reads them for its estimate: the data as
/// Git stores it, from the object database, or from the worktree where `diff_cache` has a worktree root for the side
/// of a file, which is then converted with its worktree filters. Its attributes and diff drivers decide whether data
/// is binary. Like [`Pipeline::convert_to_diffable()`](crate::blob::Pipeline::convert_to_diffable()), a file with a
/// null id and no worktree root has no data.
struct Blobs<'a, O> {
    diff_cache: &'a mut crate::blob::Platform,
    objects: &'a O,
    /// The locations of the destinations, which are read from the worktree root of new files, if there are roots.
    destinations: HashSet<&'a BStr>,
    /// The `diff` attribute of the file being read.
    attributes: gix_filter::attributes::search::Outcome,
}

impl<O: gix_object::FindObjectOrHeader> Blobs<'_, O> {
    /// The worktree root to read the file of `entry` from, or `None` to read its blob from the object database.
    fn root(&self, entry: &rename::Entry<'_>) -> Option<PathBuf> {
        let kind = if self.destinations.contains(entry.location) {
            ResourceKind::NewOrDestination
        } else {
            ResourceKind::OldOrSource
        };
        self.diff_cache.filter.roots.by_kind(kind).map(ToOwned::to_owned)
    }

    /// Read the file of `entry` in the worktree at `root` into `buf` as Git would store it, or nothing if it is gone.
    fn read_worktree_file(&mut self, root: &Path, entry: &rename::Entry<'_>, buf: &mut Vec<u8>) -> Result<()> {
        buf.clear();
        let rela_path = gix_path::from_bstr(entry.location)?;
        let path = root.join(&rela_path);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(err.and_raise(message!(
                    "Could not open worktree file \"{}\" for reading",
                    path.display()
                )));
            }
        };
        let diff_cache = &mut *self.diff_cache;
        let attributes = diff_cache
            .attr_stack
            .at_entry(entry.location, None, self.objects)
            .or_raise(|| message!("Could not read the attributes of '{}'", entry.location))?;
        let converted = diff_cache
            .filter
            .worktree_filter
            .convert_to_git(
                file,
                &rela_path,
                &mut |_, out| {
                    attributes.matching_attributes(out);
                },
                &mut |_| Ok(None),
            )
            .or_raise(|| message!("Could not convert worktree file \"{}\" to Git format", path.display()))?;
        let read = match converted {
            ToGitOutcome::Unchanged(mut file) => file.read_to_end(buf).map(|_| ()),
            ToGitOutcome::Process(mut stream) => stream.read_to_end(buf).map(|_| ()),
            ToGitOutcome::Buffer(data) => {
                buf.extend_from_slice(data);
                Ok(())
            }
        };
        read.or_raise(|| message!("Could not read worktree file \"{}\"", path.display()))
    }

    /// Whether Git takes the blob of `entry` for binary data before reading it: `Some(true)` for binary data,
    /// `Some(false)` for text and `None` if its bytes decide. The `diff` attribute of a regular file makes it text and
    /// `-diff` binary, and `diff=<name>` takes the `binary` setting of that driver. A name without a configured driver,
    /// a regular file without a `diff` attribute and any other file take the setting of the `default` driver.
    fn binary(&mut self, entry: &rename::Entry<'_>) -> Result<Option<bool>> {
        let state = if entry.mode.is_blob() {
            self.diff_cache
                .attr_stack
                .at_entry(entry.location, None, self.objects)
                .or_raise(|| message!("Could not read the attributes of '{}'", entry.location))?
                .matching_attributes(&mut self.attributes);
            self.attributes
                .iter_selected()
                .next()
                .map_or(StateRef::Unspecified, |attribute| attribute.assignment.state)
        } else {
            StateRef::Unspecified
        };
        let drivers = self.diff_cache.filter.drivers();
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

impl<O: gix_object::FindObjectOrHeader> rename::Blobs for Blobs<'_, O> {
    fn size(&mut self, entry: &rename::Entry<'_>) -> Result<u64> {
        if let Some(root) = self.root(entry) {
            let mut buf = Vec::new();
            self.read_worktree_file(&root, entry, &mut buf)?;
            return Ok(buf.len() as u64);
        }
        let id = entry.id;
        if id.is_null() {
            return Ok(0);
        }
        Ok(self
            .objects
            .try_header(id)
            .or_raise(|| message!("Could not find object header for {id}"))?
            .ok_or_raise(|| message!("An object with id {id} could not be found").not_found())?
            .size)
    }

    fn content<'buf>(
        &mut self,
        entry: &rename::Entry<'_>,
        buf: &'buf mut Vec<u8>,
    ) -> Result<(&'buf [u8], rename::Content)> {
        let binary = self.binary(entry)?;
        let data = if let Some(root) = self.root(entry) {
            self.read_worktree_file(&root, entry, buf)?;
            buf.as_slice()
        } else if entry.id.is_null() {
            buf.clear();
            buf.as_slice()
        } else {
            let id = entry.id;
            self.objects
                .try_find(id, buf)
                .or_raise(|| message!("Could not find object {id}"))?
                .ok_or_raise(|| message!("An object with id {id} could not be found").not_found())?
                .data
        };
        let content = match binary {
            Some(true) => rename::Content::Binary,
            Some(false) => rename::Content::Text,
            None => rename::Content::from_data(data),
        };
        Ok((data, content))
    }
}
