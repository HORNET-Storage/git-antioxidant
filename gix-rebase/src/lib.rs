//! Decide which commits a rebase replays onto what, and make the todo list that replays them, as `git rebase` does.
//!
//! [`Plan::new()`](plan::Plan::new()) takes `<upstream>`, `--onto`, `--keep-base` and `--root` as `cmd_rebase()` takes
//! them, with the [fork point](fork_point) of `--fork-point`, and tells whether the rebase may fast-forward. The
//! [walk] finds the commits that the rebase replays, leaving out those whose [patch ID](patch_id) is upstream
//! already, and [`script::make()`] makes the todo list of `gix_sequencer` from them, with the labels and merges of
//! `--rebase-merges`. The functions of [`todo`](mod@todo) add `update-ref` and `exec` commands, rearrange `fixup!`
//! commits for `--autosquash`, and find the `pick` commands that a rebase skips.
//!
//! The crate reads commits, trees and blobs through [`gix_object::Find`] and reference logs through `gix_ref`.
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use gix_error::Result;
use gix_hash::ObjectId;

mod commits;
pub use commits::{Commit, Commits, subject};

pub mod fork_point;
pub mod patch_id;
pub mod plan;
pub mod script;
pub mod todo;
pub mod walk;

/// Return the merge bases of `first` and `others`, best first, as `repo_get_merge_bases_many()` finds them, or nothing
/// if they share no history.
pub fn merge_bases(commits: &mut Commits<'_>, first: ObjectId, others: &[ObjectId]) -> Result<Vec<ObjectId>> {
    let mut graph = gix_revision::Graph::new(commits::ByRef(commits.objects()), None);
    Ok(gix_revision::merge_base(first, others, &mut graph)?
        .map(|bases| bases.into_iter().collect())
        .unwrap_or_default())
}
