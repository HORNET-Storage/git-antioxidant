//! The fork point of a branch with its upstream, as `git rebase --fork-point` finds it with `get_fork_point()`.

use gix_error::{Result, ResultExt, message};
use gix_hash::ObjectId;
use gix_ref::FullNameRef;

use crate::{commits::Commits, merge_bases};

/// Return the previous and new commit of each entry of the reference log of `name` in `store`, oldest first, as
/// `refs_for_each_reflog_ent()` visits them. Entries that can't be parsed are skipped, as Git skips them, and a missing
/// reference log has no entries.
pub fn reflog_entries(store: &gix_ref::file::Store, name: &FullNameRef) -> Result<Vec<(ObjectId, ObjectId)>> {
    let mut buf = Vec::new();
    let Some(entries) = store
        .reflog_iter(name, &mut buf)
        .or_raise(|| message!("could not read the reference log of '{}'", name.as_bstr()))?
    else {
        return Ok(Vec::new());
    };
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| (entry.previous_oid(), entry.new_oid()))
        .collect())
}

/// Return the commit at which `head` forked from the reference with the reference log `entries` that points at
/// `current`, as `get_fork_point()` finds it, or `None` if there is none.
///
/// The candidates are the previous commit of the first entry and the new commits of all entries that are commits, or
/// `current` if there are none. `head` must have exactly one merge base with them, which must be one of them.
pub fn fork_point(
    commits: &mut Commits<'_>,
    entries: &[(ObjectId, ObjectId)],
    current: ObjectId,
    head: ObjectId,
) -> Result<Option<ObjectId>> {
    let mut candidates = Vec::new();
    let first_previous = entries.first().map(|(previous, _)| *previous);
    for id in first_previous.into_iter().chain(entries.iter().map(|(_, new)| *new)) {
        add_candidate(commits, &mut candidates, id);
    }
    if candidates.is_empty() {
        add_candidate(commits, &mut candidates, current);
    }
    let bases = merge_bases(commits, head, &candidates)?;
    Ok(match bases.as_slice() {
        [base] if candidates.contains(base) => Some(*base),
        _ => None,
    })
}

/// Add `id` to `candidates` unless it is null, already there, or not a commit, as `add_one_commit()` does.
fn add_candidate(commits: &mut Commits<'_>, candidates: &mut Vec<ObjectId>, id: ObjectId) {
    if !id.is_null() && !candidates.contains(&id) && commits.get(&id).is_ok() {
        candidates.push(id);
    }
}
