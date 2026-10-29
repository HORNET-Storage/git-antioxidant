//! Rebasing branches as `git rebase` does with its merge backend, with the todo lists and state of
//! [`gix_sequencer`](crate::sequencer) and the plans of [`plumbing`].
//!
//! [`Repository::rebase()`](crate::Repository::rebase()) starts a rebase as `cmd_rebase()` and `run_sequencer_rebase()`
//! do: it decides with [`plumbing::plan::Plan`] which commits it replays onto what, makes the todo list with
//! [`plumbing::script::make()`], lets the sequence editor edit it in an interactive rebase, checks out the commit to replay
//! onto with a detached `HEAD` and performs the commands of the list, as `pick_commits()` does, until the rebase ends or
//! stops. [`Repository::rebase_continue()`](crate::Repository::rebase_continue()),
//! [`rebase_skip()`](crate::Repository::rebase_skip()), [`rebase_abort()`](crate::Repository::rebase_abort()) and
//! [`rebase_quit()`](crate::Repository::rebase_quit()) take the steps of `git rebase --continue`, `--skip`, `--abort`
//! and `--quit`.
//!
//! The state of a rebase in progress is the one Git keeps in `.git/rebase-merge`, so that Git continues the rebases of
//! `gix` and `gix` those of Git.
//!
//! ### Programs
//!
//! The hooks of a rebase run as Git runs them through [`gix_hook`], which needs the `hooks` feature of `gix`: without it,
//! no hook runs. The other programs of a rebase, the sequence editor that edits the todo list, the editor that edits the
//! messages of `reword` and `squash` and the commands of `exec`, only start with [`Programs::Allow`], through
//! `gix_command` as [`Repository::editor_command()`](crate::Repository::editor_command()) starts the editor. With
//! [`Programs::Deny`], the todo list is used as made and messages as prepared, as with the editor `:`.
//!
//! ### Deviation
//!
//! * Notes aren't copied to the commits that a rebase rewrites, as `notes.rewrite.rebase` would copy them.
//! * `patch` in the state directory isn't written when a rebase stops, as `gix` doesn't write patches in Git's format.
//! * The messages of Git about merges with conflicts, like `CONFLICT (content): Merge conflict in <path>`, aren't made.
//! * The editor of a commit message doesn't see the comments about the state of the repository that `git commit` adds,
//!   so that `COMMIT_EDITMSG` lacks them unless the editor replaces the message.
//! * The deviations of [`gix_worktree_state::switch()`] and [`gix_worktree_state::reset()`] apply to the checkouts and
//!   resets of a rebase, among them that the entries of the index aren't refreshed.
//! * `merge` commands that merge several labels, as octopus merges do, or that need a merge strategy other than `ort`,
//!   and `revert` commands aren't performed: they are rescheduled, and the rebase stops.
//! * The placeholders of `rebase.instructionFormat` are `%H`, `%h`, `%T`, `%t`, `%P`, `%p`, `%an`, `%ae`, `%s`, `%n` and
//!   `%%`. Others are kept as they are.
//! * `rebase.maxLabelLength` isn't read: the labels that `--rebase-merges` makes from messages have the length that Git
//!   allows by default.

use gix_hash::ObjectId;

use crate::bstr::BString;

pub use gix_rebase as plumbing;

/// Options for [`Repository::rebase()`](crate::Repository::rebase()), which follow those of `git rebase`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `<upstream>`, the revision whose history isn't replayed, which `-` names as `@{-1}`. Without it and without
    /// [`root`](Self::root), the configured upstream of the current branch is used, and [`fork_point`](Self::fork_point)
    /// defaults to `rebase.forkPoint`.
    pub upstream: Option<BString>,
    /// `--onto <newbase>`, the revision onto which the commits are replayed instead of the upstream. `<a>...<b>` names
    /// the merge base of `<a>` and `<b>`, which must be the only one, and either side defaults to `HEAD`.
    pub onto: Option<BString>,
    /// `--keep-base`: replay onto the merge base of the upstream and the branch, which must be the only one.
    pub keep_base: bool,
    /// `--root`: replay all commits of the branch, onto [`onto`](Self::onto) or onto a new root commit without files.
    pub root: bool,
    /// `<branch>`, the branch or commit to rebase, which is checked out first, or `None` for `HEAD`.
    pub branch: Option<BString>,
    /// `--fork-point` or `--no-fork-point`, which leaves out the commits that the reference log of the upstream had, or
    /// `None` to do so without an [`upstream`](Self::upstream) if `rebase.forkPoint` isn't `false`.
    pub fork_point: Option<bool>,
    /// `--interactive`: let the sequence editor edit the todo list, and stop at commits that become empty.
    pub interactive: bool,
    /// `--exec <cmd>`: the commands to run with the shell after each commit, which need [`Programs::Allow`].
    pub exec: Vec<BString>,
    /// `--autosquash` or `--no-autosquash`, or `None` for `rebase.autoSquash` in an [`interactive`](Self::interactive)
    /// rebase.
    pub autosquash: Option<bool>,
    /// `--update-refs` or `--no-update-refs`, or `None` for `rebase.updateRefs`.
    pub update_refs: Option<bool>,
    /// `--rebase-merges` or `--no-rebase-merges`, or neither.
    pub rebase_merges: Merges,
    /// `--force-rebase`: replay all commits, even those whose parent wouldn't change.
    pub force_rebase: bool,
    /// `--no-verify`: don't run the `pre-rebase` hook.
    pub no_verify: bool,
    /// `--reapply-cherry-picks` or `--no-reapply-cherry-picks`, or `None` to reapply them only with
    /// [`keep_base`](Self::keep_base).
    pub reapply_cherry_picks: Option<bool>,
    /// `--empty`, what to do with commits that become empty, or `None` to [stop](Empty::Stop) in an
    /// [`interactive`](Self::interactive) rebase, [keep](Empty::Keep) them with [`exec`](Self::exec) and
    /// [drop](Empty::Drop) them otherwise.
    pub empty: Option<Empty>,
    /// `--reschedule-failed-exec` or `--no-reschedule-failed-exec`, or `None` for `rebase.rescheduleFailedExec`.
    pub reschedule_failed_exec: Option<bool>,
    /// The action of the reflog messages, as `GIT_REFLOG_ACTION` sets it, or `None` for `rebase`.
    pub reflog_action: Option<BString>,
    /// Whether the sequence editor, the editor and the commands of `exec` may start.
    pub programs: Programs,
}

/// Options for the steps of a rebase in progress: [`Repository::rebase_continue()`](crate::Repository::rebase_continue()),
/// [`rebase_skip()`](crate::Repository::rebase_skip()) and [`rebase_abort()`](crate::Repository::rebase_abort()).
#[derive(Debug, Clone, Default)]
pub struct StepOptions {
    /// The action of the reflog messages, as `GIT_REFLOG_ACTION` sets it, or `None` for `rebase`.
    pub reflog_action: Option<BString>,
    /// Whether the editor and the commands of `exec` may start.
    pub programs: Programs,
}

/// Whether a rebase starts the programs that `git rebase` starts besides hooks: the sequence editor, the editor and the
/// commands of `exec`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Programs {
    /// Start none of them: the todo list is used as made and messages as prepared, as with the editor `:`, and the
    /// rebase stops with an error before an `exec` command, which stays in the todo list.
    #[default]
    Deny,
    /// Start them as Git does.
    Allow,
}

/// Whether a rebase keeps merges, as `--rebase-merges` and `--no-rebase-merges` say.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Merges {
    /// Keep merges as `rebase.rebaseMerges` says, and drop them if it isn't set.
    #[default]
    AsConfigured,
    /// `--no-rebase-merges`: replay the commits of merged branches as if they were one line of history.
    Drop,
    /// `--rebase-merges[=<mode>]`: keep the merges, with the commits that don't descend from the upstream rebased or
    /// not as the mode says. `--rebase-merges` without mode is
    /// [`NoRebaseCousins`](plumbing::script::RebaseMerges::NoRebaseCousins).
    Rebase(plumbing::script::RebaseMerges),
}

/// What a rebase does with a commit that becomes empty when it is replayed, as `--empty` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Empty {
    /// `--empty=drop`: leave it out.
    Drop,
    /// `--empty=keep`: make an empty commit of it.
    Keep,
    /// `--empty=stop`: stop for the user to decide.
    Stop,
}

/// What a step of a rebase did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// There was nothing to rebase, as the branch has a linear history on top of the commit to replay onto already,
    /// and the rebase wasn't forced. Git says `Current branch <name> is up to date.`, or `HEAD is up to date.` for a
    /// detached `HEAD`. The branch to rebase was checked out if it was given.
    UpToDate {
        /// The name of the branch that Git names, without `refs/heads/`, or `HEAD`.
        name: BString,
    },
    /// The rebase ended: the branch points to the commits that it made, `HEAD` points to the branch again, and the state
    /// of the rebase is gone.
    Finished {
        /// The commit to which `HEAD` points.
        head: ObjectId,
        /// The branch that was rebased, or `None` if `HEAD` was detached.
        branch: Option<gix_ref::FullName>,
    },
    /// The rebase stopped, and waits for [`Repository::rebase_continue()`](crate::Repository::rebase_continue()),
    /// [`rebase_skip()`](crate::Repository::rebase_skip()), [`rebase_abort()`](crate::Repository::rebase_abort()) or
    /// [`rebase_quit()`](crate::Repository::rebase_quit()).
    Stopped(Stop),
}

/// Why and where a rebase stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    /// Why the rebase stopped.
    pub reason: StopReason,
    /// What Git prints when it stops there, without its hints, a line for each message.
    pub message: BString,
}

/// Why a rebase stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// A command couldn't apply the changes of its commit without conflicts, which the index and the worktree hold. The
    /// commit is `None` for a `merge` without the commit of an original merge.
    Conflicts {
        /// The commit whose changes didn't apply.
        commit: Option<ObjectId>,
    },
    /// A command applied the changes of its commit, which are staged, but the commit of them wasn't made: it became empty
    /// and the rebase stops for such commits, or the commit was refused, as by a hook or for an empty message.
    Uncommitted {
        /// The commit whose changes were applied.
        commit: ObjectId,
        /// Why the commit wasn't made.
        reason: String,
    },
    /// `edit` made its commit, for it to be amended.
    Edit {
        /// The commit that `edit` named.
        commit: ObjectId,
    },
    /// `break` stopped the rebase.
    Break,
    /// The command of `exec` failed, or succeeded but left changes in the index or the worktree.
    Exec {
        /// The command.
        command: BString,
        /// Whether the command succeeded.
        success: bool,
        /// Whether the index or the worktree have changes.
        dirty: bool,
        /// Whether the `exec` command was put back at the start of the todo list, to run again.
        rescheduled: bool,
    },
    /// A command failed in a way that lets it run again, and was put back at the start of the todo list.
    Rescheduled {
        /// The line of the command in the todo list.
        line: BString,
        /// Why the command failed.
        reason: String,
    },
    /// The todo list that the sequence editor left has lines that can't be parsed, or drops commits without `drop` where
    /// `rebase.missingCommitsCheck` is `error`.
    InvalidTodoList,
}

/// The reasons for which `git rebase` refuses to act or can't go on, each with Git's message.
///
/// Nothing changed then unless the variant says otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The repository has no worktree.
    BareRepository,
    /// A rebase is in progress already, whose state is in this directory.
    InProgress {
        /// The state directory of the rebase in progress.
        state_dir: std::path::PathBuf,
    },
    /// No rebase is in progress.
    NotInProgress,
    /// Two options were given that can't be used together.
    IncompatibleOptions {
        /// The first option.
        first: &'static str,
        /// The second option.
        second: &'static str,
    },
    /// No upstream was given, and the current branch has none configured.
    NoUpstream,
    /// The upstream doesn't name a commit.
    InvalidUpstream {
        /// The upstream as it was given.
        name: BString,
    },
    /// The branch to rebase names neither a branch nor a commit.
    InvalidBranch {
        /// The branch as it was given.
        name: BString,
    },
    /// The commit to replay onto doesn't exist.
    InvalidOnto {
        /// The revision of the commit as it was given.
        name: BString,
    },
    /// The commits whose merge base the rebase needs don't have exactly one.
    MergeBase {
        /// The revision whose merge bases were looked for, as Git names it.
        name: BString,
        /// Whether the merge base with the branch was looked for, as `--keep-base` does.
        with_branch: bool,
    },
    /// `--fork-point` needs a reference for the upstream.
    NoSuchRef {
        /// The upstream as it was given.
        name: BString,
    },
    /// Another worktree uses the branch to rebase: it has the branch checked out, or rebases or bisects it with a
    /// detached `HEAD`.
    UsedByWorktree {
        /// The short name of the branch.
        name: BString,
        /// The directory of the worktree that uses the branch.
        path: std::path::PathBuf,
    },
    /// The worktree has changes that aren't staged, or the index has changes that aren't committed.
    DirtyWorktree {
        /// Whether the worktree has changes that aren't staged.
        unstaged: bool,
        /// Whether the index has changes that aren't committed.
        uncommitted: bool,
    },
    /// The `pre-rebase` hook refused the rebase.
    PreRebaseHookRefused,
    /// `--continue` found changes in the worktree that aren't staged, which are conflicts that weren't resolved.
    UnresolvedConflicts,
    /// The todo list has no command to perform, and the state of the rebase was removed.
    NothingToDo,
    /// A program would have to start that [`Programs::Deny`] doesn't allow: the command of an `exec` or of `--exec`.
    ProgramsDenied,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BareRepository => f.write_str("this operation must be run in a work tree"),
            Error::InProgress { state_dir } => {
                let name = state_dir.file_name().map_or(state_dir.as_path(), std::path::Path::new);
                write!(
                    f,
                    "It seems that there is already a {name} directory, and\nI wonder if you are in the middle of another \
                     rebase.  If that is the\ncase, please try\n\tgit rebase (--continue | --abort | --skip)\nIf that is not the \
                     case, please\n\trm -fr \"{path}\"\nand run me again.  I am stopping in case you still have something\n\
                     valuable there.",
                    name = name.display(),
                    path = state_dir.display()
                )
            }
            Error::NotInProgress => f.write_str("no rebase in progress"),
            Error::IncompatibleOptions { first, second } => {
                write!(f, "options '{first}' and '{second}' cannot be used together")
            }
            Error::NoUpstream => f.write_str("There is no tracking information for the current branch."),
            Error::InvalidUpstream { name } => write!(f, "invalid upstream '{name}'"),
            Error::InvalidBranch { name } => write!(f, "no such branch/commit '{name}'"),
            Error::InvalidOnto { name } => write!(f, "Does not point to a valid commit '{name}'"),
            Error::MergeBase { name, with_branch } => {
                let with = if *with_branch { " with branch" } else { "" };
                write!(f, "'{name}': need exactly one merge base{with}")
            }
            Error::NoSuchRef { name } => write!(f, "No such ref: '{name}'"),
            Error::UsedByWorktree { name, path } => {
                let path = path.display();
                write!(f, "'{name}' is already used by worktree at '{path}'")
            }
            Error::DirtyWorktree { unstaged, uncommitted } => {
                if *unstaged {
                    f.write_str("cannot rebase: You have unstaged changes.\n")?;
                }
                if *uncommitted {
                    let also = if *unstaged {
                        "additionally, your"
                    } else {
                        "cannot rebase: Your"
                    };
                    writeln!(f, "{also} index contains uncommitted changes.")?;
                }
                f.write_str("Please commit or stash them.")
            }
            Error::PreRebaseHookRefused => f.write_str("The pre-rebase hook refused to rebase."),
            Error::UnresolvedConflicts => {
                f.write_str("You must edit all merge conflicts and then\nmark them as resolved using git add")
            }
            Error::NothingToDo => f.write_str("nothing to do"),
            Error::ProgramsDenied => {
                f.write_str("the commands of `exec` can't run as the programs of the rebase aren't allowed to start")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use gix_error::ClassificationMarker;
        match self {
            Error::BareRepository
            | Error::NotInProgress
            | Error::IncompatibleOptions { .. }
            | Error::NoUpstream
            | Error::InvalidUpstream { .. }
            | Error::InvalidBranch { .. }
            | Error::InvalidOnto { .. }
            | Error::MergeBase { .. }
            | Error::NoSuchRef { .. }
            | Error::NothingToDo
            | Error::ProgramsDenied => Some(const { &ClassificationMarker::VALIDATION }),
            Error::InProgress { .. }
            | Error::UsedByWorktree { .. }
            | Error::DirtyWorktree { .. }
            | Error::UnresolvedConflicts => Some(const { &ClassificationMarker::CONFLICT }),
            Error::PreRebaseHookRefused => None,
        }
    }
}
