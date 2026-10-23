//! ## About `debug_assert!()
//!
//! The idea is to have code that won't panic in production. Thus, if in production that assertion would fail,
//! we will rather let the code run and hope it will either be correct enough or fail in more graceful ways later.
//!
//! Once such a case becomes a bug and is reproduced in testing, the debug-assertion will kick in and hopefully
//! contribute to finding a fix faster.
use gix_error::Result;
use std::collections::HashMap;

use bstr::{BStr, BString, ByteSlice, ByteVec};
use gix_diff::tree_with_rewrites::{Change, ChangeRef};
use gix_error::{OptionExt, ResultExt, message};
use gix_hash::ObjectId;
use gix_object::{
    tree,
    tree::{EntryKind, EntryMode},
};

use crate::{
    blob::{ResourceKind, builtin_driver::binary::Pick},
    tree::{
        Conflict, ConflictIndexEntry, ConflictIndexEntryPathHint, ConflictMapping, Options, Resolution,
        ResolutionFailure,
    },
};

/// Produce a side-qualified path for `file_path` like `a/b`, using `editor` and `tree` to assure uniqueness.
///
/// This normally keeps the file in its directory, as in `a/b~side`. If a non-tree component blocks that directory,
/// the blocker itself is qualified instead, as in `a~side/b`, because changing only the child name could never make
/// the path available.
pub fn unique_path_in_tree(
    file_path: &BStr,
    editor: &tree::Editor<'_>,
    tree: &TreeNodes,
    side_name: &BStr,
) -> Result<BString> {
    unique_path_in_tree_avoiding(file_path, editor, tree, side_name, None)
}

/// Like [`unique_path_in_tree()`], but never return `reserved`, a path already chosen for another entry that neither
/// `editor` nor `tree` contains yet.
///
/// When both entries of one path move, each to a path named after its side, equal side names, or names that are equal
/// once `/` is replaced, would otherwise give both entries the same path.
pub fn unique_path_in_tree_avoiding(
    file_path: &BStr,
    editor: &tree::Editor<'_>,
    tree: &TreeNodes,
    side_name: &BStr,
    reserved: Option<&BStr>,
) -> Result<BString> {
    let mut qualifier = BString::from("~");
    qualifier.extend(
        side_name
            .as_bytes()
            .iter()
            .copied()
            .map(|b| if b == b'/' { b'_' } else { b }),
    );

    let mut component_end = file_path.len();
    loop {
        let at_root = !file_path[..component_end].contains(&b'/');
        let mut suffix = None;
        loop {
            let mut buf = file_path[..component_end].to_owned();
            buf.extend_from_slice(&qualifier);
            if let Some(suffix) = suffix {
                buf.push_str(format!("_{suffix}"));
            }
            buf.extend_from_slice(&file_path[component_end..]);

            let conflict = tree.check_conflict(buf.as_bstr());
            if !at_root && matches!(conflict, Some(PossibleConflict::NonTreeToTree { .. })) {
                break;
            }
            if reserved.is_none_or(|reserved| reserved != buf.as_bstr())
                && editor.get(to_components_bstring_ref(&buf)).is_none()
                && conflict.is_none()
            {
                return Ok(buf);
            }
            suffix = Some(suffix.map_or(0, |suffix| suffix + 1));
        }

        component_end = file_path[..component_end]
            .iter()
            .rposition(|byte| *byte == b'/')
            .expect("a non-root component always has a preceding slash");
    }
}

/// Perform a merge between two blobs and return the result of its object id.
///
/// `pathnames` are the paths of the ancestor, ours and theirs that the conflict markers give after the labels if the
/// paths of ours and theirs differ, as merge-ort gives the `pathnames` of a conflict.
#[expect(clippy::too_many_arguments)]
pub fn perform_blob_merge(
    mut labels: crate::blob::builtin_driver::text::Labels<'_>,
    objects: &impl gix_object::FindObjectOrHeader,
    blob_merge: &mut crate::blob::Platform,
    buf: &mut Vec<u8>,
    write_blob_to_odb: &mut impl FnMut(&[u8]) -> Result<ObjectId>,
    (our_location, our_id, our_mode): (&BString, ObjectId, EntryMode),
    (their_location, their_id, their_mode): (&BString, ObjectId, EntryMode),
    (previous_location, previous_id, previous_mode): (&BString, ObjectId, EntryMode),
    [previous_pathname, our_pathname, their_pathname]: [&BStr; 3],
    (extra_markers, outer_side): (u8, ConflictMapping),
    options: &Options,
) -> Result<(ObjectId, crate::blob::Resolution)> {
    if our_id == their_id {
        // This can happen if the merge modes are different.
        debug_assert_ne!(
            our_mode, their_mode,
            "BUG: we must think anything has to be merged if the modes and the ids are the same"
        );
        return Ok((their_id, crate::blob::Resolution::Complete));
    }
    if matches!(our_mode.kind(), EntryKind::Link) && matches!(their_mode.kind(), EntryKind::Link) {
        let (pick, resolution) = crate::blob::builtin_driver::binary(options.symlink_conflicts);
        let (our_id, their_id) = match outer_side {
            ConflictMapping::Original => (our_id, their_id),
            ConflictMapping::Swapped => (their_id, our_id),
        };
        let id = match pick {
            Pick::Ancestor => previous_id,
            Pick::Ours => our_id,
            Pick::Theirs => their_id,
        };
        return Ok((id, resolution));
    }
    let (our_kind, their_kind) = match outer_side {
        ConflictMapping::Original => (ResourceKind::CurrentOrOurs, ResourceKind::OtherOrTheirs),
        ConflictMapping::Swapped => (ResourceKind::OtherOrTheirs, ResourceKind::CurrentOrOurs),
    };
    blob_merge.set_resource(our_id, our_mode.kind(), our_location.as_bstr(), our_kind, objects)?;
    blob_merge.set_resource(
        their_id,
        their_mode.kind(),
        their_location.as_bstr(),
        their_kind,
        objects,
    )?;
    blob_merge.set_resource(
        previous_id,
        previous_mode.kind(),
        previous_location.as_bstr(),
        ResourceKind::CommonAncestorOrBase,
        objects,
    )?;

    fn combined(side: &BStr, pathname: &BStr) -> BString {
        let mut buf = side.to_owned();
        buf.push_byte(b':');
        buf.push_str(pathname);
        buf
    }

    let (current_pathname, other_pathname) = if outer_side.is_swapped() {
        (labels.current, labels.other) = (labels.other, labels.current);
        (their_pathname, our_pathname)
    } else {
        (our_pathname, their_pathname)
    };

    let (ancestor, current, other);
    let labels = if our_pathname == their_pathname {
        labels
    } else {
        ancestor = labels.ancestor.map(|side| combined(side, previous_pathname));
        current = labels.current.map(|side| combined(side, current_pathname));
        other = labels.other.map(|side| combined(side, other_pathname));
        crate::blob::builtin_driver::text::Labels {
            ancestor: ancestor.as_ref().map(|n| n.as_bstr()),
            current: current.as_ref().map(|n| n.as_bstr()),
            other: other.as_ref().map(|n| n.as_bstr()),
        }
    };
    let mut prep = blob_merge.prepare_merge(objects, options.blob_merge)?;
    if let crate::blob::builtin_driver::text::Conflict::Keep { marker_size, .. } = &mut prep.options.text.conflict {
        *marker_size =
            marker_size.saturating_add(extra_markers.saturating_add(options.marker_size_multiplier.saturating_mul(2)));
    }
    let (pick, resolution) = prep.merge(buf, labels, &options.blob_merge_command_ctx)?;

    let write_error = || message("Failed to write merged blob content as blob to the object database");
    let merged_blob_id = match prep
        .id_by_pick(pick, buf, &mut *write_blob_to_odb)
        .or_raise(write_error)?
    {
        // As in Git, a merge of merge-bases reads an absent ancestor as empty content, and keeps it as the result.
        None if pick == crate::blob::platform::builtin_merge::Pick::Ancestor
            && options.blob_merge.is_virtual_ancestor =>
        {
            write_blob_to_odb(&[]).or_raise(write_error)?
        }
        id => id.ok_or_raise(|| crate::tree::Error::MissingBinaryMergeResult)?,
    };
    Ok((merged_blob_id, resolution))
}

/// A change from one side's base-to-side diff, together with its merge scheduling metadata.
///
/// Tree merge keeps each side's changes in a flat [`ChangeList`] and builds a path-based
/// [`TreeNodes`] index whose entries point back into that list. The [`ChangeState`] remains
/// on the list entry so a change can be found structurally even after it no longer needs to
/// be scheduled.
#[derive(Debug)]
pub struct TrackedChange {
    /// The actual change
    pub inner: Change,
    state: ChangeState,
    /// If `Some(ours_idx_to_ignore)`, this change must be placed into the tree before handling it.
    /// This makes sure that new changes aren't visible too early, which would mean the algorithm
    /// knows things too early which can be misleading.
    /// The `ours_idx_to_ignore` assures that the same rewrite won't be used as matching side, which
    /// would lead to strange effects. Only set if it's a rewrite though.
    pub needs_tree_insertion: Option<Option<usize>>,
}

/// The lifecycle of a [`TrackedChange`] while reconciling the two side-diffs.
///
/// The merge starts with an editor for the ancestor tree. It repeatedly takes a pending
/// change from one side, looks for a path or rename interaction in the other side's
/// [`TreeNodes`], and either applies the result to the editor or only records/resolves a
/// conflict. These outcomes must remain distinguishable:
///
/// | State | Process again? | Change effect represented in the editor? |
/// |-------|----------------|------------------------------------------|
/// | [`Pending`](ChangeState::Pending) | yes | no |
/// | [`Processed`](ChangeState::Processed) | no | no |
/// | [`Applied`](ChangeState::Applied) | no | yes |
///
/// Valid transitions are `Pending -> Processed`, `Pending -> Applied`, and
/// `Processed -> Applied`. In particular, [`TrackedChange::mark_processed()`] never
/// downgrades an already-applied change, while [`TrackedChange::mark_applied()`] may
/// upgrade a processed one.
///
/// This distinction matters for forced tree-conflict resolution. For example, resolving
/// with the ancestor can process a deletion without removing the ancestor entry. A later
/// addition below that path must still see the retained entry; treating "processed" as
/// "applied" would incorrectly suppress that tree/non-tree conflict.
#[derive(Debug, Clone, Copy)]
enum ChangeState {
    /// The change still has to be compared with the other side and handled.
    Pending,
    /// The change was handled, but its side-effect was not applied to the editor.
    Processed,
    /// The change was handled and its effect is represented in the editor.
    ///
    /// Tree changes begin in this state: they are structural matching nodes, while their
    /// effective contents are represented by the leaf changes that the algorithm schedules.
    Applied,
}

/// How handling a change affected the output editor.
#[derive(Debug, Clone, Copy)]
pub(super) enum ChangeDisposition {
    /// The change was consumed without applying its effect.
    Processed,
    /// The change's effect is represented in the editor.
    Applied,
}

impl TrackedChange {
    pub(super) fn new(inner: Change, needs_tree_insertion: Option<Option<usize>>) -> Self {
        TrackedChange {
            inner,
            state: ChangeState::Pending,
            needs_tree_insertion,
        }
    }

    /// Return whether this change still needs to be scheduled by the merge loop.
    pub(super) fn is_pending(&self) -> bool {
        matches!(self.state, ChangeState::Pending)
    }

    /// Return whether this change's effect is represented in the output editor.
    pub(super) fn was_applied(&self) -> bool {
        matches!(self.state, ChangeState::Applied)
    }

    /// Return whether this change was consumed without affecting the output editor.
    pub(super) fn was_processed_without_application(&self) -> bool {
        matches!(self.state, ChangeState::Processed)
    }

    /// Stop scheduling this change without downgrading it if it was already applied.
    pub(super) fn mark_processed(&mut self) {
        if matches!(self.state, ChangeState::Pending) {
            self.state = ChangeState::Processed;
        }
    }

    /// Record that this change's effect is represented in the output editor.
    pub(super) fn mark_applied(&mut self) {
        self.state = ChangeState::Applied;
    }

    /// Record the final disposition chosen while resolving this change.
    pub(super) fn mark(&mut self, disposition: ChangeDisposition) {
        match disposition {
            ChangeDisposition::Processed => self.mark_processed(),
            ChangeDisposition::Applied => self.mark_applied(),
        }
    }
}

pub type ChangeList = Vec<TrackedChange>;
pub type ChangeListRef = [TrackedChange];

/// Push `change` onto `changes` unless it is the modification of a tree.
///
/// Trees that were added or deleted stay as structural nodes for tree/non-tree conflicts.
pub fn track(change: ChangeRef<'_>, changes: &mut ChangeList) {
    if change.entry_mode().is_tree() && matches!(change, ChangeRef::Modification { .. }) {
        return;
    }
    let mut tracked = TrackedChange::new(change.into_owned(), None);
    if tracked.inner.entry_mode().is_tree() {
        // Tree changes are structural nodes used to detect tree/non-tree conflicts, not work items of their own.
        // Git has no empty directories, so descendant leaf changes carry every observable editor update
        // (`apply_change()` is a no-op for trees). Mark the tree applied to keep it available for matching
        // without scheduling it.
        tracked.mark_applied();
    }
    changes.push(tracked);
}

/// Unconditionally apply `change` to `editor`.
pub fn apply_change(editor: &mut tree::Editor<'_>, change: &Change) -> Result {
    use to_components_bstring_ref as to_components;
    if change.entry_mode().is_tree() {
        return Ok(());
    }

    let (location, mode, id) = match change {
        Change::Addition {
            location,
            entry_mode,
            id,
            ..
        }
        | Change::Modification {
            location,
            entry_mode,
            id,
            ..
        } => (location, entry_mode, id),
        Change::Deletion { location, .. } => {
            editor.remove_if_leaf(to_components(location))?;
            return Ok(());
        }
        Change::Rewrite {
            source_location,
            entry_mode,
            id,
            location,
            copy,
            ..
        } => {
            if !*copy {
                editor.remove_if_leaf(to_components(source_location))?;
            }
            (location, entry_mode, id)
        }
    };

    editor.upsert(to_components(location), mode.kind(), *id)?;
    Ok(())
}

/// A potential conflict that needs to be checked. It comes in several varieties and always happens
/// if paths overlap in some way between *theirs* and *ours*.
#[derive(Debug)]
pub enum PossibleConflict {
    /// *our* changes have a tree here, but *they* place a non-tree or edit an existing item (that we removed).
    TreeToNonTree {
        /// The possibly available change at this node.
        change_idx: Option<usize>,
    },
    /// A non-tree in *our* tree turned into a tree in *theirs* - this can be done with additions in *theirs*,
    /// or if we added a blob, while they added a directory.
    NonTreeToTree {
        /// The possibly available change at this node.
        change_idx: Option<usize>,
    },
    /// A perfect match, i.e. *our* change at `a/b/c` corresponds to *their* change at the same path.
    Match {
        /// The index to *our* change at *their* path.
        change_idx: usize,
    },
}

impl PossibleConflict {
    /// Return the index into the [`ChangeList`] from which this conflict tree was built.
    ///
    /// This is `None` for structural tree/non-tree conflicts if there is no change at the
    /// conflicting path itself, only one or more changes below it.
    pub(super) fn change_idx(&self) -> Option<usize> {
        match self {
            PossibleConflict::TreeToNonTree { change_idx, .. } | PossibleConflict::NonTreeToTree { change_idx, .. } => {
                *change_idx
            }
            PossibleConflict::Match { change_idx, .. } => Some(*change_idx),
        }
    }
}

/// The flat list of all tree-nodes so we can avoid having a linked-tree using pointers
/// which is useful for traversal and initial setup as that can then trivially be non-recursive.
pub struct TreeNodes(Vec<TreeNode>);

/// Trees lead to other trees, or leafs (without children).
#[derive(Debug, Default, Clone)]
struct TreeNode {
    /// A mapping of path components to their children to quickly see if `theirs` in some way is potentially
    /// conflicting with `ours`.
    children: HashMap<BString, usize>,
    /// The index to the change at this path. Leaf nodes (without children) always have one, and a node with
    /// children has one if a change occupies its path as well, like an added or deleted tree.
    change_idx: Option<usize>,
    /// Prefer non-tree changes if multiple changes occupy the same path.
    change_is_tree: bool,
}

impl TreeNode {
    fn is_leaf_node(&self) -> bool {
        self.children.is_empty()
    }
}

impl TreeNodes {
    pub fn new() -> Self {
        TreeNodes(vec![TreeNode::default()])
    }

    /// Insert our `change` at `change_idx`, into a linked-tree, assuring that each `change` is non-conflicting
    /// with this tree structure, i.e. each leaf path is only seen once.
    /// Note that directories can be added in between.
    pub fn track_change(&mut self, change: &Change, change_idx: usize) {
        for path in [
            Some(change.source_location()),
            match change {
                Change::Addition { .. } | Change::Deletion { .. } | Change::Modification { .. } => None,
                Change::Rewrite { location, .. } => Some(location.as_bstr()),
            },
        ]
        .into_iter()
        .flatten()
        {
            let mut components = to_components(path).peekable();
            let mut next_index = self.0.len();
            let mut cursor = &mut self.0[0];
            while let Some(component) = components.next() {
                let is_last = components.peek().is_none();
                match cursor.children.get(component).copied() {
                    None => {
                        let new_node = TreeNode {
                            children: Default::default(),
                            change_idx: is_last.then_some(change_idx),
                            change_is_tree: is_last && change.entry_mode().is_tree(),
                        };
                        cursor.children.insert(component.to_owned(), next_index);
                        self.0.push(new_node);
                        cursor = &mut self.0[next_index];
                        next_index += 1;
                    }
                    Some(index) => {
                        cursor = &mut self.0[index];
                        if is_last {
                            // NOTE: we might encounter the same path multiple times in rare conditions.
                            //       Prefer a non-tree change as it describes the actual leaf collision.
                            if (cursor.change_idx.is_none() && !cursor.is_leaf_node())
                                || (cursor.change_is_tree && !change.entry_mode().is_tree())
                            {
                                cursor.change_idx = Some(change_idx);
                                cursor.change_is_tree = change.entry_mode().is_tree();
                            }
                        }
                    }
                }
            }
        }
    }

    /// Search our indexed change paths for a structural overlap with `theirs_location`.
    ///
    /// Return the kind of exact-path or tree/non-tree overlap found, or `None` if the path does not interact
    /// with our indexed changes.
    pub fn check_conflict(&self, theirs_location: &BStr) -> Option<PossibleConflict> {
        if self.0[0].children.is_empty() {
            return None;
        }
        let mut cursor = &self.0[0];
        for component in to_components(theirs_location) {
            match cursor.children.get(component).copied() {
                // *their* change is outside *our* tree
                None => {
                    return (cursor.is_leaf_node() && !cursor.change_is_tree).then_some(
                        PossibleConflict::NonTreeToTree {
                            change_idx: cursor.change_idx,
                        },
                    );
                }
                Some(child_idx) => cursor = &self.0[child_idx],
            }
        }

        if cursor.is_leaf_node() {
            PossibleConflict::Match {
                change_idx: cursor.change_idx.expect("leaf nodes always have a change"),
            }
        } else {
            PossibleConflict::TreeToNonTree {
                change_idx: cursor.change_idx,
            }
        }
        .into()
    }

    pub fn remove_existing_change(&mut self, location: &BStr) {
        self.remove_change_inner(location, true);
    }

    pub fn remove_change(&mut self, location: &BStr) {
        self.remove_change_inner(location, false);
    }

    fn remove_change_inner(&mut self, location: &BStr, must_exist: bool) {
        let mut components = to_components(location).peekable();
        let mut cursor_idx = 0;
        let mut ancestry = Vec::new();
        while let Some(component) = components.next() {
            match self.0[cursor_idx].children.get(component).copied() {
                None => {
                    debug_assert!(!must_exist, "didn't find '{location}' for removal");
                    // The remaining components cannot belong to this path once a prefix is absent.
                    return;
                }
                Some(existing_idx) => {
                    ancestry.push((cursor_idx, component.to_owned(), existing_idx));
                    let is_last = components.peek().is_none();
                    if is_last {
                        let node = &mut self.0[existing_idx];
                        debug_assert!(!must_exist || node.change_idx.is_some(), "no change at '{location}'");
                        node.change_idx = None;
                        node.change_is_tree = false;
                    } else {
                        cursor_idx = existing_idx;
                    }
                }
            }
        }

        while let Some((parent_idx, component, child_idx)) = ancestry.pop() {
            let child = &self.0[child_idx];
            if child.change_idx.is_some() || !child.children.is_empty() {
                break;
            }
            self.0[parent_idx].children.remove(component.as_bstr());
        }
    }

    /// Insert the location of a change that the merge deferred into this tree.
    ///
    /// A deferred rewrite's source is already indexed by the change it came from, so only its destination is
    /// added.
    pub fn insert(&mut self, new_change: &Change, new_change_idx: usize) {
        let mut next_index = self.0.len();
        let mut cursor = &mut self.0[0];
        for component in to_components(new_change.location()) {
            match cursor.children.get(component).copied() {
                None => {
                    cursor.children.insert(component.to_owned(), next_index);
                    self.0.push(TreeNode::default());
                    cursor = &mut self.0[next_index];
                    next_index += 1;
                }
                Some(existing_idx) => {
                    cursor = &mut self.0[existing_idx];
                }
            }
        }

        cursor.change_idx = Some(new_change_idx);
        cursor.change_is_tree = new_change.entry_mode().is_tree();
    }
}

pub fn to_components_bstring_ref(rela_path: &BString) -> impl Iterator<Item = &BStr> {
    rela_path.split(|b| *b == b'/').map(Into::into)
}

pub fn to_components(rela_path: &BStr) -> impl Iterator<Item = &BStr> {
    rela_path.split(|b| *b == b'/').map(Into::into)
}

impl Conflict {
    pub(super) fn without_resolution(
        resolution: ResolutionFailure,
        changes: (&Change, &Change, ConflictMapping, ConflictMapping),
        entries: [Option<ConflictIndexEntry>; 3],
    ) -> Self {
        Conflict::maybe_resolved(Err(resolution), changes, entries)
    }

    pub(super) fn with_resolution(
        resolution: Resolution,
        changes: (&Change, &Change, ConflictMapping, ConflictMapping),
        entries: [Option<ConflictIndexEntry>; 3],
    ) -> Self {
        Conflict::maybe_resolved(Ok(resolution), changes, entries)
    }

    /// Record `ours` and `theirs`, the changes of the pass that found the conflict, as the changes of the merge's sides.
    ///
    /// `map` tells how the description of `resolution` and `entries` relates to the pass's sides, and `outer_map` how
    /// the pass's sides relate to the merge's sides: a pass processes the merge's *ours* as its *theirs* when
    /// `outer_map` is swapped.
    fn maybe_resolved(
        resolution: std::result::Result<Resolution, ResolutionFailure>,
        (ours, theirs, map, outer_map): (&Change, &Change, ConflictMapping, ConflictMapping),
        entries: [Option<ConflictIndexEntry>; 3],
    ) -> Self {
        let (ours, theirs) = match outer_map {
            ConflictMapping::Original => (ours, theirs),
            ConflictMapping::Swapped => (theirs, ours),
        };
        Conflict {
            resolution,
            ours: ours.clone(),
            theirs: theirs.clone(),
            entries,
            map: map.to_global(outer_map),
        }
    }

    pub(super) fn unknown(changes: (&Change, &Change, ConflictMapping, ConflictMapping)) -> Self {
        let (source_mode, source_id) = changes.0.source_entry_mode_and_id();
        let (our_mode, our_id) = changes.0.entry_mode_and_id();
        let (their_mode, their_id) = changes.1.entry_mode_and_id();
        let entries = [
            Some(ConflictIndexEntry {
                mode: source_mode,
                id: source_id.into(),
                path_hint: Some(ConflictIndexEntryPathHint::Source),
            }),
            Some(ConflictIndexEntry {
                mode: our_mode,
                id: our_id.into(),
                path_hint: Some(ConflictIndexEntryPathHint::Current),
            }),
            Some(ConflictIndexEntry {
                mode: their_mode,
                id: their_id.into(),
                path_hint: Some(ConflictIndexEntryPathHint::RenamedOrTheirs),
            }),
        ];
        Conflict::maybe_resolved(Err(ResolutionFailure::Unknown), changes, entries)
    }
}

#[cfg(test)]
mod tree_nodes_tests {
    use super::*;
    use gix_error::Result;

    #[test]
    fn removing_an_absent_nested_change_does_not_remove_a_matching_root_suffix() {
        let mut tree = TreeNodes::new();
        tree.0[0].children.insert("b".into(), 1);
        tree.0.push(TreeNode {
            change_idx: Some(42),
            ..Default::default()
        });

        tree.remove_change("a/b".into());
        assert!(
            matches!(
                tree.check_conflict("b".into()),
                Some(PossibleConflict::Match { change_idx: 42 })
            ),
            "a missing `a` prefix must stop removal before an unrelated root-level `b`"
        );
    }

    #[test]
    fn removing_a_change_prunes_empty_parent_nodes() {
        let mut tree = TreeNodes::new();
        tree.track_change(
            &Change::Addition {
                location: "e/e".into(),
                relation: None,
                entry_mode: EntryKind::Blob.into(),
                id: gix_hash::Kind::Sha1.null(),
            },
            0,
        );

        tree.remove_existing_change("e/e".into());
        assert!(
            tree.check_conflict("e".into()).is_none(),
            "an empty former parent isn't a leaf change or a path conflict"
        );
    }

    #[test]
    fn a_tracked_tree_without_tracked_children_does_not_occupy_paths_below_it() {
        let mut tree = TreeNodes::new();
        tree.track_change(
            &Change::Addition {
                location: "dir".into(),
                relation: None,
                entry_mode: EntryKind::Tree.into(),
                id: gix_hash::Kind::Sha1.null(),
            },
            0,
        );

        assert!(
            !matches!(
                tree.check_conflict("dir/file~side".into()),
                Some(PossibleConflict::NonTreeToTree { .. })
            ),
            "a tracked tree permits children even if no child change is currently tracked"
        );
    }

    #[test]
    fn unique_path_qualifies_a_non_tree_parent_instead_of_looping_over_child_names() -> Result {
        let mut tree = TreeNodes::new();
        tree.track_change(
            &Change::Addition {
                location: "dir".into(),
                relation: None,
                entry_mode: EntryKind::Blob.into(),
                id: gix_hash::Kind::Sha1.null(),
            },
            0,
        );
        let editor = tree::Editor::new(
            gix_object::Tree::default(),
            &gix_object::find::Never,
            gix_hash::Kind::Sha1,
        );

        assert_eq!(
            unique_path_in_tree("dir/file".into(), &editor, &tree, "OURS".into())?,
            "dir~OURS/file",
            "the blocking path component itself must be moved aside"
        );
        Ok(())
    }
}
