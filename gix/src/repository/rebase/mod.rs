//! The methods of [`Repository`](crate::Repository) that rebase branches, as `builtin/rebase.c` and `sequencer.c` rebase
//! them with the merge backend.

use std::{path::PathBuf, sync::atomic::AtomicBool};

use gix_hash::ObjectId;
use gix_sequencer::rebase::{Dir, State};

use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    commit::message::{Cleanup, CommentPrefix},
    error::{bail, message},
    ext::ObjectIdExt,
    rebase::{Error, Outcome, Programs, StepOptions},
};

mod commit;
mod merges;
mod pick;
mod programs;
mod refs;
mod start;
mod todo;

impl Repository {
    /// Rebase the current branch, or [`Options::branch`](crate::rebase::Options::branch), as `git rebase` does with its
    /// merge backend, following `cmd_rebase()`, `run_sequencer_rebase()` and `do_interactive_rebase()` in
    /// `builtin/rebase.c` and `complete_action()`, `checkout_onto()` and `pick_commits()` in `sequencer.c`.
    ///
    /// * The commits to replay and the commit to replay them onto come from the upstream, `--onto`, `--keep-base`,
    ///   `--root` and the fork point as [`plumbing::plan::Plan`](crate::rebase::plumbing::plan::Plan) decides. If the
    ///   branch has a linear history on top of that commit already and nothing asks for more, the branch is up to date
    ///   and nothing is rebased, as Git says `Current branch <name> is up to date.`
    /// * Otherwise the `pre-rebase` hook runs, the state of the rebase is written to `.git/rebase-merge` as Git writes
    ///   it, and the todo list is made, with the commands of `--exec`, `--autosquash` and `--update-refs`. In an
    ///   [interactive](crate::rebase::Options::interactive) rebase, the sequence editor edits the list, which is checked
    ///   as `todo_list_check()` does with `rebase.missingCommitsCheck`.
    /// * Then the commit to replay onto is checked out with a detached `HEAD` and the reflog message
    ///   `rebase (start): checkout <onto>`, `ORIG_HEAD` points to the commit that is rebased, `post-checkout` runs, and
    ///   the commands of the todo list are performed until it is done or a command stops the rebase.
    /// * When it is done, the branch points to the result with the reflog message
    ///   `rebase (finish): <branch> onto <onto>`, `HEAD` points to the branch again with
    ///   `rebase (finish): returning to <branch>`, the branches of `update-ref` commands are updated, `post-rewrite` runs
    ///   with the rewritten commits, and the state of the rebase is removed.
    ///
    /// `should_interrupt` stops the checkouts of the rebase as long as they didn't change the worktree yet.
    ///
    /// ### Refusals
    ///
    /// Where Git refuses to rebase, a [`rebase::Error`](crate::rebase::Error) is returned. Errors after the state of
    /// the rebase was written leave it in place, as Git leaves it, unless the error says otherwise.
    #[doc(alias = "git rebase")]
    pub fn rebase(&self, options: crate::rebase::Options, should_interrupt: &AtomicBool) -> Result<Outcome> {
        start::rebase(self, options, should_interrupt)
    }

    /// Continue the rebase in progress as `git rebase --continue` does, following `cmd_rebase()` and
    /// `sequencer_continue()`: the staged changes are committed as `commit_staged_changes()` commits them, and the
    /// commands of the todo list are performed until it is done or a command stops the rebase again.
    ///
    /// The rebase doesn't continue while the worktree has changes that aren't staged, as
    /// [`rebase::Error::UnresolvedConflicts`](crate::rebase::Error::UnresolvedConflicts) says.
    #[doc(alias = "git rebase --continue")]
    pub fn rebase_continue(&self, options: StepOptions, should_interrupt: &AtomicBool) -> Result<Outcome> {
        let mut sequencer = Sequencer::resume(self, &options, should_interrupt)?;
        if sequencer.worktree_changes()?.unstaged {
            bail!(Error::UnresolvedConflicts);
        }
        sequencer.continue_rebase()
    }

    /// Skip the commit at which the rebase in progress stopped and continue as `git rebase --skip` does: the index and
    /// the worktree are reset to `HEAD`, the state of the stopped merge goes as `remove_branch_state()` removes it, and
    /// the rebase continues as [`rebase_continue()`](Self::rebase_continue()) does.
    #[doc(alias = "git rebase --skip")]
    pub fn rebase_skip(&self, options: StepOptions, should_interrupt: &AtomicBool) -> Result<Outcome> {
        let mut sequencer = Sequencer::resume(self, &options, should_interrupt)?;
        if let Some(head) = sequencer.head_commit()? {
            sequencer.reset_hard(head)?;
        }
        let edits = sequencer.dir.skip()?;
        sequencer.apply_edits(edits)?;
        sequencer.continue_rebase()
    }

    /// Abort the rebase in progress as `git rebase --abort` does: the index and the worktree are reset to the commit at
    /// which the rebase started, the branch points to it again and `HEAD` to the branch, with the reflog message
    /// `rebase (abort): returning to <branch>`, and the state of the rebase is removed.
    #[doc(alias = "git rebase --abort")]
    pub fn rebase_abort(&self, options: StepOptions, should_interrupt: &AtomicBool) -> Result<()> {
        Sequencer::resume(self, &options, should_interrupt)?.abort()
    }

    /// Stop the rebase in progress without changing `HEAD`, the index or the worktree, as `git rebase --quit` does: the
    /// state of the rebase and the references of its labels are removed.
    #[doc(alias = "git rebase --quit")]
    pub fn rebase_quit(&self) -> Result<()> {
        let dir = state_dir(self);
        if !dir.is_in_progress() {
            bail!(Error::NotInProgress);
        }
        let edits = dir.remove()?;
        self.edit_references(edits)?;
        Ok(())
    }
}

/// Return the state directory of a rebase in the Git directory of `repo`.
fn state_dir(repo: &Repository) -> Dir {
    Dir::new(repo.current_dir().join(repo.git_dir()), repo.object_hash())
}

/// A rebase in progress, with what the commands of its todo list need, as `struct replay_opts` and `struct replay_ctx`
/// hold it.
struct Sequencer<'a> {
    /// The repository that is rebased.
    repo: &'a Repository,
    /// The state directory of the rebase.
    dir: Dir,
    /// What the rebase does.
    state: State,
    /// The action in front of the reflog messages, `rebase` unless `GIT_REFLOG_ACTION` says otherwise.
    reflog_action: BString,
    /// Whether editors and the commands of `exec` may start.
    programs: Programs,
    /// Whether a commit whose parent is `HEAD` already is taken as it is instead of being replayed, which
    /// `--force-rebase` turns off.
    allow_ff: bool,
    /// Stops the checkouts as long as they didn't change the worktree.
    should_interrupt: &'a AtomicBool,
    /// The prefix of comment lines, `core.commentChar` or `core.commentString`.
    comment: BString,
    /// `commit.cleanup` if it is set to a mode that the sequencer takes, as `explicit_cleanup` says.
    cleanup: Option<Cleanup>,
    /// The message of the commit that the current command makes, as `ctx->message` holds it.
    message: BString,
    /// Whether `message` holds the message of the current command, as `ctx->have_message` says.
    have_message: bool,
}

impl<'a> Sequencer<'a> {
    /// Prepare to act on the rebase in progress of `repo` with `options`, as `read_basic_state()` reads its state, or
    /// fail if there is none.
    fn resume(repo: &'a Repository, options: &StepOptions, should_interrupt: &'a AtomicBool) -> Result<Self> {
        if repo.workdir().is_none() {
            bail!(Error::BareRepository);
        }
        let dir = state_dir(repo);
        if !dir.is_in_progress() {
            bail!(Error::NotInProgress);
        }
        let state = dir.state()?;
        Sequencer::new(
            repo,
            dir,
            state,
            options.reflog_action.clone(),
            options.programs,
            should_interrupt,
        )
    }

    /// Prepare the sequencer of the rebase of `repo` whose state `dir` holds, as `get_replay_opts()` and
    /// `sequencer_init_config()` prepare it.
    fn new(
        repo: &'a Repository,
        dir: Dir,
        state: State,
        reflog_action: Option<BString>,
        programs: Programs,
        should_interrupt: &'a AtomicBool,
    ) -> Result<Self> {
        let comment = match repo.comment_prefix()? {
            CommentPrefix::Fixed(prefix) => prefix,
            CommentPrefix::Auto => "#".into(),
        };
        let cleanup = match repo.commit_cleanup()? {
            Cleanup::Default => None,
            cleanup => Some(cleanup),
        };
        Ok(Sequencer {
            repo,
            dir,
            state,
            reflog_action: reflog_action.unwrap_or_else(|| "rebase".into()),
            programs,
            allow_ff: true,

            should_interrupt,
            comment,
            cleanup,
            message: BString::default(),
            have_message: false,
        })
    }

    /// Return the reflog message `<action> (<sub_action>)`, optionally followed by `: <text>`, as `reflog_message()` in
    /// `sequencer.c` makes it.
    fn reflog_message(&self, sub_action: &str, text: Option<&[u8]>) -> BString {
        gix_sequencer::rebase::reflog_message(self.reflog_action.as_bstr(), Some(sub_action), text)
    }

    /// Return the Git directory, as an absolute path.
    fn git_dir(&self) -> PathBuf {
        self.repo.current_dir().join(self.repo.git_dir())
    }

    /// Return the commit of `HEAD`, or `None` if `HEAD` has no commit.
    fn head_commit(&self) -> Result<Option<ObjectId>> {
        let mut head = self.repo.head()?;
        if head.is_unborn() {
            return Ok(None);
        }
        Ok(Some(head.peel_to_commit()?.id))
    }

    /// Return the commit of `HEAD`, which a rebase in progress needs, or an error if `HEAD` has none.
    fn require_head(&self) -> Result<ObjectId> {
        self.head_commit()?
            .ok_or_else(|| message("could not read HEAD").validation_error())
    }

    /// Return the shortest unique abbreviation of `id`, as `short_commit_name()` writes it.
    fn short_id(&self, id: &gix_hash::oid) -> String {
        id.to_owned().attach(self.repo).shorten_or_id().to_string()
    }

    /// Continue the rebase as `sequencer_continue()` does once `cmd_rebase()` checked the state of the worktree.
    fn continue_rebase(&mut self) -> Result<Outcome> {
        let mut progress = self.read_progress()?;
        if self.dir.has_file(gix_sequencer::rebase::name::DROPPED) {
            if let Some(stop) = self.check_against_backup(&progress.list)? {
                return Ok(Outcome::Stopped(stop));
            }
            self.dir.remove_file(gix_sequencer::rebase::name::DROPPED)?;
        }
        self.commit_staged_changes(&progress)?;
        let head = self.head_commit()?;
        self.dir.continue_stopped(&progress, head)?;
        self.pick_commits(&mut progress)
    }

    /// Read the todo list of the rebase, as `read_populate_todo()` reads it.
    fn read_progress(&self) -> Result<gix_sequencer::rebase::Progress> {
        let mut resolve = gix_sequencer::todo::ById::new(&self.repo.objects, self.repo.object_hash());
        self.dir.progress(self.comment.as_bstr(), &mut resolve)
    }

    /// Abort the rebase as `cmd_rebase()` does for `--abort`, with `reset_head()`, `remove_branch_state()` and
    /// `finish_rebase()`.
    fn abort(self) -> Result<()> {
        self.reset_hard(self.state.orig_head)?;
        let edits = self.dir.abort(&self.state, self.reflog_action.as_bstr())?;
        self.apply_edits(edits)?;
        self.dir.remove_merge_files()?;
        let edits = self.dir.remove()?;
        self.apply_edits(edits)
    }

    /// Return whether the worktree has changes that aren't staged and the index changes that aren't committed, ignoring
    /// submodules, as `has_unstaged_changes()` and `has_uncommitted_changes()` tell.
    fn worktree_changes(&self) -> Result<gix_sequencer::rebase::Changes> {
        worktree_changes(self.repo)
    }
}

/// Return whether the worktree of `repo` has changes that aren't staged and its index changes that aren't committed,
/// ignoring submodules, as `has_unstaged_changes()` and `has_uncommitted_changes()` tell.
///
/// The index is read from its file, as Git reads it for each check and the other steps of the rebase read it: the
/// snapshot that [`Repository::index()`] shares is only read again once the modification time of the file increased,
/// which a write within the same tick of the clock of the file system leaves unchanged.
fn worktree_changes(repo: &Repository) -> Result<gix_sequencer::rebase::Changes> {
    let index = repo.read_index_file()?;
    let head_tree = repo.head_tree_id_or_empty()?.detach();
    let mut uncommitted = false;
    let mut pathspec = repo.pathspec(
        false,
        None::<&str>,
        false,
        &gix_index::State::new(repo.object_hash()),
        gix_worktree::stack::state::attributes::Source::IdMapping,
    )?;
    repo.tree_index_status(
        &head_tree,
        &index,
        Some(&mut pathspec),
        crate::status::tree_index::TrackRenames::Disabled,
        |change, _, _| {
            let is_submodule = match &change {
                gix_diff::index::ChangeRef::Addition { entry_mode, .. }
                | gix_diff::index::ChangeRef::Deletion { entry_mode, .. } => entry_mode.is_submodule(),
                gix_diff::index::ChangeRef::Modification {
                    previous_entry_mode,
                    entry_mode,
                    ..
                } => previous_entry_mode.is_submodule() && entry_mode.is_submodule(),
                gix_diff::index::ChangeRef::Rewrite { .. } => false,
            };
            if is_submodule {
                return Ok(std::ops::ControlFlow::Continue(()));
            }
            uncommitted = true;
            Ok(std::ops::ControlFlow::Break(()))
        },
    )?;
    let unstaged = repo
        .status(gix_utils::progress::Discard)?
        .index(index.into())
        .index_worktree_rewrites(None)
        .index_worktree_submodules(crate::status::Submodule::Given {
            ignore: crate::submodule::config::Ignore::All,
            check_dirty: false,
        })
        .index_worktree_options_mut(|options| {
            options.dirwalk_options = None;
        })
        .into_index_worktree_iter(Vec::new())?
        .take_while(std::result::Result::is_ok)
        .next()
        .is_some();
    Ok(gix_sequencer::rebase::Changes { unstaged, uncommitted })
}

/// Return the first line of `message` after its leading empty lines, without its line feed, as
/// `find_commit_subject()` finds the subject of a commit.
fn subject_line(message: &BStr) -> &BStr {
    let rest = skip_blank_lines(message);
    rest.lines().next().unwrap_or_default().as_bstr()
}

/// Return `message` without its leading lines that have nothing but whitespace, as `skip_blank_lines()` skips them.
fn skip_blank_lines(message: &BStr) -> &BStr {
    let mut rest = message;
    while let Some(line) = rest.lines_with_terminator().next() {
        if line.iter().any(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r')) {
            break;
        }
        rest = &rest[line.len()..];
    }
    rest
}

/// Return the message of a commit from its subject on, as `find_commit_subject()` finds it after the headers.
fn message_from_subject(message: &BStr) -> &BStr {
    skip_blank_lines(message)
}
