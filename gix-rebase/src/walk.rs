//! The commits that a rebase replays, as `sequencer_make_script()` walks them with `git rev-list --right-only
//! --cherry-mark --topo-order --reverse <upstream>...<head>`.

use std::collections::{HashMap, HashSet};

use gix_error::Result;
use gix_hash::ObjectId;

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
/// `cherry_pick_list()` compares them by [patch ID](patch_id). The order is that of `sort_in_topological_order()` in
/// graph order, with children before their parents and the parents of a commit after it in reverse, reversed.
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

    let mut patch_ids = HashSet::new();
    if !options.reapply_cherry_picks && !right.is_empty() {
        for id in reachable(commits, &[upstream], &uninteresting)? {
            if let Some(patch_id) = patch_id::of_commit(commits, &id)? {
                patch_ids.insert(patch_id);
            }
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
        let patch_same = !patch_ids.is_empty()
            && !is_merge
            && patch_id::of_commit(commits, &id)?.is_some_and(|patch_id| patch_ids.contains(&patch_id));
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
