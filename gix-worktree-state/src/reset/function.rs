use std::{cmp::Ordering, path::Path};

use bstr::{BStr, BString, ByteSlice};
use filetime::FileTime;
use gix_error::{Result, ResultExt, bail, message};
use gix_hash::oid;
use gix_index::{
    Entry, State,
    entry::{Flags, Stage, Stat, stat},
};
use gix_worktree::validate::path::component;

use crate::{
    reset::{Mode, Outcome},
    switch::{
        self, Changes, Context, Options, Refusals, apply, check,
        function::{check_interrupt, two_way},
        path,
        plan::{Plan, WithEmptyTree},
    },
};

/// The name of the operation in the error of an interruption.
const OPERATION: &str = "reset";

/// Reset `index` and the files of `worktree`, its root, to `tree` as `git reset` does in `mode`, and return what changed.
///
/// `objects` has `tree`, its trees, the blobs that the reset checks out and the `.gitattributes` files of `tree`, and for a
/// keep reset the tree of `HEAD` and its trees as well.
///
/// If Git refuses the reset, nothing changes and [`Outcome::Refused`] lists the paths, as the [module documentation](super)
/// describes. Otherwise, `index` holds the entries of `tree`, the worktree is changed as `mode` says, and the timestamp of
/// `index` becomes the time at which this function began. The cache tree is valid for each directory whose tree is in
/// `objects`, as Git primes it from `tree` after hard and keep resets and repairs it after merge resets, and the
/// resolve-undo information is dropped.
///
/// ### Errors
///
/// Nothing changes if `index` is sparse or has entries that are excluded from a sparse checkout, if the stack of `ctx`
/// doesn't provide both attributes and exclude patterns, if a tree doesn't use the hash of `index`, can't be read, has a
/// path that can't be checked out or more than one entry at a path, or if the reset is interrupted before the worktree
/// changes. Once the worktree changes, the errors of single files are reported in [`Changes`] instead.
pub fn reset<O>(
    index: &mut State,
    worktree: &Path,
    tree: &oid,
    mode: Mode,
    objects: O,
    ctx: Context<'_>,
    options: Options,
) -> Result<Outcome>
where
    O: gix_object::Find + gix_object::Exists + Send + Clone,
{
    let head_tree = match mode {
        Mode::Keep { head_tree } => Some(head_tree),
        Mode::Hard | Mode::Merge => None,
    };
    check_index_and_trees(index, tree, head_tree.as_deref())?;
    if !matches!(
        ctx.stack.state(),
        gix_worktree::stack::State::AttributesAndIgnoreStack { .. }
    ) {
        bail!("The stack has to provide both attributes and exclude patterns".validation());
    }
    let _span = gix_trace::coarse!("gix_worktree_state::reset()", entries = index.entries().len());
    let target = tree_index(tree, &objects, options.checkout.validate)?;
    if head_tree.is_none() {
        return one_way(index, worktree, &target, mode, objects, ctx, options);
    }

    // Git switches the index and the worktree from the tree of `HEAD` to `tree`, and then resets the index to `tree` as a
    // mixed reset does, which turns the staged changes that the switch keeps into changes of the worktree.
    let switched = two_way(
        index,
        worktree,
        head_tree.as_deref(),
        tree,
        objects.clone(),
        ctx,
        options,
        OPERATION,
    )?;
    let mut changes = match switched {
        switch::Outcome::Switched(changes) => changes,
        switch::Outcome::Refused(refusals) => return Ok(Outcome::Refused(refusals)),
    };
    let reset = reset_entries(index, &target, &mut |_| true);
    changes.added += reset.added;
    changes.modified += reset.modified;
    changes.removed += reset.removed;
    let mut cache_tree = index.remove_tree();
    for path in &reset.paths {
        apply::invalidate(cache_tree.as_mut(), path.as_bstr());
    }
    let cache_tree = index.repair_cache_tree(cache_tree.as_ref(), &objects)?;
    index.set_tree(cache_tree);
    Ok(Outcome::Reset(changes))
}

/// Reset `index` and the files of `worktree` to `target`, which holds the entries of the tree, as Git's `oneway_merge()`
/// does for a hard reset, which loses local changes and untracked files in the way, and for a merge reset, which refuses
/// them.
fn one_way<O>(
    index: &mut State,
    worktree: &Path,
    target: &State,
    mode: Mode,
    objects: O,
    ctx: Context<'_>,
    options: Options,
) -> Result<Outcome>
where
    O: gix_object::Find + gix_object::Exists + Send + Clone,
{
    let start = FileTime::now();
    let Context {
        stack,
        git_dir_realpath,
        current_dir,
        should_interrupt,
    } = ctx;

    let mut plan = Plan::one_way(index, target);
    check_interrupt(should_interrupt, OPERATION)?;
    let cwd = path::relative_current_dir(worktree, current_dir, options.checkout.fs);
    let cwd = cwd.as_ref().map(|cwd| cwd.as_bstr());
    let mut refusals = Refusals::default();
    {
        let lookup = options.checkout.fs.ignore_case.then(|| index.prepare_icase_backing());
        let mut checker = check::Checker {
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
            operation: OPERATION,
            options: &options,
        };
        if mode == Mode::Hard {
            let restored = checker.changed_files(&plan)?;
            plan.restore(index, restored);
            checker.check_current_dir(&plan, &mut refusals);
        } else {
            checker.check(&plan, &mut refusals)?;
        }
    }
    check_interrupt(should_interrupt, OPERATION)?;
    if !refusals.is_empty() {
        return Ok(Outcome::Refused(refusals.finish()));
    }

    let mut result = apply::result_index(index, &plan, &objects, options.checkout.stat_options)?;
    check_interrupt(should_interrupt, OPERATION)?;
    let changes = apply::update_worktree(index, &mut result, &plan, worktree, objects, cwd, &options);
    result.set_timestamp(start);
    *index = result;
    Ok(Outcome::Reset(changes))
}

/// Reset the entries of `index` at the paths that `is_selected` selects to those of `tree`, without changing the worktree,
/// as `git reset --mixed` does for all paths and `git reset <tree-ish> -- <pathspec>` for those of its pathspec, which
/// `read_from_tree()` in `builtin/reset.c` does both, and return what changed.
///
/// `is_selected` receives the path of each entry of `index` and `tree`. At a path that it selects, an entry that differs
/// from that of `tree` becomes it, one that `tree` doesn't have is removed, and one that only `tree` has is added, and
/// so the stages of a conflict become the entry of `tree` or are removed. Entries that change record a size of 0, so
/// that the next comparison reads their files, and the others keep their stat information. `objects` has `tree` and
/// its trees.
///
/// The cache tree is invalidated along the paths that change, and the resolve-undo information stays, as Git does it.
/// The timestamp of `index` becomes the time at which this function began, after the entries whose files may have
/// changed since `index` was written, as `stat_options` decide, record a size of 0.
///
/// ### Errors
///
/// Nothing changes if `index` is sparse or has entries that are excluded from a sparse checkout, if `tree` doesn't use the
/// hash of `index`, can't be read, has a path that `validate` refuses or more than one entry at a path.
pub fn mixed<O>(
    index: &mut State,
    tree: &oid,
    objects: &O,
    mut is_selected: impl FnMut(&BStr) -> bool,
    validate: component::Options,
    stat_options: stat::Options,
) -> Result<Changes>
where
    O: gix_object::Find,
{
    check_index_and_trees(index, tree, None)?;
    let _span = gix_trace::coarse!("gix_worktree_state::reset::mixed()", entries = index.entries().len());
    let start = FileTime::now();
    let target = tree_index(tree, objects, validate)?;
    let reset = reset_entries(index, &target, &mut is_selected);
    if let Some(mut cache_tree) = index.remove_tree() {
        for path in &reset.paths {
            cache_tree.invalidate_path(path.as_bstr());
        }
        index.set_tree(cache_tree);
    }
    apply::smudge_racy_entries(index, stat_options);
    index.set_timestamp(start);
    Ok(Changes {
        added: reset.added,
        modified: reset.modified,
        removed: reset.removed,
        ..Default::default()
    })
}

/// Fail if `index` is sparse or has entries that are excluded from a sparse checkout, or if `tree` or `head_tree` don't
/// use the hash of `index`.
fn check_index_and_trees(index: &State, tree: &oid, head_tree: Option<&oid>) -> Result<()> {
    if index.is_sparse()
        || index
            .entries()
            .iter()
            .any(|entry| entry.flags.contains(Flags::SKIP_WORKTREE))
    {
        bail!("Indices with entries that are excluded from a sparse checkout can't be reset".unsupported());
    }
    let object_hash = index.object_hash();
    if tree.kind() != object_hash || head_tree.is_some_and(|head_tree| head_tree.kind() != object_hash) {
        bail!("The trees have to use the hash of the index".validation());
    }
    Ok(())
}

/// Return an index with the entries of `tree`, sorted by their paths. Fails if `tree` can't be read, if `validate` refuses
/// one of its paths, or if a path has more than one entry, as a file and a directory for example.
fn tree_index<O>(tree: &oid, objects: &O, validate: component::Options) -> Result<State>
where
    O: gix_object::Find,
{
    let mut index = State::from_tree(tree, WithEmptyTree(objects), validate)
        .or_raise(|| message!("Could not read the tree {tree}"))?;
    index.sort_entries();
    let entries = index.entries();
    for (position, entry) in entries.iter().enumerate() {
        let path = entry.path(&index);
        let mut directory = path.to_owned();
        directory.push(b'/');
        if entries.get(position + 1).is_some_and(|next| next.path(&index) == path)
            || index.prefixed_entries_range(directory.as_bstr()).is_some()
        {
            bail!("The tree has more than one entry at '{}'".corrupted(), path);
        }
    }
    Ok(index)
}

/// What [`reset_entries()`] changed.
#[derive(Default)]
struct ResetEntries {
    added: usize,
    modified: usize,
    removed: usize,
    /// The paths whose entries changed.
    paths: Vec<BString>,
}

/// Make the entries of `index` at the paths that `is_selected` selects those of `target`, as Git's
/// `update_index_from_diff()` does: an entry that differs from that of `target` becomes it, one that `target` doesn't
/// have is removed, one that only `target` has is added, and the stages of a conflict become the entry of `target` or are
/// removed. An added entry replaces the entries in its way that `is_selected` doesn't select, as `add_index_entry()`
/// replaces them. Changed and added entries have no stat information. Both indices are sorted.
fn reset_entries(index: &mut State, target: &State, is_selected: &mut dyn FnMut(&BStr) -> bool) -> ResetEntries {
    let mut reset = ResetEntries::default();
    let mut removed = vec![false; index.entries().len()];
    let (mut updated, mut added) = (Vec::new(), Vec::new());
    {
        let (entries, targets) = (index.entries(), target.entries());
        let (mut position, mut target_position) = (0, 0);
        loop {
            let ordering = match (entries.get(position), targets.get(target_position)) {
                (Some(entry), Some(new)) => Entry::cmp_filepaths(entry.path(index), new.path(target)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => break,
            };
            if ordering == Ordering::Greater {
                let path = targets[target_position].path(target);
                if is_selected(path) {
                    added.push(target_position);
                    reset.added += 1;
                    reset.paths.push(path.to_owned());
                }
                target_position += 1;
                continue;
            }
            let entry = &entries[position];
            let path = entry.path(index);
            let end = position
                + entries[position..]
                    .iter()
                    .take_while(|other| other.path(index) == path)
                    .count();
            let new = (ordering == Ordering::Equal).then_some(target_position);
            if new.is_some() {
                target_position += 1;
            }
            if is_selected(path) {
                let is_conflict = end - position > 1 || entry.stage_raw() != 0;
                match new {
                    Some(new) if !is_conflict => {
                        let new_entry = &targets[new];
                        if (new_entry.mode, new_entry.id) != (entry.mode, entry.id) {
                            updated.push((position, new));
                            reset.modified += 1;
                            reset.paths.push(path.to_owned());
                        }
                    }
                    Some(new) => {
                        removed[position..end].fill(true);
                        added.push(new);
                        reset.modified += 1;
                        reset.paths.push(path.to_owned());
                    }
                    None => {
                        removed[position..end].fill(true);
                        reset.removed += 1;
                        reset.paths.push(path.to_owned());
                    }
                }
            }
            position = end;
        }

        // The entries in the way of an added entry are those of the files at its leading directories and those in the
        // directory at its path. The stages of conflicts stay, as Git only replaces entries of the same stage.
        for &new in &added {
            let path = targets[new].path(target);
            let files = path
                .find_iter("/")
                .filter_map(|slash| index.entry_index_by_path_and_stage(path[..slash].as_bstr(), Stage::Unconflicted));
            let mut directory = path.to_owned();
            directory.push(b'/');
            let contained = index.prefixed_entries_range(directory.as_bstr()).unwrap_or_default();
            for position in files.chain(contained) {
                let entry = &entries[position];
                if !removed[position] && entry.stage_raw() == 0 {
                    removed[position] = true;
                    reset.removed += 1;
                    reset.paths.push(entry.path(index).to_owned());
                }
            }
        }
    }

    let entries = index.entries_mut();
    for (position, new) in updated {
        let new = &target.entries()[new];
        let entry = &mut entries[position];
        entry.id = new.id;
        entry.mode = new.mode;
        entry.stat = Stat::default();
        entry.flags = Flags::empty();
    }
    index.remove_entries(|position, _, _| removed[position]);
    if !added.is_empty() {
        for new in added {
            let new = &target.entries()[new];
            index.dangerously_push_entry(Stat::default(), new.id, Flags::empty(), new.mode, new.path(target));
        }
        index.sort_entries();
    }
    reset
}
