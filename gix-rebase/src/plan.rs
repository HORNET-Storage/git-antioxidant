//! What a rebase replays onto what, as `cmd_rebase()` decides it from `<upstream>`, `--onto`, `--keep-base`,
//! `--fork-point` and `--root`.

use gix_error::{Result, message};
use gix_hash::ObjectId;

use crate::{
    commits::Commits,
    merge_bases,
    walk::{self, Walk},
};

/// The commits that a rebase starts from, as `cmd_rebase()` takes them from its arguments.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The commit of `<upstream>`, whose history the rebase doesn't replay, or `None` for `--root`, which replays all
    /// commits of `head`.
    pub upstream: Option<ObjectId>,
    /// The commit of `--onto`. Without it, the rebase replays onto `upstream`, or onto `squash_onto` for `--root`.
    pub onto: Option<ObjectId>,
    /// `--keep-base`: replay onto the merge base of `upstream` and `head`, which must be the only one.
    pub keep_base: bool,
    /// The fork point of `head` with `upstream` for `--fork-point`, as [`fork_point()`](crate::fork_point::fork_point())
    /// finds it, which the rebase doesn't replay the history of either.
    pub restrict: Option<ObjectId>,
    /// `--reapply-cherry-picks`, which `--keep-base` implies unless it is negated.
    pub reapply_cherry_picks: bool,
    /// The empty commit that `--root` without `--onto` replays onto, which `cmd_rebase()` makes for it.
    pub squash_onto: Option<ObjectId>,
}

/// What a rebase replays onto what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The commit onto which the commits are replayed.
    pub onto: ObjectId,
    /// The commit whose history isn't replayed, or `None` for `--root`.
    pub upstream: Option<ObjectId>,
    /// The fork point whose history isn't replayed either.
    pub restrict: Option<ObjectId>,
    /// The merge base of `onto` and `head` if it is the only one, as `fill_branch_base()` finds it.
    pub branch_base: Option<ObjectId>,
    /// The commit that is rebased.
    pub head: ObjectId,
}

impl Plan {
    /// Decide what the rebase of `head` replays onto what with `options`, as `cmd_rebase()` does.
    pub fn new(commits: &mut Commits<'_>, head: ObjectId, options: &Options) -> Result<Self> {
        let (onto, branch_base) = if options.keep_base {
            let upstream = options
                .upstream
                .ok_or_else(|| message!("--keep-base needs an upstream").validation())?;
            match merge_bases(commits, upstream, &[head])?.as_slice() {
                [base] => (*base, Some(*base)),
                _ => {
                    return Err(message!("'{upstream}': need exactly one merge base with branch")
                        .validation()
                        .into());
                }
            }
        } else {
            let onto = options
                .onto
                .or(options.upstream)
                .or(options.squash_onto)
                .ok_or_else(|| message!("a base commit must be provided with --upstream or --onto").validation())?;
            let branch_base = match merge_bases(commits, onto, &[head])?.as_slice() {
                [base] => Some(*base),
                _ => None,
            };
            (onto, branch_base)
        };
        let upstream = if options.keep_base && options.reapply_cherry_picks {
            Some(onto)
        } else {
            options.upstream
        };
        Ok(Plan {
            onto,
            upstream,
            restrict: options.restrict,
            branch_base,
            head,
        })
    }

    /// Return `true` if `head` is based on `onto` with a linear history already, so that the rebase may leave it as it
    /// is without `--force-rebase`, as `can_fast_forward()` tells.
    pub fn can_fast_forward(&self, commits: &mut Commits<'_>) -> Result<bool> {
        let Some(branch_base) = self.branch_base else {
            return Ok(false);
        };
        if branch_base != self.onto || self.restrict.is_some_and(|restrict| restrict != branch_base) {
            return Ok(false);
        }
        let Some(upstream) = self.upstream else {
            return Ok(false);
        };
        match merge_bases(commits, upstream, &[self.head])?.as_slice() {
            [base] if *base == self.onto => is_linear_history(commits, self.onto, self.head),
            _ => Ok(false),
        }
    }

    /// Return the commits that the rebase replays, as `do_interactive_rebase()` walks `<upstream or onto>...<head>`
    /// without the history of `restrict`.
    pub fn walk(&self, commits: &mut Commits<'_>, options: walk::Options) -> Result<Walk> {
        walk::walk(
            commits,
            self.upstream.unwrap_or(self.onto),
            self.head,
            self.restrict,
            options,
        )
    }
}

/// Return `true` if the commits from `to` back to `from` have one parent each, as `is_linear_history()` tells, which
/// expects `from` to be an ancestor of `to`.
pub fn is_linear_history(commits: &mut Commits<'_>, from: ObjectId, to: ObjectId) -> Result<bool> {
    let mut current = to;
    while current != from {
        match commits.get(&current)?.parents.as_slice() {
            [] => return Ok(true),
            [parent] => current = *parent,
            _ => return Ok(false),
        }
    }
    Ok(true)
}
