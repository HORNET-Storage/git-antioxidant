//! The commits that a rebase replays, as `sequencer_make_script()` walks them with `git rev-list --right-only
//! --cherry-mark --topo-order --reverse <upstream>...<head>`.

use std::collections::{HashMap, HashSet};

use gix_error::Result;
use gix_hash::{ObjectId, oid};

use crate::{commits::Commits, merge_bases, patch_id};

/// How [`walk()`] walks, as the flags of `sequencer_make_script()` set up its revision walk.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Leave the commits whose patch is upstream unmarked, as `--reapply-cherry-picks` does. Otherwise they are marked
    /// as `--cherry-mark` marks them.
    pub reapply_cherry_picks: bool,
    /// Return merge commits too, for `--rebase-merges`. Otherwise only commits with at most one parent are returned, as
    /// `--max-parents=1` keeps them.
    pub rebase_merges: bool,
}

/// A commit that [`walk()`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    /// The id of the commit.
    pub id: ObjectId,
    /// Whether a commit on the side of `upstream` has the same patch, as `--cherry-mark` marks it with `PATCHSAME`.
    pub patch_same: bool,
    /// Whether the commit has the tree of its first parent, or the empty tree as a root, as
    /// `is_original_commit_empty()` tells.
    pub empty: bool,
    /// Whether the commit has more than one parent.
    pub is_merge: bool,
}

/// The commits that a rebase replays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The commits of `head` that `upstream` doesn't have, oldest first, in the order of `--topo-order` with `--reverse`.
    pub commits: Vec<Commit>,
    /// The first merge base of `upstream` and `head`, the first revision of the walk with the flag `BOTTOM`, which
    /// `--rebase-merges` labels `onto`.
    pub merge_base: Option<ObjectId>,
}

/// Walk the commits of `head` that `upstream` doesn't have and that `restrict` doesn't have either, as
/// `sequencer_make_script()` walks `<upstream>...<head> ^<restrict>` on the right side for a rebase.
///
/// The commits that the merge bases of `upstream` and `head` or `restrict` have are left out. Of the others, those of
/// `head` are returned, and those of `upstream` find the commits of `head` that have their patch, as
/// `cherry_pick_list()` compares them by [patch ID](patch_id): only commits whose [headers](patch_id::Part::Headers) have
/// equal patch IDs are compared by the patch IDs of their changes, so the blobs of the other commits aren't read. The
/// order is that of `sort_in_topological_order()` in graph order, with children before their parents and the parents of
/// a commit after it in reverse, reversed.
pub fn walk(
    commits: &mut Commits<'_>,
    upstream: ObjectId,
    head: ObjectId,
    restrict: Option<ObjectId>,
    options: Options,
) -> Result<Walk> {
    let bases = merge_bases(commits, upstream, &[head])?;
    let mut bottoms = bases.clone();
    bottoms.extend(restrict);
    let uninteresting = reachable(commits, &bottoms, &HashSet::new())?;
    let right = reachable(commits, &[head], &uninteresting)?;

    let mut upstream_patches = Patches::default();
    if !options.reapply_cherry_picks && !right.is_empty() {
        for id in reachable(commits, &[upstream], &uninteresting)? {
            upstream_patches.add(commits, id)?;
        }
    }

    let mut indegree: HashMap<ObjectId, usize> = right.iter().map(|id| (*id, 1)).collect();
    for id in &right {
        for parent in commits.parents(id)? {
            if let Some(count) = indegree.get_mut(&parent) {
                *count += 1;
            }
        }
    }
    let mut order = Vec::with_capacity(right.len());
    let mut stack: Vec<ObjectId> = right.contains(&head).then_some(head).into_iter().collect();
    while let Some(id) = stack.pop() {
        for parent in commits.parents(&id)? {
            if let Some(count) = indegree.get_mut(&parent)
                && *count != 0
            {
                *count -= 1;
                if *count == 1 {
                    stack.push(parent);
                }
            }
        }
        indegree.insert(id, 0);
        order.push(id);
    }
    order.reverse();

    let mut out = Vec::with_capacity(order.len());
    for id in order {
        let (tree, parents) = {
            let commit = commits.get(&id)?;
            (commit.tree, commit.parents.clone())
        };
        let is_merge = parents.len() > 1;
        if is_merge && !options.rebase_merges {
            continue;
        }
        let parent_tree = match parents.first() {
            Some(parent) => commits.get(parent)?.tree,
            None => commits.object_hash().empty_tree(),
        };
        let patch_same = !is_merge && upstream_patches.have(commits, &id)?;
        out.push(Commit {
            id,
            patch_same,
            empty: tree == parent_tree,
            is_merge,
        });
    }
    Ok(Walk {
        commits: out,
        merge_base: bases.first().copied(),
    })
}

/// Commits by the patch IDs of the headers of their changes, as `cherry_pick_list()` keeps the commits of one side of its
/// walk in a hash map, with the patch IDs of their changes once they were needed.
#[derive(Default)]
struct Patches {
    by_headers: HashMap<ObjectId, Vec<Patch>>,
}

/// A commit of [`Patches`].
struct Patch {
    /// The id of the commit.
    commit: ObjectId,
    /// The patch ID of its changes, once it was needed.
    id: Option<ObjectId>,
}

impl Patches {
    /// Add `commit`, unless it is a merge, which has no patch ID.
    fn add(&mut self, commits: &mut Commits<'_>, commit: ObjectId) -> Result<()> {
        if let Some(headers) = patch_id::of_commit(commits, &commit, patch_id::Part::Headers)? {
            self.by_headers
                .entry(headers)
                .or_default()
                .push(Patch { commit, id: None });
        }
        Ok(())
    }

    /// Return `true` if one of the commits has the patch of `commit`. As `cherry_pick_list()` does, only the commits whose
    /// headers have the patch ID of those of `commit` are compared with it by the patch IDs of their changes, which reads
    /// their blobs, and Git reads those of all of them.
    fn have(&mut self, commits: &mut Commits<'_>, commit: &oid) -> Result<bool> {
        if self.by_headers.is_empty() {
            return Ok(false);
        }
        let Some(headers) = patch_id::of_commit(commits, commit, patch_id::Part::Headers)? else {
            return Ok(false);
        };
        let Some(candidates) = self.by_headers.get_mut(&headers) else {
            return Ok(false);
        };
        let id = patch_id::of_commit(commits, commit, patch_id::Part::Full)?;
        let mut found = false;
        for candidate in candidates {
            if candidate.id.is_none() {
                candidate.id = patch_id::of_commit(commits, &candidate.commit, patch_id::Part::Full)?;
            }
            found |= candidate.id == id;
        }
        Ok(found)
    }
}

/// Return the commits that `tips` have, without those of `stop` and their parents.
pub(crate) fn reachable(
    commits: &mut Commits<'_>,
    tips: &[ObjectId],
    stop: &HashSet<ObjectId>,
) -> Result<HashSet<ObjectId>> {
    let mut seen = HashSet::new();
    let mut queue: Vec<ObjectId> = tips.iter().filter(|tip| !stop.contains(*tip)).copied().collect();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        for parent in commits.parents(&id)? {
            if !stop.contains(&parent) && !seen.contains(&parent) {
                queue.push(parent);
            }
        }
    }
    Ok(seen)
}
