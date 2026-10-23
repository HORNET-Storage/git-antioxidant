use bstr::BString;
use gix_diff::{Rewrites, tree_with_rewrites::Change};
use gix_error::ClassificationMarker;

/// A recoverable failure of [`tree()`](crate::tree()).
/// Other failures retain their original causes in the returned exception.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The selected binary merge resource is absent, such as the ancestor of an add/add conflict outside a merge of
    /// [merge-bases](crate::blob::platform::merge::Options::is_virtual_ancestor), which reads it as empty content.
    /// Classified as [`gix_error::Class::NotFound`].
    MissingBinaryMergeResult,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBinaryMergeResult => f.write_str(
                "The merge was performed, but the binary merge result couldn't be selected as it wasn't found",
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingBinaryMergeResult => Some(const { &ClassificationMarker::NOT_FOUND }),
        }
    }
}

/// The outcome produced by [`tree()`](crate::tree()).
#[derive(Clone)]
pub struct Outcome<'a> {
    /// The ready-made (but unwritten) *base* tree, including all non-conflicting changes, and the changes that had
    /// conflicts which could be resolved automatically.
    ///
    /// This means, if all of their changes were conflicting, this will be equivalent to the *base* tree.
    pub tree: gix_object::tree::Editor<'a>,
    /// The set of conflicts we encountered. Can be empty to indicate there was no conflict.
    /// Note that conflicts might have been auto-resolved, but they are listed here for completeness.
    /// Use [`has_unresolved_conflicts()`](Outcome::has_unresolved_conflicts()) to see if any action is needed
    /// before using [`tree`](Outcome::tree).
    pub conflicts: Vec<Conflict>,
    /// `true` if `conflicts` contains only a single [*unresolved* conflict](ResolutionFailure) in the last slot, but
    /// possibly more [resolved ones](Resolution) before that.
    /// This also makes this outcome a very partial merge that cannot be completed.
    /// Only set if [`fail_on_conflict`](Options::fail_on_conflict) is `true`.
    pub failed_on_first_unresolved_conflict: bool,
}

/// Determine what should be considered an unresolved conflict.
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TreatAsUnresolved {
    /// Determine which content merges should be considered unresolved.
    pub content_merge: treat_as_unresolved::ContentMerge,
    /// Determine which tree merges should be considered unresolved.
    pub tree_merge: treat_as_unresolved::TreeMerge,
}

///
pub mod treat_as_unresolved {
    use crate::tree::TreatAsUnresolved;

    /// Which kind of content merges should be considered unresolved?
    #[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub enum ContentMerge {
        /// Content merges that still show conflict markers, or whose entries have conflicting executable bits, as Git
        /// also considers those unresolved.
        #[default]
        Markers,
        /// Content merges who would have conflicted if it wasn't for a
        /// [resolution strategy](crate::blob::builtin_driver::text::Conflict::ResolveWithOurs).
        ForcedResolution,
    }

    /// Which kind of tree merges should be considered unresolved?
    #[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub enum TreeMerge {
        /// All failed renames.
        Undecidable,
        /// All failed renames, and the ones where a tree item was renamed to avoid a clash.
        #[default]
        EvasiveRenames,
        /// All of `EvasiveRenames`, and tree merges that would have conflicted but which were resolved
        /// with a [resolution strategy](super::ResolveWith).
        ForcedResolution,
    }

    /// Instantiation/Presets
    impl TreatAsUnresolved {
        /// Return an instance with the highest sensitivity to what should be considered unresolved as it
        /// includes entries which have been resolved using a [merge strategy](super::ResolveWith).
        pub fn forced_resolution() -> Self {
            Self {
                content_merge: ContentMerge::ForcedResolution,
                tree_merge: TreeMerge::ForcedResolution,
            }
        }

        /// Return an instance that considers unresolved any conflict that Git would also consider unresolved.
        /// This is the same as the `default()` implementation.
        pub fn git() -> Self {
            Self::default()
        }

        /// Only undecidable tree merges and conflict markers are considered unresolved.
        /// This also means that renamed entries to make space for a conflicting one is considered acceptable,
        /// making this preset the most lenient.
        pub fn undecidable() -> Self {
            Self {
                content_merge: ContentMerge::Markers,
                tree_merge: TreeMerge::Undecidable,
            }
        }
    }
}

impl Outcome<'_> {
    /// Return `true` if there is any conflict that would still need to be resolved as they would yield undesirable trees.
    /// This is based on `how` to determine what should be considered unresolved.
    pub fn has_unresolved_conflicts(&self, how: TreatAsUnresolved) -> bool {
        self.conflicts.iter().any(|c| c.is_unresolved(how))
    }

    /// Returns `true` if `index` changed as we applied conflicting stages to it, using `how` to determine if a
    /// conflict should be considered unresolved.
    /// `removal_mode` decides how unconflicted entries should be removed if they are superseded by
    /// their conflicted counterparts.
    /// It's important that `index` is at the state of [`Self::tree`].
    ///
    /// A [conflict](Conflict) changes the index only if it has [entries](Conflict::entries()). Conflicts that Git reports
    /// without conflicting stages, such as an [unclear directory rename split](ResolutionFailure::DirectoryRenameSplit),
    /// leave it unchanged.
    pub fn index_changed_after_applying_conflicts(
        &self,
        index: &mut gix_index::State,
        how: TreatAsUnresolved,
        removal_mode: apply_index_entries::RemovalMode,
    ) -> bool {
        apply_index_entries(&self.conflicts, how, index, removal_mode)
    }
}

/// A description of a conflict (i.e. merge issue without an auto-resolution) as seen during a [tree-merge](crate::tree()).
/// They may have a resolution that was applied automatically, or be left for the caller to resolved.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// A record on how the conflict resolution succeeded with `Ok(_)` or failed with `Err(_)`.
    /// Note that in case of `Err(_)`, edits may still have been made to the tree to aid resolution.
    /// On failure, one can examine `ours` and `theirs` to potentially find a custom solution.
    /// Note that the descriptions of resolutions or resolution failures may be swapped compared
    /// to the actual changes. This is due to changes like `modification|deletion` being treated the
    /// same as `deletion|modification`, i.e. *ours* is not more privileged than theirs.
    /// To compensate for that, use [`changes_in_resolution()`](Conflict::changes_in_resolution()).
    pub resolution: Result<Resolution, ResolutionFailure>,
    /// The change representing *our* side.
    pub ours: Change,
    /// The change representing *their* side.
    pub theirs: Change,
    /// An array to store an entry for each stage of the conflict.
    ///
    /// * `entries[0]`  => Base
    /// * `entries[1]`  => Ours
    /// * `entries[2]`  => Theirs
    ///
    /// Note that ours and theirs might be swapped, so one should access it through [`Self::entries()`] to compensate for that.
    pub entries: [Option<ConflictIndexEntry>; 3],
    /// Determine how to interpret the `ours` and `theirs` fields. This is used to implement [`Self::changes_in_resolution()`]
    /// and [`Self::into_parts_by_resolution()`].
    map: ConflictMapping,
}

/// A conflicting entry for insertion into the index.
/// It will always be either on stage 1 (ancestor/base), 2 (ours) or 3 (theirs)
#[derive(Debug, Clone, Copy)]
pub struct ConflictIndexEntry {
    /// The kind of object at this stage.
    /// Note that it's possible that this is a directory, for instance if a directory was replaced with a file.
    pub mode: gix_object::tree::EntryMode,
    /// The id defining the state of the object.
    pub id: gix_hash::ObjectId,
    /// Hidden, maybe one day we can do without?
    path_hint: Option<ConflictIndexEntryPathHint>,
}

/// A hint for [`apply_index_entries()`] to know which paths to use for an entry.
/// This is only used when necessary.
#[derive(Debug, Clone, Copy)]
enum ConflictIndexEntryPathHint {
    /// Use the previous path, i.e. rename source.
    Source,
    /// Use the current path as it is in the tree.
    Current,
    /// Use the path of the final destination, or *their* name.
    /// It's definitely finicky, as we don't store the actual path and instead refer to it.
    RenamedOrTheirs,
}

/// A utility to help define which side is what in the [`Conflict`] type.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ConflictMapping {
    /// The sides are as described in the field documentation, i.e. `ours` is `ours`.
    Original,
    /// The sides are the opposite of the field documentation. i.e. `ours` is `theirs` and `theirs` is `ours`.
    Swapped,
}

impl ConflictMapping {
    fn is_swapped(&self) -> bool {
        matches!(self, ConflictMapping::Swapped)
    }
    fn swapped(self) -> ConflictMapping {
        match self {
            ConflictMapping::Original => ConflictMapping::Swapped,
            ConflictMapping::Swapped => ConflictMapping::Original,
        }
    }
    fn to_global(self, global: ConflictMapping) -> ConflictMapping {
        match global {
            ConflictMapping::Original => self,
            ConflictMapping::Swapped => self.swapped(),
        }
    }
}

impl Conflict {
    /// Return `true` if this instance is considered unresolved based on the criterion specified by `how`.
    pub fn is_unresolved(&self, how: TreatAsUnresolved) -> bool {
        use crate::blob;
        let content_merge_unresolved = |info: &ContentMerge| match how.content_merge {
            treat_as_unresolved::ContentMerge::Markers => matches!(info.resolution, blob::Resolution::Conflict),
            treat_as_unresolved::ContentMerge::ForcedResolution => {
                matches!(
                    info.resolution,
                    blob::Resolution::Conflict | blob::Resolution::CompleteWithAutoResolvedConflict
                )
            }
        };
        match how.tree_merge {
            treat_as_unresolved::TreeMerge::Undecidable => {
                self.resolution.is_err() || self.content_merge().is_some_and(|info| content_merge_unresolved(&info))
            }
            treat_as_unresolved::TreeMerge::EvasiveRenames | treat_as_unresolved::TreeMerge::ForcedResolution => {
                match &self.resolution {
                    Ok(success) => match success {
                        Resolution::SourceLocationAffectedByRename { .. } => false,
                        Resolution::Forced(_) => {
                            how.tree_merge == treat_as_unresolved::TreeMerge::ForcedResolution
                                || self
                                    .content_merge()
                                    .is_some_and(|merged_blob| content_merge_unresolved(&merged_blob))
                        }
                        Resolution::OursModifiedTheirsRenamedAndChangedThenRename { merged_blob, .. } => {
                            merged_blob.as_ref().is_some_and(content_merge_unresolved)
                        }
                        Resolution::OursModifiedTheirsModifiedThenBlobContentMerge { merged_blob } => {
                            content_merge_unresolved(merged_blob)
                        }
                    },
                    Err(_failure) => true,
                }
            }
        }
    }

    /// Returns the changes of fields `ours` and `theirs` so they match their description in the
    /// [`Resolution`] or [`ResolutionFailure`] respectively.
    /// Without this, the sides may appear swapped as `ours|theirs` is treated the same as `theirs/ours`
    /// if both types are different, like `modification|deletion`.
    pub fn changes_in_resolution(&self) -> (&Change, &Change) {
        match self.map {
            ConflictMapping::Original => (&self.ours, &self.theirs),
            ConflictMapping::Swapped => (&self.theirs, &self.ours),
        }
    }

    /// Similar to [`changes_in_resolution()`](Self::changes_in_resolution()), but returns the parts
    /// of the structure so the caller can take ownership. This can be useful when applying your own
    /// resolutions for resolution failures.
    pub fn into_parts_by_resolution(self) -> (Result<Resolution, ResolutionFailure>, Change, Change) {
        match self.map {
            ConflictMapping::Original => (self.resolution, self.ours, self.theirs),
            ConflictMapping::Swapped => (self.resolution, self.theirs, self.ours),
        }
    }

    /// Return the index entries for insertion into the index, to match with what's returned by [`Self::changes_in_resolution()`].
    pub fn entries(&self) -> [Option<ConflictIndexEntry>; 3] {
        match self.map {
            ConflictMapping::Original => self.entries,
            ConflictMapping::Swapped => [self.entries[0], self.entries[2], self.entries[1]],
        }
    }

    /// Return information about the content merge if it was performed.
    pub fn content_merge(&self) -> Option<ContentMerge> {
        fn failure_merged_blob(failure: &ResolutionFailure) -> Option<ContentMerge> {
            match failure {
                ResolutionFailure::OursRenamedTheirsRenamedDifferently { merged_blob } => *merged_blob,
                ResolutionFailure::Unknown
                | ResolutionFailure::SubmoduleMerge
                | ResolutionFailure::SubmoduleAddAdd
                | ResolutionFailure::OursRenamedTheirsRenamedToSameLocation
                | ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed { .. }
                | ResolutionFailure::OursModifiedTheirsDeleted
                | ResolutionFailure::OursModifiedTheirsRenamedTypeMismatch
                | ResolutionFailure::OursModifiedTheirsDirectoryThenOursRenamed {
                    renamed_unique_path_to_modified_blob: _,
                }
                | ResolutionFailure::OursAddedTheirsAddedTypeMismatch { .. }
                | ResolutionFailure::OursModifiedTheirsModifiedTypeMismatch { .. }
                | ResolutionFailure::OursDeletedTheirsRenamed
                | ResolutionFailure::DirectoryRenameSplit
                | ResolutionFailure::FileInWayOfDirectoryRename { .. }
                | ResolutionFailure::DirectoryRenameCollision { .. }
                | ResolutionFailure::SourceLocationAffectedByRename { .. } => None,
            }
        }
        match &self.resolution {
            Ok(success) => match success {
                Resolution::Forced(failure) => failure_merged_blob(failure),
                Resolution::SourceLocationAffectedByRename { .. } => None,
                Resolution::OursModifiedTheirsRenamedAndChangedThenRename { merged_blob, .. } => *merged_blob,
                Resolution::OursModifiedTheirsModifiedThenBlobContentMerge { merged_blob } => Some(*merged_blob),
            },
            Err(failure) => failure_merged_blob(failure),
        }
    }
}

/// Describes of a conflict involving *our* change and *their* change was specifically resolved.
///
/// Note that all resolutions are side-agnostic, so *ours* could also have been *theirs* and vice versa.
/// Also note that symlink merges are always done via binary merge, using the same logic.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// *ours* renamed a directory into which *theirs* added a file or renamed one. The file was moved into the renamed
    /// directory, as [`DirectoryRenames::Move`] selects. With [`DirectoryRenames::MoveAsConflict`] as well, a move that
    /// returned a file that *theirs* renamed to its source is resolved if *ours* renamed that source too: merge-ort
    /// skips such a rename to itself, and *their* version of the file is merged at the destination of *our* rename.
    SourceLocationAffectedByRename {
        /// The repository-relative path to the location that the change ended up in after
        /// being affected by a renamed directory.
        final_location: BString,
    },
    /// *ours* was a modified blob and *theirs* renamed that blob.
    /// We moved the changed blob from *ours* to its new location, and merged it successfully.
    OursModifiedTheirsRenamedAndChangedThenRename {
        /// The mode of the merged result if it differs from the mode of the renamed blob, as the modifying side
        /// changed the executable bit.
        merged_mode: Option<gix_object::tree::EntryMode>,
        /// If `Some(…)`, the content of the involved blob had to be merged.
        merged_blob: Option<ContentMerge>,
    },
    /// *ours* and *theirs* carried changes and where content-merged.
    ///
    /// Note that *ours* and *theirs* may also be rewrites with the same destination and mode,
    /// or additions.
    OursModifiedTheirsModifiedThenBlobContentMerge {
        /// The outcome of the content merge.
        merged_blob: ContentMerge,
    },
    /// This is a resolution failure was forcefully turned into a usable resolution, i.e. [making a choice](ResolveWith)
    /// is turned into a valid resolution.
    Forced(ResolutionFailure),
}

/// Describes of a conflict involving *our* change and *their* failed to be resolved.
#[derive(Debug, Clone)]
pub enum ResolutionFailure {
    /// *ours* and *theirs* produced conflicting commits for a submodule, but submodule history isn't available to this
    /// tree merge to determine whether either commit contains the other.
    SubmoduleMerge,
    /// *ours* and *theirs* added different submodule commits at the same path, possibly by renaming one into place.
    /// There is no common gitlink at this path, so this is not a candidate for three-way reachability resolution.
    SubmoduleAddAdd,
    /// *ours* and *theirs* renamed different source entries to the same destination.
    OursRenamedTheirsRenamedToSameLocation,
    /// *ours* was renamed, but *theirs* was renamed differently. Both versions will be present in the tree,
    OursRenamedTheirsRenamedDifferently {
        /// If `Some(…)`, the content of the involved blob had to be merged.
        merged_blob: Option<ContentMerge>,
    },
    /// *ours* was modified, but *theirs* was turned into a directory, so *ours* was renamed to a non-conflicting path.
    OursModifiedTheirsDirectoryThenOursRenamed {
        /// The path at which `ours` can be found in the tree - it's in the same directory that it was in before.
        renamed_unique_path_to_modified_blob: BString,
    },
    /// *ours* is a directory, but *theirs* is a non-directory (i.e. file), which wants to be in its place, even though
    /// *ours* has a modification in that subtree.
    /// Rename *theirs* to retain that modification.
    ///
    /// Important: there is no actual modification on *ours* side, so *ours* is filled in with *theirs* as the data structure
    /// cannot represent this case.
    // TODO: Can we have a better data-structure? This would be for a rewrite though.
    OursDirectoryTheirsNonDirectoryTheirsRenamed {
        /// The non-conflicting path of *their* non-tree entry.
        renamed_unique_path_of_theirs: BString,
    },
    /// *ours* and *theirs* were added at the same path, possibly by renaming, with types that can't be merged, such as a
    /// regular file and a symlink, so they were kept at separate paths.
    ///
    /// As Git does, a directory keeps the path and the other entry moves. Otherwise the regular file moves and the
    /// other entry, a symlink or a submodule, keeps the path; if neither is a regular file, both move. An entry that
    /// moves is placed at a unique path named after its side.
    OursAddedTheirsAddedTypeMismatch {
        /// The location at which *our* entry was placed, or `None` if it kept its path.
        our_unique_location: Option<BString>,
        /// The location at which *their* entry was placed, or `None` if it kept its path.
        their_unique_location: Option<BString>,
    },
    /// *ours* and *theirs* both changed the same entry, and their results have types that can't be merged, such as a
    /// regular file and a symlink. Both were kept at separate paths, placed as for
    /// [`OursAddedTheirsAddedTypeMismatch`](Self::OursAddedTheirsAddedTypeMismatch), and the base entry stays with the
    /// result that has the base's type, if either has it.
    OursModifiedTheirsModifiedTypeMismatch {
        /// The location at which *our* entry was placed, or `None` if it kept its path.
        our_unique_location: Option<BString>,
        /// The location at which *their* entry was placed, or `None` if it kept its path.
        their_unique_location: Option<BString>,
    },
    /// *ours* was modified, and they renamed the same file, but there is also a non-mergable type-change.
    /// Here we keep both versions of the file.
    OursModifiedTheirsRenamedTypeMismatch,
    /// *ours* was deleted, but *theirs* was renamed.
    OursDeletedTheirsRenamed,
    /// *ours* was modified and *theirs* was deleted. We keep the modified one and ignore the deletion.
    OursModifiedTheirsDeleted,
    /// One side renamed the files of a directory into several other directories, none of which received more of them
    /// than every other, so it's unclear where the directory went and no directory rename applies to it. Both changes
    /// are the removal of that directory.
    DirectoryRenameSplit,
    /// *ours* renamed a directory into which *theirs* added a file or renamed one, but a file or directory is in the
    /// way at the path that the directory rename would move it to, so it stays where *theirs* put it.
    FileInWayOfDirectoryRename {
        /// The path that the directory rename would have moved *their* file to.
        destination: BString,
    },
    /// *ours* renamed directories into which *theirs* added or renamed several files that these directory renames
    /// would all move to one path. None of these files is moved, and each has a conflict of this kind.
    DirectoryRenameCollision {
        /// The path that the directory renames would have moved these files to.
        destination: BString,
    },
    /// *ours* renamed a directory into which *theirs* added a file or renamed one. The file was moved into the renamed
    /// directory, but as [`DirectoryRenames::MoveAsConflict`] selects, the move is left for the user to confirm.
    SourceLocationAffectedByRename {
        /// The repository-relative path that the file was moved to.
        final_location: BString,
    },
    /// *ours* and *theirs* are in an untested state so it can't be handled yet, and is considered a conflict
    /// without adding our *or* their side to the resulting tree.
    Unknown,
}

/// Information about a blob content merge for use in a [`Resolution`].
/// Note that content merges always count as success to avoid duplication of cases, which forces callers
/// to check for the [`resolution`](Self::resolution) field.
#[derive(Debug, Copy, Clone)]
pub struct ContentMerge {
    /// The fully merged blob.
    pub merged_blob_id: gix_hash::ObjectId,
    /// Identify the kind of resolution of the blob merge. Note that it may be conflicting, which, as in Git, includes
    /// merged entries whose executable bits conflict even though their content merged, and the merged entry then has
    /// *our* mode.
    pub resolution: crate::blob::Resolution,
}

/// A way to configure [`tree()`](crate::tree()).
#[derive(Default, Debug, Clone)]
pub struct Options {
    /// If *not* `None`, renames are found among the changes of each side of the merge as Git's `merge-ort` finds them:
    /// files with identical content are always paired, and other files only where the merge needs to know about
    /// their rename. [`percentage`](Rewrites::percentage) is the minimal similarity of a rename, and
    /// [`limit`](Rewrites::limit) bounds the search for renames of changed files as Git's rename limit does. Copies are
    /// never detected, and empty files are only paired if [`track_empty`](Rewrites::track_empty) is set.
    pub rewrites: Option<Rewrites>,
    /// What happens to files that one side added to a directory the other side renamed, or renamed into it.
    ///
    /// It has no effect if [`rewrites`](Self::rewrites) is `None`, as directory renames are inferred from renames.
    pub directory_renames: DirectoryRenames,
    /// Decide how blob-merges should be done. This relates to if conflicts can be resolved or not.
    pub blob_merge: crate::blob::platform::merge::Options,
    /// The context to use when invoking merge-drivers.
    pub blob_merge_command_ctx: gix_command::Context,
    /// If `Some(what-is-unresolved)`, the first unresolved conflict will cause the entire merge to stop.
    /// This is useful to see if there is any conflict, without performing the whole operation, something
    /// that can be very relevant during merges that would cause a lot of blob-diffs.
    pub fail_on_conflict: Option<TreatAsUnresolved>,
    /// This value also affects the size of merge-conflict markers, to allow differentiating
    /// merge conflicts on each level, for any value greater than 0, with values `N` causing `N*2`
    /// markers to be added to the configured value.
    ///
    /// This is used automatically when merging merge-bases recursively.
    pub marker_size_multiplier: u8,
    /// If `None`, when symlinks clash *ours* will be chosen and a conflict will occur.
    /// Otherwise, the same logic applies as for the merge of binary resources.
    pub symlink_conflicts: Option<crate::blob::builtin_driver::binary::ResolveWith>,
    /// If `None`, tree irreconcilable tree conflicts will result in [resolution failures](ResolutionFailure).
    /// Otherwise, one can choose a side. Note that it's still possible to determine that auto-resolution happened
    /// despite this choice, which allows carry forward the conflicting information, possibly for later resolution.
    /// If `Some(…)`, irreconcilable conflicts are reconciled by making a choice.
    /// Note that [`Conflict::entries()`] will still be set, to not degenerate information, even though they then represent
    /// the entries what would fit the index if no forced resolution was performed.
    /// It's up to the caller to handle that information mindfully.
    pub tree_conflicts: Option<ResolveWith>,
}

/// Decide how to resolve tree-related conflicts, but only those that have [no way of being correct](ResolutionFailure).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ResolveWith {
    /// On irreconcilable conflict, choose neither *our* nor *their* state, but keep the common *ancestor* state instead.
    Ancestor,
    /// On irreconcilable conflict, choose *our* side.
    ///
    /// Note that in order to get something equivalent to *theirs*, put *theirs* into the side of *ours*,
    /// swapping the sides essentially.
    Ours,
    /// Resolve as Git's `merge-ort` does when it merges merge-bases into a virtual merge-base, one level of recursion
    /// deeper than the merge that needs it.
    ///
    /// Changes involved in a conflict are applied as without forced resolution: a rename's source is removed, and a
    /// non-directory entry at a path where the other side has a directory moves to a unique path. An entry that can't
    /// be merged keeps the *ancestor* version at its path, or is absent where the ancestor has none, as for entries of
    /// distinct types or submodules that both sides added. The destination of a rename whose source the other side
    /// deleted gets the *ancestor* version of the source, unless the other side has an entry there as well, which is
    /// then merged with the renamed one.
    VirtualMergeBase,
}

/// Decide what happens to a file that one side adds to a directory the other side renamed, or renames into it, as
/// Git's `merge.directoryRenames` does.
///
/// A side renamed a directory if the directory is gone on that side, and more of its files were renamed into one
/// other directory than into any other. No directory rename applies to a directory that both sides renamed, or in
/// merges of merge-bases, which `merge-ort` performs without directory renames.
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DirectoryRenames {
    /// Leave such files where their side put them, as `merge.directoryRenames=false` does.
    Ignore,
    /// Move such files into the renamed directory, and record each move as
    /// [`Resolution::SourceLocationAffectedByRename`], as `merge.directoryRenames=true` does.
    Move,
    /// Move such files into the renamed directory, but record each move as the unresolved conflict
    /// [`ResolutionFailure::SourceLocationAffectedByRename`], as `merge.directoryRenames=conflict`, Git's default,
    /// does. A move that returns a renamed file to a source that the other side renamed as well is
    /// [resolved](Resolution::SourceLocationAffectedByRename) in this mode too.
    #[default]
    MoveAsConflict,
}

pub(super) mod function;
mod utils;
///
pub mod apply_index_entries {

    /// Determines how we deal with the removal of unconflicted entries if these are superseded by their conflicted counterparts,
    /// i.e. stage 1, 2 and 3.
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub enum RemovalMode {
        /// Add the [`gix_index::entry::Flags::REMOVE`] flag to entries that are to be removed.
        ///
        /// **Note** that this also means that unconflicted and conflicted stages will be visible in the same index.
        /// When written, entries marked for removal will automatically be ignored. However, this also means that
        /// one must not use the in-memory index or take specific care of entries that are marked for removal.
        Mark,
        /// Entries marked for removal (even those that were already marked) will be removed from memory at the end.
        ///
        /// This is an expensive step that leaves a consistent index, ready for use.
        Prune,
    }

    pub(super) mod function {
        use std::collections::{HashMap, hash_map};

        use bstr::{BStr, BString, ByteSlice};

        use crate::tree::{
            Conflict, ConflictIndexEntryPathHint, Resolution, ResolutionFailure, TreatAsUnresolved,
            apply_index_entries::RemovalMode,
        };

        /// Returns `true` if `index` changed as we applied conflicting stages to it, using `how` to determine if a
        /// conflict should be considered unresolved.
        /// Once a stage of a path conflicts, the unconflicting stage is removed even though it might be the one
        /// that is currently checked out.
        /// This removal is only done by flagging it with [gix_index::entry::Flags::REMOVE], which means
        /// these entries won't be written back to disk but will still be present in the index if `removal_mode`
        /// is [`RemovalMode::Mark`]. For proper removal, choose [`RemovalMode::Prune`].
        /// It's important that `index` matches the tree that was produced as part of the merge that also
        /// brought about `conflicts`, or else this function will fail if it cannot find the path matching
        /// the conflicting entries.
        ///
        /// A [conflict](Conflict) changes the index only if it has [entries](Conflict::entries()). Conflicts that Git
        /// reports without conflicting stages, such as an
        /// [unclear directory rename split](ResolutionFailure::DirectoryRenameSplit), leave it unchanged.
        /// Errors can only occour if `index` isn't the one created from the merged tree that produced the `conflicts`.
        pub fn apply_index_entries(
            conflicts: &[Conflict],
            how: TreatAsUnresolved,
            index: &mut gix_index::State,
            removal_mode: RemovalMode,
        ) -> bool {
            if index.is_sparse() {
                gix_trace::error!("Refusing to apply index entries to sparse index - it's not tested yet");
                return false;
            }
            let len = index.entries().len();
            let mut idx_by_path_stage = HashMap::<(gix_index::entry::Stage, &BStr), usize>::default();
            for conflict in conflicts.iter().filter(|c| c.is_unresolved(how)) {
                // The changes in the order in which the description of the conflict names them, which the path
                // hints of its entries refer to.
                let (ours, theirs) = conflict.changes_in_resolution();
                let (renamed_path, current_path): (Option<&BStr>, &BStr) = match &conflict.resolution {
                    Ok(success) => match success {
                        Resolution::Forced(_) => continue,
                        Resolution::SourceLocationAffectedByRename { final_location } => {
                            (Some(final_location.as_bstr()), final_location.as_bstr())
                        }
                        Resolution::OursModifiedTheirsRenamedAndChangedThenRename { .. } => (None, theirs.location()),
                        Resolution::OursModifiedTheirsModifiedThenBlobContentMerge { .. } => (None, ours.location()),
                    },
                    Err(failure) => match failure {
                        ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                            renamed_unique_path_of_theirs,
                        } => (Some(renamed_unique_path_of_theirs.as_bstr()), ours.location()),
                        ResolutionFailure::OursRenamedTheirsRenamedDifferently { .. } => {
                            (Some(theirs.location()), ours.location())
                        }
                        ResolutionFailure::OursModifiedTheirsRenamedTypeMismatch
                        | ResolutionFailure::SubmoduleMerge
                        | ResolutionFailure::SubmoduleAddAdd
                        | ResolutionFailure::OursRenamedTheirsRenamedToSameLocation
                        | ResolutionFailure::OursDeletedTheirsRenamed
                        | ResolutionFailure::OursModifiedTheirsDeleted
                        | ResolutionFailure::DirectoryRenameSplit
                        | ResolutionFailure::FileInWayOfDirectoryRename { .. }
                        | ResolutionFailure::DirectoryRenameCollision { .. }
                        | ResolutionFailure::Unknown => (None, ours.location()),
                        ResolutionFailure::SourceLocationAffectedByRename { final_location } => {
                            (Some(final_location.as_bstr()), final_location.as_bstr())
                        }
                        ResolutionFailure::OursModifiedTheirsDirectoryThenOursRenamed {
                            renamed_unique_path_to_modified_blob,
                        } => (Some(renamed_unique_path_to_modified_blob.as_bstr()), ours.location()),
                        ResolutionFailure::OursAddedTheirsAddedTypeMismatch {
                            our_unique_location,
                            their_unique_location,
                        }
                        | ResolutionFailure::OursModifiedTheirsModifiedTypeMismatch {
                            our_unique_location,
                            their_unique_location,
                        } => {
                            // Both changes are at the contested location, which an entry without a unique
                            // location kept.
                            let location = ours.location();
                            (
                                Some(their_unique_location.as_ref().map_or(location, |path| path.as_bstr())),
                                our_unique_location.as_ref().map_or(location, |path| path.as_bstr()),
                            )
                        }
                    },
                };
                let source_path = ours.source_location();

                let entries_with_stage = conflict
                    .entries()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(idx, entry)| {
                        entry.filter(|e| e.mode.is_no_tree()).map(|e| {
                            (
                                match idx {
                                    0 => gix_index::entry::Stage::Base,
                                    1 => gix_index::entry::Stage::Ours,
                                    2 => gix_index::entry::Stage::Theirs,
                                    _ => unreachable!("fixed size array with three items"),
                                },
                                match e.path_hint {
                                    None => renamed_path.unwrap_or(current_path),
                                    Some(ConflictIndexEntryPathHint::Source) => source_path,
                                    Some(ConflictIndexEntryPathHint::Current) => current_path,
                                    Some(ConflictIndexEntryPathHint::RenamedOrTheirs) => {
                                        renamed_path.unwrap_or_else(|| theirs.location())
                                    }
                                },
                                e,
                            )
                        })
                    })
                    // merge-ort has no entry at a path that is a directory of the merged tree: it drops a file
                    // that only the base has there and moves a file of a side to a unique path, which has its
                    // own conflict.
                    .filter(|(_, path, _)| !is_directory_bounded(index, path, len))
                    .collect::<Vec<_>>();

                if !entries_with_stage.iter().any(|(_, path, _)| {
                    index
                        .entry_index_by_path_and_stage_bounded(path, gix_index::entry::Stage::Unconflicted, len)
                        .is_some()
                }) {
                    continue;
                }

                for (stage, path, entry) in entries_with_stage {
                    if let Some(pos) =
                        index.entry_index_by_path_and_stage_bounded(path, gix_index::entry::Stage::Unconflicted, len)
                    {
                        index.entries_mut()[pos].flags.insert(gix_index::entry::Flags::REMOVE);
                    }
                    match idx_by_path_stage.entry((stage, path)) {
                        hash_map::Entry::Occupied(map_entry) => {
                            // This can happen due to the way the algorithm works.
                            // The same happens in Git, but it stores the index-related data as part of its deduplicating tree.
                            // We store each conflict we encounter, which also may duplicate their index entries, sometimes, but
                            // with different values. The most recent value wins.
                            // Instead of trying to deduplicate the index entries when the merge runs, we put the cost
                            // to the tree-assembly - there is no way around it.
                            let index_entry = &mut index.entries_mut()[*map_entry.get()];
                            index_entry.mode = entry.mode.into();
                            index_entry.id = entry.id;
                        }
                        hash_map::Entry::Vacant(map_entry) => {
                            map_entry.insert(index.entries().len());
                            index.dangerously_push_entry(
                                Default::default(),
                                entry.id,
                                stage.into(),
                                entry.mode.into(),
                                path,
                            );
                        }
                    }
                }
            }

            let res = index.entries().len() != len;
            match removal_mode {
                RemovalMode::Mark => {}
                RemovalMode::Prune => {
                    index.remove_entries(|_, _, e| e.flags.contains(gix_index::entry::Flags::REMOVE));
                }
            }
            index.sort_entries();
            res
        }

        /// Return `true` if `path` is a directory of the entries in `index.entries()[..len]`, which are sorted by their
        /// paths.
        fn is_directory_bounded(index: &gix_index::State, path: &BStr, len: usize) -> bool {
            let mut prefix = BString::from(path);
            prefix.push(b'/');
            let entries = &index.entries()[..len];
            let pos = entries.partition_point(|entry| entry.path(index) < prefix.as_bstr());
            entries
                .get(pos)
                .is_some_and(|entry| entry.path(index).starts_with(prefix.as_slice()))
        }
    }
}
pub use apply_index_entries::function::apply_index_entries;
