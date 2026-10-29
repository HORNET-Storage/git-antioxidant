//! `gix rebase`, which rebases a branch as `git rebase` does with its merge backend, and continues, skips, aborts or
//! forgets the rebase in progress with `--continue`, `--skip`, `--abort` and `--quit`.

use gix::bstr::BString;

/// What `gix rebase` does with the rebase in progress instead of starting one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Commit the staged changes and perform the rest of the todo list, as `--continue` does.
    Continue,
    /// Leave out the commit at which the rebase stopped and perform the rest of the todo list, as `--skip` does.
    Skip,
    /// Go back to the branch as it was before the rebase, as `--abort` does.
    Abort,
    /// Forget the rebase in progress and keep `HEAD`, the index and the worktree as they are, as `--quit` does.
    Quit,
}

/// The options of `gix rebase`, named after those of `git rebase`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `<upstream>`, the revision whose history isn't replayed, or `None` for the upstream of the current branch. With
    /// `--root`, the branch to rebase, as `git rebase --root [<branch>]` takes it.
    pub upstream: Option<BString>,
    /// `<branch>`, the branch or commit to check out and rebase, or `None` for `HEAD`.
    pub branch: Option<BString>,
    /// The revision to replay the commits onto, as `--onto` names it, or `None` for the upstream.
    pub onto: Option<BString>,
    /// If `true`, replay onto the merge base of the upstream and the branch, as `--keep-base` does.
    pub keep_base: bool,
    /// If `true`, replay all commits of the branch, as `--root` does.
    pub root: bool,
    /// If `true`, let the sequence editor edit the todo list, as `--interactive` does.
    pub interactive: bool,
    /// The commands to run after each commit, as `--exec` gives them.
    pub exec: Vec<BString>,
    /// `--autosquash` or `--no-autosquash`, or `None` for `rebase.autoSquash`.
    pub autosquash: Option<bool>,
    /// `--update-refs` or `--no-update-refs`, or `None` for `rebase.updateRefs`.
    pub update_refs: Option<bool>,
    /// Whether merges are rebased, as `--rebase-merges` and `--no-rebase-merges` say.
    pub rebase_merges: gix::rebase::Merges,
    /// `--fork-point` or `--no-fork-point`, or `None` to use the fork point only without upstream.
    pub fork_point: Option<bool>,
    /// If `true`, replay all commits even if their parents wouldn't change, as `--force-rebase` does.
    pub force_rebase: bool,
    /// What to do with commits that become empty, as `--empty` says.
    pub empty: Option<gix::rebase::Empty>,
    /// If `true`, don't run the `pre-rebase` hook, as `--no-verify` does.
    pub no_verify: bool,
    /// If `true`, put a failed `exec` command back into the todo list, as `--reschedule-failed-exec` does, and otherwise
    /// do as `rebase.rescheduleFailedExec` says.
    pub reschedule_failed_exec: bool,
    /// What to do with the rebase in progress instead of starting one, or `None` to start one.
    pub action: Option<Action>,
}

pub(crate) mod function {
    use std::io::Write;

    use gix::{
        Result,
        bstr::ByteSlice,
        error::{ResultExt, bail},
        rebase::{Error, Outcome, Programs, Stop, StopReason},
    };

    use super::{Action, Options};
    use crate::repository::{advise_if_enabled, reflog_action};

    /// The advice `mergeConflict` that `print_advice()` in Git's `sequencer.c` gives when a commit of a rebase didn't
    /// apply without conflicts, `rebase_resolvemsg`.
    const RESOLVE_MESSAGE: &str = "Resolve all conflicts manually, mark them as resolved with\n\"git add/rm <conflicted_files>\", then run \"git rebase --continue\".\nYou can instead skip this commit: run \"git rebase --skip\".\nTo abort and get back to the state before \"git rebase\", run \"git rebase --abort\".";

    /// Rebase as `git rebase` does with its merge backend and the arguments of `options`, as `cmd_rebase()` in Git's
    /// `builtin/rebase.c` does, or take the step of the rebase in progress that the action of `options` names, and
    /// print what Git prints at the end of the rebase or where it stops.
    ///
    /// * `Current branch <name> is up to date.`, or `HEAD is up to date.` for a detached `HEAD`, if there is nothing
    ///   to rebase.
    /// * `Successfully rebased and updated <reference>.` on `err` when the rebase ends, as `pick_commits()` in Git's
    ///   `sequencer.c` prints it.
    /// * Where the rebase stops, Git's message: on `err` after `edit` and `break`, which end successfully as in Git, and
    ///   as the failure otherwise, after the advice about conflicts and the reason why a commit wasn't made.
    ///
    /// The sequence editor, the editor and the commands of `--exec` and `exec` start as Git starts them, and
    /// `GIT_REFLOG_ACTION` replaces the action of the reflog messages, as in Git.
    pub fn rebase(
        repo: gix::Repository,
        mut out: impl std::io::Write,
        mut err: impl std::io::Write,
        options: Options,
    ) -> Result<()> {
        let step = gix::rebase::StepOptions {
            reflog_action: reflog_action(),
            programs: Programs::Allow,
        };
        let should_interrupt = &gix::interrupt::IS_INTERRUPTED;
        let outcome = match options.action {
            Some(Action::Abort) => return repo.rebase_abort(step, should_interrupt),
            Some(Action::Quit) => return repo.rebase_quit(),
            Some(Action::Continue) => repo.rebase_continue(step, should_interrupt),
            Some(Action::Skip) => repo.rebase_skip(step, should_interrupt),
            None => start(&repo, options),
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return Err(refused(&repo, error)?),
        };
        match outcome {
            Outcome::UpToDate { name } => {
                if name == "HEAD" {
                    writeln!(out, "HEAD is up to date.").or_error()
                } else {
                    writeln!(out, "Current branch {name} is up to date.").or_error()
                }
            }
            Outcome::Finished { branch, .. } => {
                let updated = branch
                    .as_ref()
                    .map_or("detached HEAD".into(), gix::refs::FullName::as_bstr);
                writeln!(err, "Successfully rebased and updated {updated}.").or_error()
            }
            Outcome::Stopped(stop) => stopped(&repo, stop, &mut err),
        }
    }

    /// Start the rebase that `options` describe, as `cmd_rebase()` does after it checked its arguments.
    fn start(
        repo: &gix::Repository,
        Options {
            upstream,
            branch,
            onto,
            keep_base,
            root,
            interactive,
            exec,
            autosquash,
            update_refs,
            rebase_merges,
            fork_point,
            force_rebase,
            empty,
            no_verify,
            reschedule_failed_exec,
            action: _,
        }: Options,
    ) -> Result<Outcome> {
        let (upstream, branch) = if root {
            if branch.is_some() {
                bail!(gix::error::validation(
                    "--root takes no upstream, only the branch to rebase"
                ));
            }
            (None, upstream)
        } else {
            (upstream, branch)
        };
        repo.rebase(
            gix::rebase::Options {
                upstream,
                onto,
                keep_base,
                root,
                branch,
                fork_point,
                interactive,
                exec,
                autosquash,
                update_refs,
                rebase_merges,
                force_rebase,
                no_verify,
                reapply_cherry_picks: None,
                empty,
                reschedule_failed_exec: reschedule_failed_exec.then_some(true),
                reflog_action: reflog_action(),
                programs: Programs::Allow,
            },
            &gix::interrupt::IS_INTERRUPTED,
        )
    }

    /// Return the error to fail with for `error`, which is `error` unless Git says more: without an upstream to rebase
    /// onto, all that `error_on_missing_default_upstream()` in Git's `builtin/rebase.c` prints.
    fn refused(repo: &gix::Repository, error: gix::Error) -> Result<gix::Error> {
        if !matches!(error.downcast_any_ref::<Error>(), Some(Error::NoUpstream)) {
            return Ok(error);
        }
        let text = match repo.head_name()? {
            Some(branch) => {
                let name = branch.shorten();
                let remote = repo
                    .branch_remote_name(name, gix::remote::Direction::Fetch)
                    .map_or_else(|| "<remote>".into(), |remote| remote.as_bstr().to_string());
                format!(
                    "There is no tracking information for the current branch.\n{MISSING_UPSTREAM}\n\nIf you wish to set tracking information for this branch you can do so with:\n\n    git branch --set-upstream-to={remote}/<branch> {name}"
                )
            }
            None => format!("You are not currently on a branch.\n{MISSING_UPSTREAM}"),
        };
        Ok(gix::error::validation(text).validation_error())
    }

    /// What `error_on_missing_default_upstream()` in Git's `builtin/rebase.c` prints after its first line.
    const MISSING_UPSTREAM: &str = "Please specify which branch you want to rebase against.\nSee git-rebase(1) for details.\n\n    git rebase '<branch>'";

    /// Print what Git prints where the rebase stopped as `stop` says, and fail as Git does unless the rebase stopped for
    /// `edit` or `break`: the advice about conflicts and the reason why a commit wasn't made on `err`, followed by the
    /// message of the stop as the failure.
    fn stopped(repo: &gix::Repository, stop: Stop, err: &mut dyn Write) -> Result<()> {
        let message = stop.message.trim_end_with(|character| character == '\n');
        match &stop.reason {
            StopReason::Edit { .. } | StopReason::Break => {
                err.write_all(message).or_error()?;
                return writeln!(err).or_error();
            }
            StopReason::Conflicts { commit: Some(_) } => {
                advise_if_enabled(repo, "mergeConflict", RESOLVE_MESSAGE, err)?;
            }
            StopReason::Uncommitted { reason, .. } => writeln!(err, "{reason}").or_error()?,
            _ => {}
        }
        bail!(gix::error::conflict(message.to_str_lossy().into_owned()))
    }
}
