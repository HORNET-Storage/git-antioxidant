//! Tree-merge scheduling and conflict resolution.
//!
//! See [`tree()`] for the main entrypoint and how it works.

use gix_error::Result;
use std::borrow::Cow;

use bstr::{BString, ByteSlice};
use gix_diff::tree_with_rewrites::Change;
use gix_hash::ObjectId;
use gix_object::{FindExt, tree, tree::EntryMode};

use crate::tree::{
    Conflict, ConflictIndexEntry, ConflictIndexEntryPathHint, ConflictMapping,
    ConflictMapping::{Original, Swapped},
    ContentMerge, Options, Outcome, Resolution, ResolutionFailure, ResolveWith,
    utils::{
        ChangeDisposition, ChangeList, PossibleConflict, TrackedChange, TreeNodes, apply_change, perform_blob_merge,
        possibly_rewritten_location, rewrite_location_with_renamed_directory, to_components, unique_path_in_tree,
        unique_path_in_tree_avoiding,
    },
};

use super::change::{MatchKind, collect as collect_changes, matching as matching_change, pair as pair_candidate};

/// Perform a merge between `our_tree` and `their_tree`, using `base_tree` as merge-base.
/// Note that `base_tree` can be an empty tree to indicate 'no common ancestor between the two sides'.
///
/// * `labels` names the ancestor, ours, and theirs resources passed to blob merge drivers and written into text
///   conflict markers.
/// * `objects` loads the input and descendant trees as well as blobs needed for rename detection and content merges.
///   It also backs the returned tree editor for its entire lifetime.
/// * `write_blob_to_odb` persists content produced by a blob merge and returns the object ID to place in the result
///   tree. Existing input blobs are reused without calling it.
/// * `diff_state` is reusable scratch space for both ancestor-to-side tree diffs and blob-content merges.
/// * `diff_resource_cache` loads and caches blob resources used to determine rename similarity when [`options.rewrites`](Options::rewrites)
///   enables rewrite tracking.
/// * `blob_merge` is a pre-configured platform used when both sides changed compatible non-tree entries. It must not
///   fall back to worktree data: all merge inputs come from the supplied trees and `objects` database.
/// * [`options`](Options) configures rewrite tracking, blob and symlink conflict handling, merge-driver context, conflict-marker
///   size, forced tree-conflict resolution, and whether to stop at the first selected kind of unresolved conflict.
///
/// ### Side handling
///
/// The scheduler swaps the sides to share resolution logic instead of generally privileging one input.
/// If either side has ambiguous rewrite identities, it also chooses a canonical starting side so reversing ours and
/// theirs does not change how those rewrites are paired. Exact merge symmetry is still not guaranteed because
/// conflict-marker content and forced "ours" resolution are directional by definition.
///
/// ### Algorithm
///
/// 1. Diff the ancestor against each side, including rename detection, to obtain two flat lists of tracked changes.
/// 2. Build a path tree for each list. Its nodes point back to list entries and make same-path, tree/non-tree,
///    and renamed-directory interactions discoverable.
/// 3. Start an editor at the ancestor tree and process pending changes from one list against the other list's path tree.
///    A change can be applied directly, paired with another change and merged, consumed only as part of a conflict, or
///    transformed into a deferred change at a rewritten or unique path.
/// 4. Append deferred changes as pending work, then swap the two side-lists and repeat until neither side has pending
///    changes. Swapping roles lets the same scheduling and resolution code process both inputs.
///
/// Each tracked change therefore records both whether it still needs processing and whether its effect is actually
/// represented in the editor. This is why "processed without application" is distinct from "applied": a forced
/// ancestor resolution may consume a deletion while retaining the ancestor entry, and later path conflicts must not
/// behave as if that deletion had removed it.
///
/// ### Relation to Git's `merge-ort`
///
/// Git's `merge-ort` is the behavioral reference, but it uses different state and result construction. It collects all
/// relevant files and directories from the three input trees in a full-path map, detects and processes renames, then
/// resolves entries in reverse path order. As each directory is completed, it writes that subtree to the object database
/// and ultimately returns a parsed result tree.
///
/// This implementation instead diffs the ancestor against each side into flat change lists with path indexes, schedules
/// interactions between those lists, and applies resolved changes to an ancestor-backed sparse tree editor. It writes
/// newly merged blobs through `write_blob_to_odb`, but leaves serialization of the returned editor to the caller.
///
/// If `gix-diff` reports a copy, it is normalized to an addition because the source remains in place. By comparison,
/// `merge-ort` configures its diffcore pass for rename detection, not copy detection.
///
/// Both produce an as-merged-as-possible tree and retain conflict information. Here a conflict records either an
/// automatic [`Resolution`] or a [`ResolutionFailure`]; [`TreatAsUnresolved`](crate::tree::TreatAsUnresolved) determines
/// which recorded resolutions a caller still considers unresolved, so unresolved conflicts are not limited to content
/// containing conflict markers.
///
/// ### Errors
///
/// Selecting an absent binary merge resource, such as the ancestor in an add/add conflict, returns
/// [`crate::tree::Error::MissingBinaryMergeResult`], classified as [`gix_error::Class::NotFound`], unless the blob
/// merge is that of [merge-bases](crate::blob::platform::merge::Options::is_virtual_ancestor), which reads the absent
/// ancestor as empty content as Git does. Downcast to [`crate::tree::Error`] and match this variant to distinguish it
/// from missing object headers or data.
///
/// ### Performance
///
/// Note that `objects` *should* have an object cache to greatly accelerate tree-retrieval.
#[expect(clippy::too_many_arguments)]
pub fn tree<'objects>(
    base_tree: &gix_hash::oid,
    our_tree: &gix_hash::oid,
    their_tree: &gix_hash::oid,
    mut labels: crate::blob::builtin_driver::text::Labels<'_>,
    objects: &'objects impl gix_object::FindObjectOrHeader,
    mut write_blob_to_odb: impl FnMut(&[u8]) -> Result<ObjectId>,
    diff_state: &mut gix_diff::tree::State,
    diff_resource_cache: &mut gix_diff::blob::Platform,
    blob_merge: &mut crate::blob::Platform,
    options: Options,
) -> Result<Outcome<'objects>> {
    let _span = gix_trace::coarse!("gix_merge::tree", ?base_tree, ?our_tree, ?their_tree, ?labels);
    let (mut base_buf, mut side_buf) = (Vec::new(), Vec::new());
    let mut editor = {
        let ancestor_tree = objects.find_tree(base_tree, &mut base_buf)?;
        tree::Editor::new(ancestor_tree.to_owned(), objects, base_tree.kind())
    };
    let resolve_tree_conflicts = options.tree_conflicts;

    let mut ours = collect_changes(
        base_tree,
        our_tree,
        &base_buf,
        &mut side_buf,
        objects,
        diff_resource_cache,
        diff_state,
        options.rewrites,
    )?;
    let mut theirs = collect_changes(
        base_tree,
        their_tree,
        &base_buf,
        &mut side_buf,
        objects,
        diff_resource_cache,
        diff_state,
        options.rewrites,
    )?;
    let mut conflicts = Vec::new();
    let mut failed_on_first_conflict = false;
    let mut should_fail_on_conflict = |mut conflict: Conflict| -> bool {
        if resolve_tree_conflicts.is_some()
            && let Err(failure) = conflict.resolution
        {
            conflict.resolution = Ok(Resolution::Forced(failure));
        }
        if let Some(how) = options.fail_on_conflict
            && (conflict.resolution.is_err() || conflict.is_unresolved(how))
        {
            failed_on_first_conflict = true;
        }
        conflicts.push(conflict);
        failed_on_first_conflict
    };

    // Ambiguous rewrite identities otherwise make the side processed first decide
    // which repeated additions are paired. Give both directions the same schedule.
    let canonicalize_schedule = ours.has_ambiguous_rewrite_sources() || theirs.has_ambiguous_rewrite_sources();
    let swap_sides_for_schedule = canonicalize_schedule && theirs.cmp_for_scheduling(&ours).is_gt();
    let ((mut our_changes, mut our_tree), (mut their_changes, mut their_tree)) = (ours.parts_mut(), theirs.parts_mut());
    let mut outer_side = Original;
    if their_changes.is_empty() || (!our_changes.is_empty() && swap_sides_for_schedule) {
        ((our_changes, our_tree), (their_changes, their_tree)) = ((their_changes, their_tree), (our_changes, our_tree));
        (labels.current, labels.other) = (labels.other, labels.current);
        outer_side = outer_side.swapped();
    }

    // Drain one side at a time. The first batch spans its current list; after each batch, the bounds advance to the old
    // length so only work appended during that batch is visited next. Resolution may also append to the opposite
    // side, which the swap below makes active. Once the swapped-in side has no pending change, both lists are drained,
    // and the merge is complete.
    'outer: while their_changes.iter().rev().any(TrackedChange::is_pending) {
        let mut batch_start = 0;
        let mut batch_end = their_changes.len();

        while batch_start != batch_end {
            for theirs_idx in batch_start..batch_end {
                // Tree changes are structural index entries, not work items: their descendant leaf changes carry the
                // actual edits. They remain in the side's path index so matching can detect file/directory overlaps and
                // paths crossing a renamed directory after the scheduler swaps sides and searches this side's index.
                let TrackedChange {
                    inner: theirs,
                    needs_tree_insertion,
                    rewritten_location,
                    ..
                } = &their_changes[theirs_idx];
                if theirs.entry_mode().is_tree() || !their_changes[theirs_idx].is_pending() {
                    continue;
                }

                if needs_tree_insertion.is_some() {
                    their_tree.insert(theirs, theirs_idx);
                }

                match matching_change(
                    theirs,
                    *needs_tree_insertion,
                    rewritten_location.as_ref(),
                    our_tree,
                    our_changes,
                ) {
                    None => {
                        if let Some((rewritten_location, ours_idx)) = rewritten_location {
                            // The deferred change has no conflict at its directory-rewritten destination. Record the
                            // relocation for callers, but attach no unmerged index stages because the editor contains
                            // the complete resolved result.
                            if should_fail_on_conflict(Conflict::with_resolution(
                                Resolution::SourceLocationAffectedByRename {
                                    final_location: rewritten_location.to_owned(),
                                },
                                (&our_changes[*ours_idx].inner, theirs, Original, outer_side),
                                [None, None, None],
                            )) {
                                break 'outer;
                            }
                            editor.remove_if_leaf(to_components(theirs.location()))?;
                        }
                        apply_change(&mut editor, theirs, rewritten_location.as_ref().map(|t| &t.0))?;
                        their_changes[theirs_idx].mark_applied();
                    }
                    Some(candidate) => {
                        use crate::tree::utils::to_components_bstring_ref as toc;

                        if let PossibleConflict::PassedRewrittenDirectory { change_idx } = candidate {
                            let ours = &our_changes[change_idx];
                            // This path passed through a directory renamed on the other side: for `a -> b`, translate
                            // `a/file` to `b/file`. `None` means the candidate cannot provide that directory rewrite.
                            let location_after_passed_rename =
                                rewrite_location_with_renamed_directory(theirs.location(), &ours.inner);
                            if let Some(new_location) = location_after_passed_rename {
                                // The translated destination may have conflicts of its own, so retry there instead of
                                // applying now. Remove the old scheduling node (which another structural conflict may
                                // already have consumed) and ignore this same directory rewrite on the retry.
                                their_tree.remove_change(theirs.location());
                                push_deferred_with_rewrite(
                                    (theirs.clone(), Some(change_idx)),
                                    Some((new_location, change_idx)),
                                    their_changes,
                                );
                            } else {
                                // The passed node did not yield a directory rewrite for this path, so there is no
                                // translated destination to retry. Apply the change at its original location.
                                apply_change(&mut editor, theirs, None)?;
                                their_changes[theirs_idx].mark_applied();
                            }
                            their_changes[theirs_idx].mark_processed();
                            continue;
                        }

                        let (ours_idx, match_kind) = pair_candidate(&candidate, our_changes);
                        // A path overlap need not identify a change that the resolution matrix can pair with `theirs`.
                        // For example, ours may modify `dir/file` while theirs replaces `dir` with a blob, leaving no
                        // ours-change at `dir` itself. Handle such structural-only overlaps below.
                        let Some(ours_idx) = ours_idx else {
                            let candidate_ours_idx = candidate.change_idx();
                            // Example: the base has `dir/file`, ours modifies `dir/file`, and theirs replaces `dir`
                            // with a blob. Our index has descendants below `dir` but no change at `dir` itself. The
                            // normal conflict resolution keeps our directory and moves their blob to a unique path.
                            if candidate_ours_idx.is_none()
                                && matches!(candidate, PossibleConflict::TreeToNonTree { .. })
                            {
                                let (mode, id) = theirs.entry_mode_and_id();
                                let location = theirs.location();
                                if needs_tree_insertion.is_some() {
                                    their_tree.remove_change(location);
                                }
                                let renamed_location = unique_path_in_tree(
                                    location.as_bstr(),
                                    &editor,
                                    their_tree,
                                    labels.other.unwrap_or_default(),
                                )?;
                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.upsert(toc(&renamed_location), mode.kind(), id.to_owned())?;
                                    }
                                    Some(ResolveWith::Ours) => {
                                        if outer_side.is_swapped() {
                                            editor.upsert(to_components(location), mode.kind(), id.to_owned())?;
                                        }
                                    }
                                    Some(ResolveWith::Ancestor) => {
                                        // we found no matching node of 'ours', so nothing to apply here.
                                    }
                                }

                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                                        renamed_unique_path_of_theirs: renamed_location,
                                    },
                                    (theirs, theirs, Original, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            &mode.kind().into(),
                                            &id.to_owned(),
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                );
                                their_changes[theirs_idx].mark_processed();
                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            } else {
                                // All remaining shapes violate path-index invariants. If the candidate has no indexed
                                // change, use the active change for both sides because `Conflict` requires a pair; this
                                // preserves the unknown conflict instead of attempting an unvalidated structural edit.
                                let ours = candidate_ours_idx.map_or(theirs, |idx| &our_changes[idx].inner);
                                gix_trace::warn!(
                                    "Unexpected unpairable path overlap; recording an unknown conflict. theirs: {theirs:#?} ours: {ours:#?} kind: {match_kind:?} candidate: {candidate:#?}"
                                );
                                let conflict = Conflict::unknown((ours, theirs, Original, outer_side));
                                if let Some(ResolveWith::Ours) = resolve_tree_conflicts {
                                    let selected = match outer_side {
                                        Original => ours,
                                        Swapped => theirs,
                                    };
                                    apply_change(&mut editor, selected, None)?;
                                    match (outer_side, candidate_ours_idx) {
                                        (Original, Some(ours_idx)) => our_changes[ours_idx].mark_applied(),
                                        _ => their_changes[theirs_idx].mark_applied(),
                                    }
                                }
                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            continue;
                        };

                        // Pairing consumes both changes, so each starts as `Processed`. An arm upgrades a side to
                        // `Applied` only when its effect is represented by `apply_change_and_mark()` or equivalent manual
                        // editor updates; a discarded or ancestor-resolved side remains `Processed`. The common tail
                        // passes both dispositions to `TrackedChange::mark()`.
                        let (mut ours_disposition, mut theirs_disposition) =
                            (ChangeDisposition::Processed, ChangeDisposition::Processed);
                        let ours = &our_changes[ours_idx].inner;
                        // Both changes are concrete here: for example, ours may modify `file` while theirs deletes the
                        // same `file`, unlike the structural-only `dir` overlap handled above. Symmetric arms determine
                        // which local change represents each logical side, edit the result, and record any conflict. The
                        // common tail applies the chosen dispositions to both scheduled changes.
                        match (ours, theirs) {
                            // Modify/rename: carry the modification to the rename destination, merging content if the
                            // rename also changed it. Incompatible entry types retain both paths unless policy chooses a
                            // side. For example, ours modifies `file` while theirs only renames it to `moved`; if theirs
                            // also edits it, merge both edits at `moved`. If ours changes `file` into a symlink while
                            // theirs renames it as a blob, retain the symlink at `file` and the blob at `moved`. A directory
                            // rename on the modifying side can further relocate a destination such as `dir/moved` to
                            // `new/moved`.
                            (
                                Change::Modification {
                                    previous_id,
                                    previous_entry_mode,
                                    id: our_id,
                                    location: our_location,
                                    entry_mode: our_mode,
                                    ..
                                },
                                Change::Rewrite {
                                    source_id: their_source_id,
                                    id: their_id,
                                    location: their_location,
                                    entry_mode: their_mode,
                                    source_location,
                                    ..
                                },
                            )
                            | (
                                Change::Rewrite {
                                    source_id: their_source_id,
                                    id: their_id,
                                    location: their_location,
                                    entry_mode: their_mode,
                                    source_location,
                                    ..
                                },
                                Change::Modification {
                                    previous_id,
                                    previous_entry_mode,
                                    id: our_id,
                                    location: our_location,
                                    entry_mode: our_mode,
                                    ..
                                },
                            ) => {
                                let side = if matches!(ours, Change::Modification { .. }) {
                                    Original
                                } else {
                                    Swapped
                                };
                                if modes_are_compatible(*our_mode, *their_mode) {
                                    // Both changes derive from the entry at the rename source.
                                    let (merged_mode, modes_merged_cleanly) = merge_modes(
                                        *our_mode,
                                        *their_mode,
                                        Some(*previous_entry_mode),
                                        side.to_global(outer_side),
                                    );
                                    let their_rewritten_location = possibly_rewritten_location(
                                        pick(side, our_tree, their_tree),
                                        their_location.as_ref(),
                                        pick(side, our_changes, their_changes),
                                    );
                                    let renamed_without_change = their_source_id == their_id;
                                    let (merged_blob_id, resolution) = if renamed_without_change {
                                        (*our_id, identical_content_resolution(modes_merged_cleanly))
                                    } else {
                                        let (our_location, our_id, our_mode, their_location, their_id, their_mode) =
                                            match side {
                                                Original => (
                                                    our_location,
                                                    our_id,
                                                    our_mode,
                                                    their_location,
                                                    their_id,
                                                    their_mode,
                                                ),
                                                Swapped => (
                                                    their_location,
                                                    their_id,
                                                    their_mode,
                                                    our_location,
                                                    our_id,
                                                    our_mode,
                                                ),
                                            };
                                        let (merged_blob_id, resolution) = perform_blob_merge(
                                            labels,
                                            objects,
                                            blob_merge,
                                            &mut diff_state.buf1,
                                            &mut write_blob_to_odb,
                                            (our_location, *our_id, *our_mode),
                                            (their_location, *their_id, *their_mode),
                                            (source_location, *previous_id, *previous_entry_mode),
                                            (0, outer_side),
                                            &options,
                                        )?;
                                        let resolution = content_resolution(resolution, modes_merged_cleanly);
                                        (merged_blob_id, Some(resolution))
                                    };

                                    editor.remove_if_leaf(toc(our_location))?;
                                    pick_mut(side, our_tree, their_tree).remove_existing_change(our_location.as_bstr());
                                    let final_location = their_rewritten_location.clone();
                                    let new_change = Change::Addition {
                                        location: their_rewritten_location.unwrap_or_else(|| their_location.to_owned()),
                                        relation: None,
                                        entry_mode: merged_mode,
                                        id: merged_blob_id,
                                    };
                                    if should_fail_on_conflict(Conflict::with_resolution(
                                        Resolution::OursModifiedTheirsRenamedAndChangedThenRename {
                                            merged_mode: (merged_mode != *their_mode).then_some(merged_mode),
                                            merged_blob: resolution.map(|resolution| ContentMerge {
                                                resolution,
                                                merged_blob_id,
                                            }),
                                            final_location,
                                        },
                                        (ours, theirs, side, outer_side),
                                        [
                                            index_entry(previous_entry_mode, previous_id),
                                            index_entry(our_mode, our_id),
                                            index_entry(their_mode, their_id),
                                        ],
                                    )) {
                                        break 'outer;
                                    }

                                    // The other side gets the addition, not our side.
                                    push_deferred((new_change, None), pick_mut(side, their_changes, our_changes));
                                } else {
                                    match resolve_tree_conflicts {
                                        None => {
                                            // keep both states - 'our_location' is the previous location as well.
                                            editor.upsert(toc(our_location), our_mode.kind(), *our_id)?;
                                            editor.upsert(toc(their_location), their_mode.kind(), *their_id)?;
                                        }
                                        Some(ResolveWith::Ours) => {
                                            editor.remove_if_leaf(toc(source_location))?;
                                            if side.to_global(outer_side).is_swapped() {
                                                editor.upsert(toc(their_location), their_mode.kind(), *their_id)?;
                                            } else {
                                                editor.upsert(toc(our_location), our_mode.kind(), *our_id)?;
                                            }
                                        }
                                        Some(ResolveWith::Ancestor) => {}
                                        // The modification stays, and the rename's destination gets the ancestor's
                                        // version, as merge-ort does at that depth for a rename vs. a type change.
                                        Some(ResolveWith::VirtualMergeBase) => {
                                            editor.upsert(toc(our_location), our_mode.kind(), *our_id)?;
                                            editor.upsert(
                                                toc(their_location),
                                                previous_entry_mode.kind(),
                                                *previous_id,
                                            )?;
                                        }
                                    }

                                    if should_fail_on_conflict(Conflict::without_resolution(
                                        ResolutionFailure::OursModifiedTheirsRenamedTypeMismatch,
                                        (ours, theirs, side, outer_side),
                                        [
                                            index_entry_at_path(
                                                previous_entry_mode,
                                                previous_id,
                                                ConflictIndexEntryPathHint::RenamedOrTheirs,
                                            ),
                                            None,
                                            index_entry_at_path(
                                                their_mode,
                                                their_id,
                                                ConflictIndexEntryPathHint::RenamedOrTheirs,
                                            ),
                                        ],
                                    )) {
                                        break 'outer;
                                    }
                                }
                            }
                            // Modify/modify for compatible, non-submodule entries: merge their content and modes. For
                            // example, both sides produce different contents for ancestor blob `script`, possibly changing
                            // its executable bit as well, or produce the same content with different executable bits. If
                            // the ancestor had another type, such as a symlink both sides replace with blobs, merge those
                            // blobs with an empty compatible base instead, and different executable bits conflict.
                            (
                                Change::Modification {
                                    location,
                                    previous_id,
                                    previous_entry_mode,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    ..
                                },
                                Change::Modification {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    ..
                                },
                            ) if !involves_submodule(our_mode, their_mode)
                                && modes_are_compatible(*our_mode, *their_mode) =>
                            {
                                let (merged_mode, modes_merged_cleanly) =
                                    merge_modes(*our_mode, *their_mode, Some(*previous_entry_mode), outer_side);
                                let (merged_blob_id, resolution) = if our_id == their_id {
                                    // Both sides produced the same content, so only their modes need merging.
                                    (*our_id, identical_content_resolution(modes_merged_cleanly))
                                } else {
                                    let (merge_base_id, merge_base_mode) =
                                        if modes_are_compatible(*our_mode, *previous_entry_mode) {
                                            (*previous_id, *previous_entry_mode)
                                        } else {
                                            (previous_id.kind().null(), merged_mode)
                                        };
                                    let (merged_blob_id, resolution) = perform_blob_merge(
                                        labels,
                                        objects,
                                        blob_merge,
                                        &mut diff_state.buf1,
                                        &mut write_blob_to_odb,
                                        (location, *our_id, *our_mode),
                                        (location, *their_id, *their_mode),
                                        (location, merge_base_id, merge_base_mode),
                                        (0, outer_side),
                                        &options,
                                    )?;
                                    let resolution = content_resolution(resolution, modes_merged_cleanly);
                                    (merged_blob_id, Some(resolution))
                                };

                                editor.upsert(toc(location), merged_mode.kind(), merged_blob_id)?;
                                if let Some(resolution) = resolution
                                    && should_fail_on_conflict(Conflict::with_resolution(
                                        Resolution::OursModifiedTheirsModifiedThenBlobContentMerge {
                                            merged_blob: ContentMerge {
                                                resolution,
                                                merged_blob_id,
                                            },
                                        },
                                        (ours, theirs, Original, outer_side),
                                        [
                                            index_entry(previous_entry_mode, previous_id),
                                            index_entry(our_mode, our_id),
                                            index_entry(their_mode, their_id),
                                        ],
                                    ))
                                {
                                    break 'outer;
                                }
                            }
                            // Divergent modifications of one submodule are a known conflict, but this tree-only merge
                            // cannot inspect submodule history to select a descendant. For example, both sides move
                            // gitlink `sub` from the same base commit to different commits.
                            (
                                Change::Modification {
                                    previous_entry_mode,
                                    previous_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    ..
                                },
                                Change::Modification {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    ..
                                },
                            ) if our_mode.is_commit() && their_mode.is_commit() => {
                                if our_id == their_id {
                                    apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                    theirs_disposition = ChangeDisposition::Applied;
                                } else {
                                    // Like merge-ort, leave the conflict unresolved but put ours in the result tree when
                                    // no forced resolution was requested. A later submodule-aware pass can replace it. A
                                    // merged merge-base keeps the ancestor's commit, as merge-ort does at that depth
                                    // when neither commit contains the other.
                                    if !matches!(
                                        resolve_tree_conflicts,
                                        Some(ResolveWith::Ancestor | ResolveWith::VirtualMergeBase)
                                    ) {
                                        apply_our_resolution(
                                            ours,
                                            theirs,
                                            outer_side,
                                            &mut editor,
                                            &mut ours_disposition,
                                            &mut theirs_disposition,
                                        )?;
                                    }
                                    if should_fail_on_conflict(Conflict::without_resolution(
                                        ResolutionFailure::SubmoduleMerge,
                                        (ours, theirs, Original, outer_side),
                                        [
                                            index_entry(previous_entry_mode, previous_id),
                                            index_entry(our_mode, our_id),
                                            index_entry(their_mode, their_id),
                                        ],
                                    )) {
                                        break 'outer;
                                    }
                                }
                            }
                            // Modify/modify with results of distinct types, such as a regular file and a
                            // symlink, which can't be merged: both are kept, placed as Git does. For example,
                            // ours replaces the symlink `item` with a regular file while theirs changes the
                            // symlink's target. The symlink keeps `item` and the base entry, and the regular file
                            // moves to a path named after our side.
                            (
                                Change::Modification {
                                    location,
                                    previous_entry_mode,
                                    previous_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    ..
                                },
                                Change::Modification {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    ..
                                },
                            ) if !modes_are_compatible(*our_mode, *their_mode) => {
                                let placement = DistinctTypes::new(
                                    location,
                                    (*our_mode, *our_id),
                                    (*their_mode, *their_id),
                                    &editor,
                                    (&*our_tree, &*their_tree),
                                    labels,
                                )?;
                                let (our_unique_location, their_unique_location) =
                                    placement.unique_locations(outer_side);
                                let base = placement.base_entry((*previous_entry_mode, *previous_id), outer_side);
                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursModifiedTheirsModifiedTypeMismatch {
                                        our_unique_location,
                                        their_unique_location,
                                    },
                                    (ours, theirs, Original, outer_side),
                                    placement.index_entries(base, outer_side),
                                );
                                match resolve_tree_conflicts {
                                    None => placement.apply(
                                        &mut editor,
                                        (&mut *our_tree, &mut *their_tree),
                                        (&mut *our_changes, &mut *their_changes),
                                        (&mut ours_disposition, &mut theirs_disposition),
                                        (ours_idx, theirs_idx),
                                    )?,
                                    Some(ResolveWith::Ours) => apply_our_resolution(
                                        ours,
                                        theirs,
                                        outer_side,
                                        &mut editor,
                                        &mut ours_disposition,
                                        &mut theirs_disposition,
                                    )?,
                                    // A merged merge-base keeps the ancestor's entry, as merge-ort does at that depth.
                                    Some(ResolveWith::Ancestor | ResolveWith::VirtualMergeBase) => {}
                                }
                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // Both sides deleting the same base entry agree on the result. For example, both delete
                            // `dir/file`, even if each deletion is related to a different parent rewrite.
                            (Change::Deletion { .. }, Change::Deletion { .. }) => {
                                apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                theirs_disposition = ChangeDisposition::Applied;
                            }
                            // A descendant addition encountered before its shared parent deletion must wait for that
                            // deletion to pair with the other side's deletion; applying it later would remove the new
                            // directory. For example, both sides delete the file `dir`, while theirs also adds `dir/file`.
                            (Change::Deletion { .. }, Change::Addition { .. })
                                if matches!(match_kind, Some(MatchKind::EraseLeaf))
                                    && !our_changes[ours_idx].was_processed_without_application() =>
                            {
                                push_deferred((theirs.clone(), Some(ours_idx)), their_changes);
                            }
                            // A deletion processed without application retained its base leaf due to an earlier forced
                            // resolution, so its related child cannot be added. For example, choosing the ancestor for a
                            // rename/delete of `dir` also discards the other side's deferred `dir/file` without creating a
                            // second conflict.
                            (Change::Deletion { .. }, Change::Addition { .. })
                                if matches!(match_kind, Some(MatchKind::EraseLeaf))
                                    && our_changes[ours_idx].was_processed_without_application() =>
                            {
                                // Both changes remain processed: the earlier forced resolution is already recorded.
                            }
                            // A child of a file-to-directory replacement waits for the parent deletion to settle the
                            // blocking rewrite; the retry ignores that same rewrite when looking for another conflict. For
                            // example, ours renames the file `dir` to `moved` while theirs replaces it with `dir/file`.
                            (Change::Rewrite { .. }, Change::Addition { relation: Some(_), .. })
                                if matches!(match_kind, Some(MatchKind::EraseLeaf))
                                    && needs_tree_insertion.is_none()
                                    && !matches!(resolve_tree_conflicts, Some(ResolveWith::Ancestor)) =>
                            {
                                push_deferred((theirs.clone(), Some(ours_idx)), their_changes);
                            }
                            // An explicit file rename wins over an inferred directory relocation at the same destination;
                            // keep the rename and apply the addition at its original path. For example, `file` is renamed
                            // explicitly to `new`, while a directory rename would place an added child below `new/`.
                            (Change::Rewrite { .. }, Change::Addition { .. })
                                if matches!(match_kind, Some(MatchKind::EraseLeaf)) && rewritten_location.is_some() =>
                            {
                                apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                            }
                            // A rewritten non-tree blocks a directory introduced by the other side. Preserve the
                            // directory and relocate the blocking entry to a unique, side-qualified path. For example,
                            // ours renames the file `file` to `dir` while theirs adds `dir/child`.
                            (
                                Change::Rewrite {
                                    source_location,
                                    entry_mode: blocking_mode,
                                    id: blocking_id,
                                    location: blocking_location,
                                    ..
                                },
                                Change::Addition { .. },
                            ) if matches!(match_kind, Some(MatchKind::EraseLeaf)) => {
                                let renamed_location = unique_path_in_tree(
                                    blocking_location.as_bstr(),
                                    &editor,
                                    our_tree,
                                    labels.current.unwrap_or_default(),
                                )?;
                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                                        renamed_unique_path_of_theirs: renamed_location.clone(),
                                    },
                                    (ours, theirs, Swapped, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            blocking_mode,
                                            blocking_id,
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                );

                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(source_location))?;
                                        editor.remove_if_leaf(toc(blocking_location))?;
                                        our_tree.remove_change(blocking_location.as_bstr());
                                        editor.upsert(toc(&renamed_location), blocking_mode.kind(), *blocking_id)?;
                                        apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        ours_disposition = ChangeDisposition::Applied;
                                    }
                                    Some(ResolveWith::Ours) => match outer_side {
                                        Original => {
                                            apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                        }
                                        Swapped => {
                                            apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        }
                                    },
                                    Some(ResolveWith::Ancestor) => {}
                                }

                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // A non-tree addition blocks the other side's added directory. Preserve the directory and
                            // relocate the blocking addition to a unique, side-qualified path. For example, ours adds the
                            // file `dir` while theirs adds `dir/child`.
                            (
                                Change::Addition {
                                    location: blocking_location,
                                    entry_mode: blocking_mode,
                                    id: blocking_id,
                                    ..
                                },
                                Change::Addition { .. },
                            ) if matches!(match_kind, Some(MatchKind::EraseLeaf)) => {
                                // `ours` is the non-tree prefix of `theirs`, whose parent directories
                                // are represented only by already-applied structural changes. Preserve
                                // the directory at its intended path and move the blocking addition.
                                let renamed_location = unique_path_in_tree(
                                    blocking_location.as_bstr(),
                                    &editor,
                                    our_tree,
                                    labels.current.unwrap_or_default(),
                                )?;
                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                                        renamed_unique_path_of_theirs: renamed_location.clone(),
                                    },
                                    (ours, theirs, Swapped, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            blocking_mode,
                                            blocking_id,
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                );

                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(blocking_location))?;
                                        our_tree.remove_change(blocking_location.as_bstr());
                                        editor.upsert(toc(&renamed_location), blocking_mode.kind(), *blocking_id)?;
                                        apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        ours_disposition = ChangeDisposition::Applied;
                                    }
                                    Some(ResolveWith::Ours) => match outer_side {
                                        Original => {
                                            apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                        }
                                        Swapped => {
                                            editor.remove_if_leaf(toc(blocking_location))?;
                                            our_tree.remove_change(blocking_location.as_bstr());
                                            apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        }
                                    },
                                    Some(ResolveWith::Ancestor) => {}
                                }

                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // Two different submodule commits added at the same path need a graph-aware follow-up. For
                            // example, both sides add gitlink `vendor/lib`, but point it at different commits.
                            (
                                Change::Addition {
                                    entry_mode: our_mode,
                                    id: our_id,
                                    ..
                                },
                                Change::Addition {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    ..
                                },
                            ) if our_mode.is_commit() && their_mode.is_commit() => {
                                // With no merge base at this path, merge-ort likewise keeps ours as the provisional
                                // result while retaining the add/add conflict for a submodule-aware pass. A merged
                                // merge-base has no entry here, as merge-ort's two-way gitlink merge then takes the
                                // absent ancestor.
                                if !matches!(
                                    resolve_tree_conflicts,
                                    Some(ResolveWith::Ancestor | ResolveWith::VirtualMergeBase)
                                ) {
                                    apply_our_resolution(
                                        ours,
                                        theirs,
                                        outer_side,
                                        &mut editor,
                                        &mut ours_disposition,
                                        &mut theirs_disposition,
                                    )?;
                                }
                                if should_fail_on_conflict(Conflict::without_resolution(
                                    ResolutionFailure::SubmoduleAddAdd,
                                    (ours, theirs, Original, outer_side),
                                    [None, index_entry(our_mode, our_id), index_entry(their_mode, their_id)],
                                )) {
                                    break 'outer;
                                }
                            }
                            // Add/add at one path: compatible types use a two-way content merge with an empty
                            // ancestor, which leaves different executable bits in conflict, while entries of
                            // distinct types are both kept, placed as Git does. For example, two different blobs
                            // added at `file` are content-merged, while of a symlink and a blob both added at
                            // `file`, the symlink keeps `file` and the blob moves to a path named after its side.
                            // Two submodules are handled above.
                            (
                                Change::Addition {
                                    location,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    ..
                                },
                                Change::Addition {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    ..
                                },
                            ) => {
                                let conflict = if modes_are_compatible(*our_mode, *their_mode) {
                                    let (merged_mode, modes_merged_cleanly) =
                                        merge_modes(*our_mode, *their_mode, None, outer_side);
                                    let (merged_blob_id, resolution) = if our_id == their_id {
                                        (*our_id, crate::blob::Resolution::Complete)
                                    } else {
                                        perform_blob_merge(
                                            labels,
                                            objects,
                                            blob_merge,
                                            &mut diff_state.buf1,
                                            &mut write_blob_to_odb,
                                            (location, *our_id, merged_mode),
                                            (location, *their_id, merged_mode),
                                            (location, their_id.kind().null(), merged_mode),
                                            (0, outer_side),
                                            &options,
                                        )?
                                    };
                                    let resolution = content_resolution(resolution, modes_merged_cleanly);
                                    editor.upsert(toc(location), merged_mode.kind(), merged_blob_id)?;
                                    Conflict::with_resolution(
                                        Resolution::OursModifiedTheirsModifiedThenBlobContentMerge {
                                            merged_blob: ContentMerge {
                                                resolution,
                                                merged_blob_id,
                                            },
                                        },
                                        (ours, theirs, Original, outer_side),
                                        [None, index_entry(our_mode, our_id), index_entry(their_mode, their_id)],
                                    )
                                } else {
                                    let placement = DistinctTypes::new(
                                        location,
                                        (*our_mode, *our_id),
                                        (*their_mode, *their_id),
                                        &editor,
                                        (&*our_tree, &*their_tree),
                                        labels,
                                    )?;
                                    let (our_unique_location, their_unique_location) =
                                        placement.unique_locations(outer_side);
                                    let mut conflict = Conflict::without_resolution(
                                        ResolutionFailure::OursAddedTheirsAddedTypeMismatch {
                                            our_unique_location,
                                            their_unique_location,
                                        },
                                        (ours, theirs, Original, outer_side),
                                        placement.index_entries(None, outer_side),
                                    );
                                    match resolve_tree_conflicts {
                                        None => placement.apply(
                                            &mut editor,
                                            (&mut *our_tree, &mut *their_tree),
                                            (&mut *our_changes, &mut *their_changes),
                                            (&mut ours_disposition, &mut theirs_disposition),
                                            (ours_idx, theirs_idx),
                                        )?,
                                        // A directory keeps its path at every depth, and the other entry moves.
                                        Some(ResolveWith::VirtualMergeBase) if placement.involves_directory() => {
                                            placement.apply(
                                                &mut editor,
                                                (&mut *our_tree, &mut *their_tree),
                                                (&mut *our_changes, &mut *their_changes),
                                                (&mut ours_disposition, &mut theirs_disposition),
                                                (ours_idx, theirs_idx),
                                            )?;
                                        }
                                        Some(resolve) => {
                                            conflict.entries = Default::default();
                                            match resolve {
                                                ResolveWith::Ours => {
                                                    let (mode, id) = match outer_side {
                                                        Original => (our_mode, our_id),
                                                        Swapped => (their_mode, their_id),
                                                    };
                                                    editor.upsert(toc(location), mode.kind(), *id)?;
                                                }
                                                // A merged merge-base has no entry here either, as merge-ort keeps the
                                                // absent ancestor version of entries of distinct types at that depth.
                                                ResolveWith::Ancestor | ResolveWith::VirtualMergeBase => {
                                                    // Do nothing - this discards both sides.
                                                    // Note that one of these adds might be the result of a rename, which
                                                    // means we effectively loose the original and can't get it back as that information is degenerated.
                                                }
                                            }
                                        }
                                    }
                                    conflict
                                };

                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // Modify/delete in either order: normally retain the modified file as the conflicted result.
                            // For example, ours modifies `file` while theirs deletes it. If the deleting side also adds a
                            // tree at that path, such as replacing file `dir` with directory `dir/child`, relocate the
                            // modified file so the directory survives.
                            (
                                Change::Modification {
                                    location,
                                    entry_mode,
                                    id,
                                    previous_entry_mode,
                                    previous_id,
                                },
                                Change::Deletion { .. },
                            )
                            | (
                                Change::Deletion { .. },
                                Change::Modification {
                                    location,
                                    entry_mode,
                                    id,
                                    previous_entry_mode,
                                    previous_id,
                                },
                            ) => {
                                let (label_of_side_to_be_moved, side) = if matches!(ours, Change::Modification { .. }) {
                                    (labels.current.unwrap_or_default(), Original)
                                } else {
                                    (labels.other.unwrap_or_default(), Swapped)
                                };
                                let deletion_replaced_by_directory = {
                                    // The deleted leaf is replaced by a dir added at the same location.
                                    // Rename-tracking sort order shouldn't be dependent on here, but maybe
                                    // could one day once rename tracking caught up with Git.
                                    let changes = match side {
                                        Original => &their_changes,
                                        Swapped => &our_changes,
                                    };
                                    changes.iter().any(|change| {
                                        change.inner.entry_mode().is_tree()
                                            && matches!(change.inner, Change::Addition { .. })
                                            && change.inner.location() == location
                                    })
                                };

                                let should_break = if deletion_replaced_by_directory {
                                    let entries = [
                                        index_entry(previous_entry_mode, previous_id),
                                        index_entry(entry_mode, id),
                                        None,
                                    ];
                                    match resolve_tree_conflicts {
                                        None | Some(ResolveWith::VirtualMergeBase) => {
                                            let our_tree = pick_mut(side, our_tree, their_tree);
                                            let renamed_path = unique_path_in_tree(
                                                location.as_bstr(),
                                                &editor,
                                                our_tree,
                                                label_of_side_to_be_moved,
                                            )?;
                                            editor.remove_if_leaf(toc(location))?;
                                            our_tree.remove_existing_change(location.as_bstr());

                                            // A merged merge-base moves the ancestor's version, as merge-ort's
                                            // modify/delete keeps it at that depth.
                                            let (moved_mode, moved_id) = match resolve_tree_conflicts {
                                                Some(ResolveWith::VirtualMergeBase) => {
                                                    (previous_entry_mode, previous_id)
                                                }
                                                _ => (entry_mode, id),
                                            };
                                            let new_change = Change::Addition {
                                                location: renamed_path.clone(),
                                                relation: None,
                                                entry_mode: *moved_mode,
                                                id: *moved_id,
                                            };
                                            let should_break = should_fail_on_conflict(Conflict::without_resolution(
                                                ResolutionFailure::OursModifiedTheirsDirectoryThenOursRenamed {
                                                    renamed_unique_path_to_modified_blob: renamed_path,
                                                },
                                                (ours, theirs, side, outer_side),
                                                entries,
                                            ));

                                            // Since we move *our* side, our tree needs to be modified.
                                            push_deferred(
                                                (new_change, None),
                                                pick_mut(side, our_changes, their_changes),
                                            );
                                            should_break
                                        }
                                        Some(ResolveWith::Ours) => {
                                            match side.to_global(outer_side) {
                                                Original => {
                                                    // ours is modification
                                                    editor.upsert(toc(location), entry_mode.kind(), *id)?;
                                                }
                                                Swapped => {
                                                    // ours is deletion
                                                    editor.remove_if_leaf(toc(location))?;
                                                }
                                            }
                                            should_fail_on_conflict(Conflict::without_resolution(
                                                ResolutionFailure::OursModifiedTheirsDeleted,
                                                (ours, theirs, side, outer_side),
                                                entries,
                                            ))
                                        }
                                        Some(ResolveWith::Ancestor) => {
                                            should_fail_on_conflict(Conflict::without_resolution(
                                                ResolutionFailure::OursModifiedTheirsDeleted,
                                                (ours, theirs, side, outer_side),
                                                entries,
                                            ))
                                        }
                                    }
                                } else {
                                    let entries = [
                                        index_entry(previous_entry_mode, previous_id),
                                        index_entry(entry_mode, id),
                                        None,
                                    ];
                                    match resolve_tree_conflicts {
                                        None => {
                                            editor.upsert(toc(location), entry_mode.kind(), *id)?;
                                        }
                                        Some(ResolveWith::Ours) => {
                                            let ours = match outer_side {
                                                Original => ours,
                                                Swapped => theirs,
                                            };

                                            match ours {
                                                Change::Modification { .. } => {
                                                    editor.upsert(toc(location), entry_mode.kind(), *id)?;
                                                }
                                                Change::Deletion { .. } => {
                                                    editor.remove_if_leaf(toc(location))?;
                                                }
                                                _ => unreachable!("parent-match assures this"),
                                            }
                                        }
                                        // A merged merge-base keeps the ancestor's version, as merge-ort does at that
                                        // depth.
                                        Some(ResolveWith::Ancestor | ResolveWith::VirtualMergeBase) => {}
                                    }
                                    should_fail_on_conflict(Conflict::without_resolution(
                                        ResolutionFailure::OursModifiedTheirsDeleted,
                                        (ours, theirs, side, outer_side),
                                        entries,
                                    ))
                                };
                                let deletion_was_applied = match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => deletion_replaced_by_directory,
                                    Some(ResolveWith::Ours) => side.to_global(outer_side).is_swapped(),
                                    Some(ResolveWith::Ancestor) => false,
                                };
                                if deletion_was_applied {
                                    match side {
                                        Original => theirs_disposition = ChangeDisposition::Applied,
                                        Swapped => ours_disposition = ChangeDisposition::Applied,
                                    }
                                }
                                if should_break {
                                    break 'outer;
                                }
                            }
                            // A descendant addition from a file-to-directory replacement can arrive before the blocking
                            // file's delete/modify pair; defer it until that parent conflict is resolved. For example, ours
                            // modifies the file `dir` while theirs deletes it and adds `dir/child`.
                            (
                                Change::Modification { .. },
                                Change::Addition {
                                    location,
                                    entry_mode,
                                    id,
                                    ..
                                },
                            ) if ours.location() != theirs.location() => {
                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        // A file-to-directory diff can yield the descendant addition
                                        // before the deletion of the blocking base file. Defer it so
                                        // the modification/deletion pair can relocate the modification
                                        // and remove this structural match first.
                                        push_deferred((theirs.clone(), Some(ours_idx)), their_changes);
                                    }
                                    Some(ResolveWith::Ancestor) => {}
                                    Some(ResolveWith::Ours) => {
                                        if outer_side.is_swapped() {
                                            editor.upsert(toc(location), entry_mode.kind(), *id)?;
                                        }
                                        // we have already taken care of the 'root' of this -
                                        // everything that follows can safely be ignored
                                    }
                                }
                            }
                            // A file rename lands at a renamed directory's destination. Keep the directory there and
                            // relocate the file rename to a unique path. For example, a directory moves from `old` to
                            // `new` while a file moves from `file` to `new`.
                            (
                                Change::Rewrite {
                                    entry_mode: tree_mode,
                                    location: tree_location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location,
                                    entry_mode,
                                    id,
                                    location,
                                    ..
                                },
                            ) if tree_mode.is_tree() && tree_location == location => {
                                let renamed_location = unique_path_in_tree(
                                    location.as_bstr(),
                                    &editor,
                                    our_tree,
                                    labels.other.unwrap_or_default(),
                                )?;
                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                                        renamed_unique_path_of_theirs: renamed_location.clone(),
                                    },
                                    (ours, theirs, Original, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            entry_mode,
                                            id,
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                );

                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(source_location))?;
                                        editor.upsert(toc(&renamed_location), entry_mode.kind(), *id)?;
                                        their_tree.remove_existing_change(location.as_bstr());
                                        ours_disposition = ChangeDisposition::Applied;
                                        theirs_disposition = ChangeDisposition::Applied;
                                    }
                                    Some(ResolveWith::Ours) => {
                                        apply_our_resolution(
                                            ours,
                                            theirs,
                                            outer_side,
                                            &mut editor,
                                            &mut ours_disposition,
                                            &mut theirs_disposition,
                                        )?;
                                        match outer_side {
                                            Original => {
                                                their_tree.remove_existing_change(location.as_bstr());
                                            }
                                            Swapped => {
                                                our_tree.remove_existing_change(tree_location.as_bstr());
                                            }
                                        }
                                    }
                                    Some(ResolveWith::Ancestor) => {}
                                }

                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // A file rename targets the source vacated by a directory rename. Accept it there; descendant
                            // changes perform the directory's actual rename or deletion. For example, directory `old`
                            // moves to `new` while file `file` moves to `old`.
                            (
                                Change::Rewrite {
                                    source_location,
                                    entry_mode: tree_mode,
                                    ..
                                },
                                Change::Rewrite { location, .. },
                            ) if tree_mode.is_tree()
                                && location == source_location
                                && matches!(match_kind, Some(MatchKind::EraseTree)) =>
                            {
                                // The leaf rename occupies a path vacated by the directory rename.
                                // Descendant changes resolve the actual rename/delete conflict.
                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                    }
                                    Some(ResolveWith::Ours) => {
                                        apply_our_resolution(
                                            ours,
                                            theirs,
                                            outer_side,
                                            &mut editor,
                                            &mut ours_disposition,
                                            &mut theirs_disposition,
                                        )?;
                                    }
                                    Some(ResolveWith::Ancestor) => {}
                                }
                            }
                            // Different sources renamed to the same destination with identical type and content collapse
                            // cleanly into one entry while both sources are removed. For example, ours renames `a` to
                            // `dst` and theirs renames an identical `b` to `dst`.
                            (
                                Change::Rewrite {
                                    source_location: our_source_location,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location: their_source_location,
                                    entry_mode: their_mode,
                                    id: their_id,
                                    location: their_location,
                                    ..
                                },
                            ) if our_source_location != their_source_location
                                && location == their_location
                                && our_mode == their_mode
                                && our_id == their_id =>
                            {
                                editor.remove_if_leaf(toc(our_source_location))?;
                                editor.remove_if_leaf(toc(their_source_location))?;
                                our_tree.remove_change(our_source_location.as_bstr());
                                their_tree.remove_change(their_source_location.as_bstr());
                                editor.upsert(toc(location), our_mode.kind(), *our_id)?;
                                ours_disposition = ChangeDisposition::Applied;
                                theirs_disposition = ChangeDisposition::Applied;
                            }
                            // Different sources renamed to the same destination: as for add/add, compatible
                            // entries are content-merged, and entries of distinct types are both kept, placed as
                            // Git does. For example, blobs renamed from `a` and `b` to `dst` are merged, while of a
                            // symlink renamed from `a` and a blob renamed from `b` to `dst`, the symlink keeps
                            // `dst` and the blob moves. Two different submodules renamed to one destination are
                            // left to the fallback below.
                            (
                                Change::Rewrite {
                                    source_location: our_source_location,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location: their_source_location,
                                    entry_mode: their_mode,
                                    id: their_id,
                                    location: their_location,
                                    ..
                                },
                            ) if our_source_location != their_source_location
                                && location == their_location
                                && !(our_mode.is_commit() && their_mode.is_commit()) =>
                            {
                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(our_source_location))?;
                                        editor.remove_if_leaf(toc(their_source_location))?;
                                        our_tree.remove_change(our_source_location.as_bstr());
                                        their_tree.remove_change(their_source_location.as_bstr());
                                        let conflict = if modes_are_compatible(*our_mode, *their_mode) {
                                            let (merged_mode, modes_merged_cleanly) =
                                                merge_modes(*our_mode, *their_mode, None, outer_side);
                                            let (merged_blob_id, resolution) = perform_blob_merge(
                                                labels,
                                                objects,
                                                blob_merge,
                                                &mut diff_state.buf1,
                                                &mut write_blob_to_odb,
                                                (location, *our_id, *our_mode),
                                                (location, *their_id, *their_mode),
                                                (location, our_id.kind().null(), merged_mode),
                                                (0, outer_side),
                                                &options,
                                            )?;
                                            let resolution = content_resolution(resolution, modes_merged_cleanly);
                                            editor.upsert(toc(location), merged_mode.kind(), merged_blob_id)?;
                                            Conflict::with_resolution(
                                                Resolution::OursModifiedTheirsModifiedThenBlobContentMerge {
                                                    merged_blob: ContentMerge {
                                                        resolution,
                                                        merged_blob_id,
                                                    },
                                                },
                                                (ours, theirs, Original, outer_side),
                                                [
                                                    None,
                                                    index_entry(our_mode, our_id),
                                                    index_entry(their_mode, their_id),
                                                ],
                                            )
                                        } else if matches!(resolve_tree_conflicts, Some(ResolveWith::VirtualMergeBase))
                                        {
                                            // A merged merge-base has no entry at the destination, as merge-ort keeps
                                            // the absent ancestor version of entries of distinct types at that depth.
                                            Conflict::without_resolution(
                                                ResolutionFailure::OursRenamedTheirsRenamedToSameLocation,
                                                (ours, theirs, Original, outer_side),
                                                [
                                                    None,
                                                    index_entry(our_mode, our_id),
                                                    index_entry(their_mode, their_id),
                                                ],
                                            )
                                        } else {
                                            let placement = DistinctTypes::new(
                                                location,
                                                (*our_mode, *our_id),
                                                (*their_mode, *their_id),
                                                &editor,
                                                (&*our_tree, &*their_tree),
                                                labels,
                                            )?;
                                            let (our_unique_location, their_unique_location) =
                                                placement.unique_locations(outer_side);
                                            let conflict = Conflict::without_resolution(
                                                ResolutionFailure::OursAddedTheirsAddedTypeMismatch {
                                                    our_unique_location,
                                                    their_unique_location,
                                                },
                                                (ours, theirs, Original, outer_side),
                                                placement.index_entries(None, outer_side),
                                            );
                                            placement.apply(
                                                &mut editor,
                                                (&mut *our_tree, &mut *their_tree),
                                                (&mut *our_changes, &mut *their_changes),
                                                (&mut ours_disposition, &mut theirs_disposition),
                                                (ours_idx, theirs_idx),
                                            )?;
                                            conflict
                                        };
                                        if should_fail_on_conflict(conflict) {
                                            break 'outer;
                                        }
                                    }
                                    Some(resolve) => {
                                        if matches!(resolve, ResolveWith::Ours) {
                                            let (source, mode, id, tree) = match outer_side {
                                                Original => (our_source_location, our_mode, our_id, &mut *our_tree),
                                                Swapped => {
                                                    (their_source_location, their_mode, their_id, &mut *their_tree)
                                                }
                                            };
                                            editor.remove_if_leaf(toc(source))?;
                                            tree.remove_change(source.as_bstr());
                                            editor.upsert(toc(location), mode.kind(), *id)?;
                                        }
                                        if should_fail_on_conflict(Conflict::without_resolution(
                                            ResolutionFailure::OursRenamedTheirsRenamedToSameLocation,
                                            (ours, theirs, Original, outer_side),
                                            [None, index_entry(our_mode, our_id), index_entry(their_mode, their_id)],
                                        )) {
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                            // One side renames a file away while the other replaces its source with a directory and adds
                            // a child below it. The child is unrelated to the renamed blob, so both results can coexist.
                            // For example, ours renames `path` to `moved` while theirs adds `path/child`.
                            (
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                                Change::Addition {
                                    id: their_id,
                                    entry_mode: their_mode,
                                    location: add_location,
                                    ..
                                },
                            )
                            | (
                                Change::Addition {
                                    id: their_id,
                                    entry_mode: their_mode,
                                    location: add_location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                            ) if add_location
                                .strip_prefix(source_location.as_bytes())
                                .is_some_and(|suffix| suffix.starts_with(b"/")) =>
                            {
                                // The rewrite moves the file out of the way while the other side replaces it
                                // with a directory. The child is unrelated to the rewritten blob, so keep both
                                // instead of merging their contents at the rewrite destination. The preceding
                                // deletion/rewrite pairing already recorded the rename/delete conflict.
                                let side = if matches!(ours, Change::Rewrite { .. }) {
                                    Original
                                } else {
                                    Swapped
                                };
                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        // A merged merge-base gives the rename's destination the version that the
                                        // rename/delete rule decides, as the other side deleted the rename's source.
                                        let (mode, id) = match resolve_tree_conflicts {
                                            Some(ResolveWith::VirtualMergeBase) => rename_delete_entry_in_merge_base(
                                                pick(side, their_tree, our_tree),
                                                location,
                                                (*our_mode, *our_id),
                                                (*source_entry_mode, *source_id),
                                            ),
                                            _ => (*our_mode, *our_id),
                                        };
                                        editor.remove_if_leaf(toc(source_location))?;
                                        pick_mut(side, our_tree, their_tree).remove_change(source_location.as_bstr());
                                        editor.upsert(toc(location), mode.kind(), id)?;
                                        editor.upsert(toc(add_location), their_mode.kind(), *their_id)?;
                                        ours_disposition = ChangeDisposition::Applied;
                                        theirs_disposition = ChangeDisposition::Applied;
                                    }
                                    Some(ResolveWith::Ours) => match side.to_global(outer_side) {
                                        Original => {
                                            editor.remove_if_leaf(toc(source_location))?;
                                            editor.upsert(toc(location), our_mode.kind(), *our_id)?;
                                            match side {
                                                Original => ours_disposition = ChangeDisposition::Applied,
                                                Swapped => theirs_disposition = ChangeDisposition::Applied,
                                            }
                                        }
                                        Swapped => {
                                            editor.remove_if_leaf(toc(source_location))?;
                                            editor.upsert(toc(add_location), their_mode.kind(), *their_id)?;
                                            match side {
                                                Original => theirs_disposition = ChangeDisposition::Applied,
                                                Swapped => ours_disposition = ChangeDisposition::Applied,
                                            }
                                        }
                                    },
                                    Some(ResolveWith::Ancestor) => {}
                                }
                            }
                            // Unrelated renames form a file/directory conflict at their destinations. Keep the directory
                            // at its intended path and relocate the blocking file rename. For example, ours renames `a`
                            // to `dst` while theirs renames `b` to `dst/child`.
                            (
                                Change::Rewrite {
                                    source_location: blocking_source,
                                    entry_mode: blocking_mode,
                                    id: blocking_id,
                                    location: blocking_location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location: nested_source,
                                    ..
                                },
                            ) if blocking_source != nested_source
                                && matches!(match_kind, Some(MatchKind::EraseLeaf)) =>
                            {
                                // These are unrelated renames whose destinations form a file/directory
                                // conflict. Keep the directory at its intended path and move the blocking file.
                                let renamed_location = unique_path_in_tree(
                                    blocking_location.as_bstr(),
                                    &editor,
                                    our_tree,
                                    labels.current.unwrap_or_default(),
                                )?;
                                let conflict = Conflict::without_resolution(
                                    ResolutionFailure::OursDirectoryTheirsNonDirectoryTheirsRenamed {
                                        renamed_unique_path_of_theirs: renamed_location.clone(),
                                    },
                                    (ours, theirs, Swapped, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            blocking_mode,
                                            blocking_id,
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                );

                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(blocking_source))?;
                                        editor.remove_if_leaf(toc(blocking_location))?;
                                        our_tree.remove_change(blocking_location.as_bstr());
                                        editor.upsert(toc(&renamed_location), blocking_mode.kind(), *blocking_id)?;
                                        apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        ours_disposition = ChangeDisposition::Applied;
                                    }
                                    Some(ResolveWith::Ours) => match outer_side {
                                        Original => {
                                            apply_change_and_mark(&mut editor, ours, &mut ours_disposition)?;
                                        }
                                        Swapped => {
                                            apply_change_and_mark(&mut editor, theirs, &mut theirs_disposition)?;
                                        }
                                    },
                                    Some(ResolveWith::Ancestor) => {}
                                }

                                if should_fail_on_conflict(conflict) {
                                    break 'outer;
                                }
                            }
                            // Both sides rename the same blob, symlink, or unchanged gitlink and merge blob content changes
                            // once. If both rename `file` to `dst`, schedule one entry there; if ours renames it to `ours`
                            // and theirs to `theirs`, report rename/rename and schedule it at both destinations. Inferred
                            // directory renames are applied first, so `old/file` may become `new/file` before comparison.
                            (
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location: our_location,
                                    ..
                                },
                                Change::Rewrite {
                                    entry_mode: their_mode,
                                    id: their_id,
                                    location: their_location,
                                    ..
                                },
                            ) if ((our_mode.is_blob_or_symlink() && their_mode.is_blob_or_symlink())
                                || (our_mode.is_commit() && their_mode.is_commit() && our_id == their_id))
                                && modes_are_compatible(*our_mode, *their_mode) =>
                            {
                                let (merged_mode, modes_merged_cleanly) =
                                    merge_modes(*our_mode, *their_mode, Some(*source_entry_mode), outer_side);
                                let (merged_blob_id, mut resolution) = if our_id == their_id {
                                    (*our_id, identical_content_resolution(modes_merged_cleanly))
                                } else {
                                    let (id, resolution) = perform_blob_merge(
                                        labels,
                                        objects,
                                        blob_merge,
                                        &mut diff_state.buf1,
                                        &mut write_blob_to_odb,
                                        (our_location, *our_id, *our_mode),
                                        (their_location, *their_id, *their_mode),
                                        (source_location, *source_id, *source_entry_mode),
                                        (u8::from(our_location != their_location), outer_side),
                                        &options,
                                    )?;
                                    let resolution = content_resolution(resolution, modes_merged_cleanly);
                                    (id, Some(resolution))
                                };

                                let their_location =
                                    possibly_rewritten_location(our_tree, their_location.as_bstr(), our_changes)
                                        .map_or(Cow::Borrowed(their_location.as_bstr()), Cow::Owned);
                                let our_location =
                                    possibly_rewritten_location(their_tree, our_location.as_bstr(), their_changes)
                                        .map_or(Cow::Borrowed(our_location.as_bstr()), Cow::Owned);

                                // Both sides renamed the source away. Only renames to two destinations conflict, and
                                // only the resolution of that conflict with the ancestor keeps the source.
                                if our_location == their_location
                                    || matches!(
                                        resolve_tree_conflicts,
                                        None | Some(ResolveWith::Ours | ResolveWith::VirtualMergeBase)
                                    )
                                {
                                    editor.remove_if_leaf(toc(source_location))?;
                                    our_tree.remove_change(source_location.as_bstr());
                                    their_tree.remove_change(source_location.as_bstr());
                                }

                                let (our_addition, their_addition) = if our_location == their_location {
                                    (
                                        None,
                                        Some(Change::Addition {
                                            location: our_location.into_owned(),
                                            relation: None,
                                            entry_mode: merged_mode,
                                            id: merged_blob_id,
                                        }),
                                    )
                                } else {
                                    if should_fail_on_conflict(Conflict::without_resolution(
                                        ResolutionFailure::OursRenamedTheirsRenamedDifferently {
                                            merged_blob: resolution.take().map(|resolution| ContentMerge {
                                                resolution,
                                                merged_blob_id,
                                            }),
                                        },
                                        (ours, theirs, Original, outer_side),
                                        [
                                            index_entry_at_path(
                                                source_entry_mode,
                                                source_id,
                                                ConflictIndexEntryPathHint::Source,
                                            ),
                                            index_entry_at_path(
                                                &merged_mode,
                                                &merged_blob_id,
                                                ConflictIndexEntryPathHint::Current,
                                            ),
                                            index_entry_at_path(
                                                &merged_mode,
                                                &merged_blob_id,
                                                ConflictIndexEntryPathHint::RenamedOrTheirs,
                                            ),
                                        ],
                                    )) {
                                        break 'outer;
                                    }
                                    match resolve_tree_conflicts {
                                        None | Some(ResolveWith::VirtualMergeBase) => {
                                            let our_addition = Change::Addition {
                                                location: our_location.into_owned(),
                                                relation: None,
                                                entry_mode: merged_mode,
                                                id: merged_blob_id,
                                            };
                                            let their_addition = Change::Addition {
                                                location: their_location.into_owned(),
                                                relation: None,
                                                entry_mode: merged_mode,
                                                id: merged_blob_id,
                                            };
                                            (Some(our_addition), Some(their_addition))
                                        }
                                        Some(ResolveWith::Ancestor) => (None, None),
                                        Some(ResolveWith::Ours) => {
                                            let our_addition = Change::Addition {
                                                location: match outer_side {
                                                    Original => our_location,
                                                    Swapped => their_location,
                                                }
                                                .into_owned(),
                                                relation: None,
                                                entry_mode: merged_mode,
                                                id: merged_blob_id,
                                            };
                                            (Some(our_addition), None)
                                        }
                                    }
                                };

                                // As merge-ort does for renames to one destination, a conflict there has the source's
                                // version and each side's version at the destination, and no stage at the source.
                                if let Some(resolution) = resolution
                                    && should_fail_on_conflict(Conflict::with_resolution(
                                        Resolution::OursModifiedTheirsModifiedThenBlobContentMerge {
                                            merged_blob: ContentMerge {
                                                resolution,
                                                merged_blob_id,
                                            },
                                        },
                                        (ours, theirs, Original, outer_side),
                                        [
                                            index_entry(source_entry_mode, source_id),
                                            index_entry(our_mode, our_id),
                                            index_entry(their_mode, their_id),
                                        ],
                                    ))
                                {
                                    break 'outer;
                                }
                                if let Some(addition) = our_addition {
                                    push_deferred((addition, Some(theirs_idx)), our_changes);
                                }
                                if let Some(addition) = their_addition {
                                    push_deferred((addition, Some(ours_idx)), their_changes);
                                }
                            }
                            // Rename/delete: remove the common source and report the conflict, then defer the renamed
                            // entry at its destination (adjusted for a containing directory rename) when policy keeps it.
                            // For example, ours deletes `dir/file` while theirs renames it to `dir/moved`; if ours also
                            // renames containing `dir` to `new`, that destination is adjusted to `new/moved`.
                            (
                                Change::Deletion { .. },
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: rewritten_mode,
                                    id: rewritten_id,
                                    location,
                                    ..
                                },
                            )
                            | (
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: rewritten_mode,
                                    id: rewritten_id,
                                    location,
                                    ..
                                },
                                Change::Deletion { .. },
                            ) => {
                                let side = if matches!(ours, Change::Deletion { .. }) {
                                    Original
                                } else {
                                    Swapped
                                };

                                match resolve_tree_conflicts {
                                    None | Some(ResolveWith::Ours | ResolveWith::VirtualMergeBase) => {
                                        editor.remove_if_leaf(toc(source_location))?;
                                        pick_mut(side, our_tree, their_tree).remove_change(source_location.as_bstr());
                                        match side {
                                            Original => ours_disposition = ChangeDisposition::Applied,
                                            Swapped => theirs_disposition = ChangeDisposition::Applied,
                                        }
                                    }
                                    Some(ResolveWith::Ancestor) => {}
                                }

                                let their_rewritten_location = possibly_rewritten_location(
                                    pick_mut(side, our_tree, their_tree),
                                    location.as_ref(),
                                    pick(side, our_changes, their_changes),
                                )
                                .unwrap_or_else(|| location.to_owned());
                                // A merged merge-base keeps the source's ancestor version at the destination, unless
                                // the deleting side has an entry there to merge with, as merge-ort does at that depth.
                                let (entry_mode, id) = match resolve_tree_conflicts {
                                    Some(ResolveWith::VirtualMergeBase) => rename_delete_entry_in_merge_base(
                                        pick(side, our_tree, their_tree),
                                        &their_rewritten_location,
                                        (*rewritten_mode, *rewritten_id),
                                        (*source_entry_mode, *source_id),
                                    ),
                                    _ => (*rewritten_mode, *rewritten_id),
                                };
                                let our_addition = Change::Addition {
                                    location: their_rewritten_location,
                                    relation: None,
                                    entry_mode,
                                    id,
                                };

                                if should_fail_on_conflict(Conflict::without_resolution(
                                    ResolutionFailure::OursDeletedTheirsRenamed,
                                    (ours, theirs, side, outer_side),
                                    [
                                        None,
                                        None,
                                        index_entry_at_path(
                                            rewritten_mode,
                                            rewritten_id,
                                            ConflictIndexEntryPathHint::RenamedOrTheirs,
                                        ),
                                    ],
                                )) {
                                    break 'outer;
                                }

                                let ours_is_rewrite = side.to_global(outer_side).is_swapped();
                                if matches!(resolve_tree_conflicts, None | Some(ResolveWith::VirtualMergeBase))
                                    || (matches!(resolve_tree_conflicts, Some(ResolveWith::Ours)) && ours_is_rewrite)
                                {
                                    push_deferred((our_addition, None), pick_mut(side, their_changes, our_changes));
                                }
                            }
                            // Rename/add: if directory `old` moves to `new` while the other side adds a file at vacated
                            // `old`, leaf changes already represent the result. Otherwise, two gitlinks require a
                            // submodule-aware follow-up, compatible blobs are content-merged, and entries of distinct
                            // types are both kept, placed as for add/add.
                            (
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                                Change::Addition {
                                    id: their_id,
                                    entry_mode: their_mode,
                                    location: add_location,
                                    ..
                                },
                            )
                            | (
                                Change::Addition {
                                    id: their_id,
                                    entry_mode: their_mode,
                                    location: add_location,
                                    ..
                                },
                                Change::Rewrite {
                                    source_location,
                                    source_entry_mode,
                                    source_id,
                                    entry_mode: our_mode,
                                    id: our_id,
                                    location,
                                    ..
                                },
                            ) => {
                                let side = if matches!(ours, Change::Rewrite { .. }) {
                                    Original
                                } else {
                                    Swapped
                                };
                                if our_mode.is_commit() && their_mode.is_commit() {
                                    // Keep Git's provisional global-ours tree while leaving the conflict unresolved.
                                    // `apply_change()` also removes the source when global ours is the rewrite.
                                    match resolve_tree_conflicts {
                                        None | Some(ResolveWith::Ours) => apply_our_resolution(
                                            ours,
                                            theirs,
                                            outer_side,
                                            &mut editor,
                                            &mut ours_disposition,
                                            &mut theirs_disposition,
                                        )?,
                                        Some(ResolveWith::Ancestor) => {}
                                        // A merged merge-base has neither entry, as merge-ort's two-way gitlink merge
                                        // takes the absent ancestor, while the rename's source is removed as at any
                                        // depth. The rename is already at `location` if it was applied before.
                                        Some(ResolveWith::VirtualMergeBase) => {
                                            editor.remove_if_leaf(toc(source_location))?;
                                            pick_mut(side, our_tree, their_tree)
                                                .remove_change(source_location.as_bstr());
                                            editor.remove_if_leaf(toc(location))?;
                                        }
                                    }
                                    if should_fail_on_conflict(Conflict::without_resolution(
                                        ResolutionFailure::SubmoduleAddAdd,
                                        (ours, theirs, side, outer_side),
                                        [None, index_entry(our_mode, our_id), index_entry(their_mode, their_id)],
                                    )) {
                                        break 'outer;
                                    }
                                } else if our_mode.is_tree() && add_location == source_location {
                                    // The leaf changes already represent the directory rename and its replacement.
                                    // Keep the replacement at the explicit source instead of relocating it with
                                    // the inferred directory rename and reporting a second conflict.
                                    match resolve_tree_conflicts {
                                        None | Some(ResolveWith::VirtualMergeBase) => {
                                            editor.upsert(toc(add_location), their_mode.kind(), *their_id)?;
                                            ours_disposition = ChangeDisposition::Applied;
                                            theirs_disposition = ChangeDisposition::Applied;
                                        }
                                        Some(ResolveWith::Ours) => {
                                            apply_our_resolution(
                                                ours,
                                                theirs,
                                                outer_side,
                                                &mut editor,
                                                &mut ours_disposition,
                                                &mut theirs_disposition,
                                            )?;
                                        }
                                        Some(ResolveWith::Ancestor) => {}
                                    }
                                } else if modes_are_compatible(*our_mode, *their_mode) {
                                    // The rename destination and the addition have no common ancestor entry. The `our_*`
                                    // bindings belong to the rename, which `side` maps to its global side for the modes, the
                                    // content merge and the conflict's index entries alike.
                                    let (merged_mode, modes_merged_cleanly) =
                                        merge_modes(*our_mode, *their_mode, None, side.to_global(outer_side));
                                    let (merged_blob_id, resolution) = if our_id == their_id {
                                        (*our_id, identical_content_resolution(modes_merged_cleanly))
                                    } else {
                                        let (id, resolution) = perform_blob_merge(
                                            labels,
                                            objects,
                                            blob_merge,
                                            &mut diff_state.buf1,
                                            &mut write_blob_to_odb,
                                            (location, *our_id, *our_mode),
                                            (location, *their_id, *their_mode),
                                            (source_location, source_id.kind().null(), *source_entry_mode),
                                            (0, side.to_global(outer_side)),
                                            &options,
                                        )?;
                                        let resolution = content_resolution(resolution, modes_merged_cleanly);
                                        (id, Some(resolution))
                                    };

                                    editor.remove_if_leaf(toc(source_location))?;
                                    pick_mut(side, our_tree, their_tree).remove_change(source_location.as_bstr());

                                    if let Some(resolution) = resolution
                                        && should_fail_on_conflict(Conflict::with_resolution(
                                            Resolution::OursModifiedTheirsModifiedThenBlobContentMerge {
                                                merged_blob: ContentMerge {
                                                    resolution,
                                                    merged_blob_id,
                                                },
                                            },
                                            (ours, theirs, side, outer_side),
                                            [None, index_entry(our_mode, our_id), index_entry(their_mode, their_id)],
                                        ))
                                    {
                                        break 'outer;
                                    }

                                    // Because this constellation can only be found by the lookup tree, there is
                                    // no need to put it as addition, we know it's not going to intersect on the other side.
                                    editor.upsert(toc(location), merged_mode.kind(), merged_blob_id)?;
                                } else {
                                    // Entries of distinct types are both kept, placed as Git does. Git removes
                                    // the rename source in any case. The rename itself is already at `location`
                                    // if it was applied before the addition was paired with it.
                                    editor.remove_if_leaf(toc(source_location))?;
                                    pick_mut(side, our_tree, their_tree).remove_change(source_location.as_bstr());

                                    // `our_*` belong to the rename and `their_*` to the addition, while the
                                    // placement is decided for the local sides.
                                    let (local_ours, local_theirs) = match side {
                                        Original => ((*our_mode, *our_id), (*their_mode, *their_id)),
                                        Swapped => ((*their_mode, *their_id), (*our_mode, *our_id)),
                                    };
                                    let placement = DistinctTypes::new(
                                        location,
                                        local_ours,
                                        local_theirs,
                                        &editor,
                                        (&*our_tree, &*their_tree),
                                        labels,
                                    )?;
                                    let (our_unique_location, their_unique_location) =
                                        placement.unique_locations(outer_side);
                                    let conflict = Conflict::without_resolution(
                                        ResolutionFailure::OursAddedTheirsAddedTypeMismatch {
                                            our_unique_location,
                                            their_unique_location,
                                        },
                                        (ours, theirs, Original, outer_side),
                                        placement.index_entries(None, outer_side),
                                    );
                                    match resolve_tree_conflicts {
                                        None => placement.apply(
                                            &mut editor,
                                            (&mut *our_tree, &mut *their_tree),
                                            (&mut *our_changes, &mut *their_changes),
                                            (&mut ours_disposition, &mut theirs_disposition),
                                            (ours_idx, theirs_idx),
                                        )?,
                                        // A directory keeps its path at every depth, and the other entry moves.
                                        Some(ResolveWith::VirtualMergeBase) if placement.involves_directory() => {
                                            placement.apply(
                                                &mut editor,
                                                (&mut *our_tree, &mut *their_tree),
                                                (&mut *our_changes, &mut *their_changes),
                                                (&mut ours_disposition, &mut theirs_disposition),
                                                (ours_idx, theirs_idx),
                                            )?;
                                        }
                                        Some(ResolveWith::Ours) => {
                                            let (mode, id) = match outer_side {
                                                Original => local_ours,
                                                Swapped => local_theirs,
                                            };
                                            editor.upsert(toc(location), mode.kind(), id)?;
                                        }
                                        // Keep neither entry, removing the rename if it is already applied. A merged
                                        // merge-base has neither, as merge-ort keeps the absent ancestor version of
                                        // entries of distinct types at that depth.
                                        Some(ResolveWith::Ancestor | ResolveWith::VirtualMergeBase) => {
                                            editor.remove_if_leaf(toc(location))?;
                                        }
                                    }

                                    if should_fail_on_conflict(conflict) {
                                        break 'outer;
                                    }
                                }
                            }
                            // Defensive, best-effort fallback for a concrete pair without a specialized resolution.
                            // Leave the ancestor in place by default, or apply ours if explicitly requested, and record
                            // `Unknown` instead of aborting the merge. Log it so a tested combination can gain an explicit
                            // arm and classification.
                            _unknown => {
                                gix_trace::warn!(
                                    ?ours,
                                    ?theirs,
                                    ?match_kind,
                                    "Concrete tree-merge pair has no specialized conflict classification"
                                );
                                if let Some(ResolveWith::Ours) = resolve_tree_conflicts {
                                    apply_our_resolution(
                                        ours,
                                        theirs,
                                        outer_side,
                                        &mut editor,
                                        &mut ours_disposition,
                                        &mut theirs_disposition,
                                    )?;
                                }
                                if should_fail_on_conflict(Conflict::unknown((ours, theirs, Original, outer_side))) {
                                    break 'outer;
                                }
                            }
                        }
                        their_changes[theirs_idx].mark(theirs_disposition);
                        our_changes[ours_idx].mark(ours_disposition);
                    }
                }
            }
            batch_start = batch_end;
            batch_end = their_changes.len();
        }

        ((our_changes, our_tree), (their_changes, their_tree)) = ((their_changes, their_tree), (our_changes, our_tree));
        (labels.current, labels.other) = (labels.other, labels.current);
        outer_side = outer_side.swapped();
    }

    Ok(Outcome {
        tree: editor,
        conflicts,
        failed_on_first_unresolved_conflict: failed_on_first_conflict,
    })
}

fn apply_change_and_mark(
    editor: &mut tree::Editor<'_>,
    change: &Change,
    disposition: &mut ChangeDisposition,
) -> Result {
    apply_change(editor, change, None)?;
    *disposition = ChangeDisposition::Applied;
    Ok(())
}

fn apply_our_resolution(
    local_ours: &Change,
    local_theirs: &Change,
    outer_side: ConflictMapping,
    editor: &mut gix_object::tree::Editor<'_>,
    local_ours_disposition: &mut ChangeDisposition,
    local_theirs_disposition: &mut ChangeDisposition,
) -> Result {
    let (ours, disposition) = match outer_side {
        Original => (local_ours, local_ours_disposition),
        Swapped => (local_theirs, local_theirs_disposition),
    };
    apply_change_and_mark(editor, ours, disposition)
}

fn involves_submodule(a: &EntryMode, b: &EntryMode) -> bool {
    a.is_commit() || b.is_commit()
}

/// Return `true` if entries with modes `a` and `b` can be merged as they have the same type, allowing blobs to differ in
/// their executable bit.
fn modes_are_compatible(a: EntryMode, b: EntryMode) -> bool {
    a.kind() == b.kind() || (a.is_blob() && b.is_blob())
}

/// Merge the modes `a` and `b` of [compatible](modes_are_compatible()) entries as Git does, and return the merged mode
/// along with `true` if the modes merged cleanly.
///
/// `ancestor` is the mode of the entry both sides changed, or `None` if both added their entry, and `a_side` is
/// `Original` if `a` belongs to *our* side, or `Swapped` if it belongs to *their* side. A mode that only one side changed
/// wins. Blobs with different executable bits whose ancestor is missing or not a blob keep *our* mode, and their merge
/// conflicts.
fn merge_modes(a: EntryMode, b: EntryMode, ancestor: Option<EntryMode>, a_side: ConflictMapping) -> (EntryMode, bool) {
    let (ours, theirs) = match a_side {
        Original => (a, b),
        Swapped => (b, a),
    };
    let ancestor = ancestor.map(|mode| mode.kind());
    if ours.kind() == theirs.kind() || Some(ours.kind()) == ancestor {
        (theirs, true)
    } else {
        (ours, Some(theirs.kind()) == ancestor)
    }
}

/// Return the `resolution` of a content merge if the modes of the merged entries merged cleanly, or a conflict otherwise.
///
/// Like Git, report conflicting executable bits as a conflicting content merge, which no conflict resolution strategy of
/// the content merge resolves.
fn content_resolution(resolution: crate::blob::Resolution, modes_merged_cleanly: bool) -> crate::blob::Resolution {
    if modes_merged_cleanly {
        resolution
    } else {
        crate::blob::Resolution::Conflict
    }
}

/// Return the resolution of merged entries with identical content: none if their modes merged cleanly, as nothing needed
/// merging, or a conflicting content merge otherwise, as Git reports conflicting executable bits.
fn identical_content_resolution(modes_merged_cleanly: bool) -> Option<crate::blob::Resolution> {
    (!modes_merged_cleanly).then_some(crate::blob::Resolution::Conflict)
}

fn push_deferred(change_and_idx: (Change, Option<usize>), changes: &mut ChangeList) {
    push_deferred_with_rewrite(change_and_idx, None, changes);
}

fn push_deferred_with_rewrite(
    (change, ours_idx): (Change, Option<usize>),
    new_location: Option<(BString, usize)>,
    changes: &mut ChangeList,
) {
    changes.push(TrackedChange::new(change, Some(ours_idx), new_location));
}

fn pick<'a, T: ?Sized>(side: ConflictMapping, ours: &'a T, theirs: &'a T) -> &'a T {
    match side {
        Original => ours,
        Swapped => theirs,
    }
}

fn pick_mut<'a, T: ?Sized>(side: ConflictMapping, ours: &'a mut T, theirs: &'a mut T) -> &'a mut T {
    match side {
        Original => ours,
        Swapped => theirs,
    }
}

/// Return the entry that a merged merge-base keeps at `location`, the destination of a rename of `renamed` whose source,
/// with the ancestor entry `source`, the other side deleted. As Git's `merge-ort` does when it merges merge-bases, that's
/// `source`, unless `deleting_side_tree` has an entry at `location` too, which `renamed` is then merged with like an
/// addition.
fn rename_delete_entry_in_merge_base(
    deleting_side_tree: &TreeNodes,
    location: &BString,
    renamed: (EntryMode, ObjectId),
    source: (EntryMode, ObjectId),
) -> (EntryMode, ObjectId) {
    if matches!(
        deleting_side_tree.check_conflict(location.as_bstr()),
        Some(PossibleConflict::Match { .. })
    ) {
        renamed
    } else {
        source
    }
}

fn index_entry(mode: &gix_object::tree::EntryMode, id: &gix_hash::ObjectId) -> Option<ConflictIndexEntry> {
    Some(ConflictIndexEntry {
        mode: *mode,
        id: *id,
        path_hint: None,
    })
}

fn index_entry_at_path(
    mode: &gix_object::tree::EntryMode,
    id: &gix_hash::ObjectId,
    hint: ConflictIndexEntryPathHint,
) -> Option<ConflictIndexEntry> {
    Some(ConflictIndexEntry {
        mode: *mode,
        id: *id,
        path_hint: Some(hint),
    })
}

/// Two entries of distinct types that want the same path, and where each of them goes, as Git's `merge-ort` decides in
/// `process_entry()`.
///
/// A directory keeps the path and the other entry moves. Otherwise the regular file moves and the other entry, a
/// symlink or a submodule, keeps the path; if neither is a regular file, both move. An entry that moves goes to a
/// unique path named after its side, such as `item~ours` for `item`.
struct DistinctTypes {
    /// The path that both entries want.
    location: BString,
    /// The entry of the local *ours* side.
    ours: PlacedEntry,
    /// The entry of the local *theirs* side.
    theirs: PlacedEntry,
}

/// One of the two entries of [`DistinctTypes`].
struct PlacedEntry {
    mode: EntryMode,
    id: ObjectId,
    /// The unique path that this entry moves to, or `None` if it keeps the contested path.
    unique_location: Option<BString>,
}

impl DistinctTypes {
    /// Place the local *ours* entry `(our_mode, our_id)` and the local *theirs* entry
    /// `(their_mode, their_id)`, which both want `location`. An entry that moves is named after its side
    /// in `labels`, at a path that neither `editor` nor the path tree of its side uses.
    fn new(
        location: &BString,
        (our_mode, our_id): (EntryMode, ObjectId),
        (their_mode, their_id): (EntryMode, ObjectId),
        editor: &tree::Editor<'_>,
        (our_tree, their_tree): (&TreeNodes, &TreeNodes),
        labels: crate::blob::builtin_driver::text::Labels<'_>,
    ) -> Result<Self> {
        let (ours_moves, theirs_moves) = if our_mode.is_tree() {
            (false, true)
        } else if their_mode.is_tree() {
            (true, false)
        } else {
            // The types differ, so at most one of the entries is a regular file.
            (
                our_mode.is_blob() || !their_mode.is_blob(),
                their_mode.is_blob() || !our_mode.is_blob(),
            )
        };
        let our_unique_location = ours_moves
            .then(|| unique_path_in_tree(location.as_bstr(), editor, our_tree, labels.current.unwrap_or_default()))
            .transpose()?;
        // If both entries move, sides with equal names would otherwise get the same path.
        let their_unique_location = theirs_moves
            .then(|| {
                unique_path_in_tree_avoiding(
                    location.as_bstr(),
                    editor,
                    their_tree,
                    labels.other.unwrap_or_default(),
                    our_unique_location.as_ref().map(|path| path.as_bstr()),
                )
            })
            .transpose()?;
        Ok(DistinctTypes {
            location: location.clone(),
            ours: PlacedEntry {
                mode: our_mode,
                id: our_id,
                unique_location: our_unique_location,
            },
            theirs: PlacedEntry {
                mode: their_mode,
                id: their_id,
                unique_location: their_unique_location,
            },
        })
    }

    /// Return the unique locations of the entries of the global *ours* and *theirs* sides, which `outer_side`
    /// maps to the local sides.
    fn unique_locations(&self, outer_side: ConflictMapping) -> (Option<BString>, Option<BString>) {
        let (ours, theirs) = match outer_side {
            Original => (&self.ours, &self.theirs),
            Swapped => (&self.theirs, &self.ours),
        };
        (ours.unique_location.clone(), theirs.unique_location.clone())
    }

    /// Return `true` if one of the entries is a directory, which keeps the path at every depth of a recursive merge.
    fn involves_directory(&self) -> bool {
        self.ours.mode.is_tree() || self.theirs.mode.is_tree()
    }

    /// Return the index entry of the base `(mode, id)` if either entry has the base's type, in which case it
    /// stays with that entry.
    fn base_entry(&self, (mode, id): (EntryMode, ObjectId), outer_side: ConflictMapping) -> Option<ConflictIndexEntry> {
        [(Original, &self.ours), (Swapped, &self.theirs)]
            .into_iter()
            .find(|(_, entry)| modes_are_compatible(entry.mode, mode))
            .and_then(|(local_side, _)| {
                index_entry_at_path(&mode, &id, type_mismatch_path_hint(local_side, outer_side))
            })
    }

    /// Return `base` and the entries of the local *ours* and *theirs* sides as the index entries of a conflict
    /// between the local changes, which `outer_side` maps to the global sides.
    fn index_entries(
        &self,
        base: Option<ConflictIndexEntry>,
        outer_side: ConflictMapping,
    ) -> [Option<ConflictIndexEntry>; 3] {
        [
            base,
            index_entry_at_path(
                &self.ours.mode,
                &self.ours.id,
                type_mismatch_path_hint(Original, outer_side),
            ),
            index_entry_at_path(
                &self.theirs.mode,
                &self.theirs.id,
                type_mismatch_path_hint(Swapped, outer_side),
            ),
        ]
    }

    /// Put both entries into the result. An entry that keeps the contested path is written there, and the path is
    /// removed if both entries move. An entry that moves leaves the path tree of its side and is deferred as an
    /// addition at its unique path to the change list of its side, to be matched against the other side there.
    ///
    /// `ours_idx` and `theirs_idx` index the changes of the two entries in `our_changes` and `their_changes`. A
    /// deferred addition ignores the change of the other entry when it is matched, so the pair can't meet again,
    /// for example where the unique path is the source of the other side's rename.
    fn apply(
        self,
        editor: &mut tree::Editor<'_>,
        (our_tree, their_tree): (&mut TreeNodes, &mut TreeNodes),
        (our_changes, their_changes): (&mut ChangeList, &mut ChangeList),
        (ours_disposition, theirs_disposition): (&mut ChangeDisposition, &mut ChangeDisposition),
        (ours_idx, theirs_idx): (usize, usize),
    ) -> Result {
        if self.ours.unique_location.is_some() && self.theirs.unique_location.is_some() {
            editor.remove_if_leaf(to_components(self.location.as_bstr()))?;
        }
        self.ours.apply(
            &self.location,
            editor,
            our_tree,
            our_changes,
            ours_disposition,
            theirs_idx,
        )?;
        self.theirs.apply(
            &self.location,
            editor,
            their_tree,
            their_changes,
            theirs_disposition,
            ours_idx,
        )
    }
}

impl PlacedEntry {
    /// Write this entry to `editor` at the contested `location` if it keeps that path. Otherwise remove it from
    /// `path_tree` at `location` and defer its addition at its unique path to `changes`, ignoring the change at
    /// `other_idx` of the other side when it is matched. `path_tree` and `changes` belong to the side of this entry.
    fn apply(
        self,
        location: &BString,
        editor: &mut tree::Editor<'_>,
        path_tree: &mut TreeNodes,
        changes: &mut ChangeList,
        disposition: &mut ChangeDisposition,
        other_idx: usize,
    ) -> Result {
        match self.unique_location {
            None => {
                editor.upsert(to_components(location.as_bstr()), self.mode.kind(), self.id)?;
                *disposition = ChangeDisposition::Applied;
            }
            Some(unique_location) => {
                path_tree.remove_change(location.as_bstr());
                push_deferred(
                    (
                        Change::Addition {
                            location: unique_location,
                            relation: None,
                            entry_mode: self.mode,
                            id: self.id,
                        },
                        Some(other_idx),
                    ),
                    changes,
                );
            }
        }
        Ok(())
    }
}

/// Return the hint that finds the index entry of `local_side` at the location that a type-mismatch failure records for
/// the global side, which `outer_side` maps it to: [`Current`](ConflictIndexEntryPathHint::Current) for *ours* and
/// [`RenamedOrTheirs`](ConflictIndexEntryPathHint::RenamedOrTheirs) for *theirs*.
fn type_mismatch_path_hint(local_side: ConflictMapping, outer_side: ConflictMapping) -> ConflictIndexEntryPathHint {
    match local_side.to_global(outer_side) {
        Original => ConflictIndexEntryPathHint::Current,
        Swapped => ConflictIndexEntryPathHint::RenamedOrTheirs,
    }
}
