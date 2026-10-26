use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bstr::{BString, ByteSlice};
use filetime::FileTime;
use gix_error::{Result, bail};
use gix_hash::oid;
use gix_index::{State, entry::Flags};

use crate::switch::{Context, Options, Outcome, Refusals, apply, check, path, plan};

/// Switch `index` and the files of `worktree`, its root, from `old_tree` to `new_tree`, as `git read-tree -m -u` does, and
/// return what changed.
///
/// `old_tree` is the tree that `index` was made from or last committed, usually that of `HEAD`, or `None` if there is none,
/// as in a repository without commits. `objects` has both trees, the trees and blobs that the new tree adds or changes, and
/// the `.gitattributes` files of the new tree.
///
/// If the switch would lose changes or untracked files, nothing changes and [`Outcome::Refused`] lists the paths, as the
/// [module documentation](super) describes. Otherwise, `index` and the worktree are switched, and the timestamp of
/// `index` becomes the time at which this function began.
///
/// ### Errors
///
/// Nothing changes if `index` is sparse or has entries that are excluded from a sparse checkout, if the stack of `ctx`
/// doesn't provide both attributes and exclude patterns, if a tree doesn't use the hash of `index`, can't be read or has an
/// entry that can't be checked out, or if the switch is interrupted before the worktree changes. Once the worktree changes,
/// the errors of single files are reported in [`Switched`](super::Switched) instead.
pub fn switch<O>(
    index: &mut State,
    worktree: &Path,
    old_tree: Option<&oid>,
    new_tree: &oid,
    objects: O,
    ctx: Context<'_>,
    options: Options,
) -> Result<Outcome>
where
    O: gix_object::Find + gix_object::Exists + Send + Clone,
{
    if index.is_sparse()
        || index
            .entries()
            .iter()
            .any(|entry| entry.flags.contains(Flags::SKIP_WORKTREE))
    {
        bail!("Indices with entries that are excluded from a sparse checkout can't be switched".unsupported());
    }
    if !matches!(
        ctx.stack.state(),
        gix_worktree::stack::State::AttributesAndIgnoreStack { .. }
    ) {
        bail!("The stack has to provide both attributes and exclude patterns".validation());
    }
    let object_hash = index.object_hash();
    if new_tree.kind() != object_hash || old_tree.is_some_and(|old_tree| old_tree.kind() != object_hash) {
        bail!("The trees have to use the hash of the index".validation());
    }
    let _span = gix_trace::coarse!("gix_worktree_state::switch()", entries = index.entries().len());
    let start = FileTime::now();
    let Context {
        stack,
        git_dir_realpath,
        current_dir,
        should_interrupt,
    } = ctx;

    let unmerged = unmerged_paths(index);
    if !unmerged.is_empty() {
        return Ok(Outcome::Refused(Refusals {
            unmerged,
            ..Default::default()
        }));
    }
    let changes = plan::changes(old_tree, new_tree, &objects, options.checkout.validate)?;
    check_interrupt(should_interrupt)?;
    let mut refusals = Refusals::default();
    let plan = plan::Plan::new(index, changes, &mut refusals.staged_changes);
    let ignore_case = options.checkout.fs.ignore_case;
    let cwd = path::relative_current_dir(worktree, current_dir, options.checkout.fs);
    let cwd = cwd.as_ref().map(|cwd| cwd.as_bstr());
    {
        let lookup = ignore_case.then(|| index.prepare_icase_backing());
        check::Checker {
            index,
            lookup: lookup.as_ref(),
            worktree,
            objects: &objects,
            stack,
            pathspec: gix_pathspec::Search::from_specs(std::iter::empty(), None, worktree)?,
            git_dir_realpath,
            current_dir,
            cwd,
            should_interrupt,
            options: &options,
        }
        .check(&plan, &mut refusals)?;
    }
    check_interrupt(should_interrupt)?;
    if !refusals.is_empty() {
        return Ok(Outcome::Refused(refusals.finish()));
    }

    let mut result = apply::result_index(index, &plan, &objects, options.checkout.stat_options)?;
    check_interrupt(should_interrupt)?;
    let switched = apply::update_worktree(index, &mut result, &plan, worktree, objects, cwd, &options);
    result.set_timestamp(start);
    *index = result;
    Ok(Outcome::Switched(switched))
}

/// Return the paths of the conflicts in `index`.
fn unmerged_paths(index: &State) -> Vec<BString> {
    let mut paths: Vec<BString> = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .map(|entry| entry.path(index).to_owned())
        .collect();
    paths.dedup();
    paths
}

/// Fail if `should_interrupt` is set.
pub(crate) fn check_interrupt(should_interrupt: &AtomicBool) -> Result<()> {
    if should_interrupt.load(Ordering::Relaxed) {
        bail!("The switch was interrupted".cancelled());
    }
    Ok(())
}
