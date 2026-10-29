//! The commands of the todo list of a rebase, performed as `pick_commits()` performs them, the commands that pick
//! commits as `pick_one_commit()` and `do_pick_commit()` perform them, and the end of a rebase.

use gix_hash::ObjectId;
use gix_ref::{
    Target,
    transaction::{PreviousValue, RefEdit},
};
use gix_sequencer::{
    rebase::{AuthorScript, Progress, Stop as StopFiles, name},
    refs,
    todo::{Command, Flags as TodoFlags, Instruction},
};

use super::{
    Sequencer,
    commit::{Committed, Flags, MERGE_MSG, read_file, remove_file, write_file},
    message_from_subject,
    programs::allow_exec,
    skip_blank_lines, subject_line,
};
use crate::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    error::{ResultExt, bail, message},
    rebase::{Outcome, Stop, StopReason},
};

/// What a command of the todo list came to.
pub(super) enum Step {
    /// The rebase goes on with the next command, after reading the todo list again with `check_todo` if it changed.
    Next {
        /// Whether a program ran that may have changed the todo list.
        check_todo: bool,
    },
    /// The rebase stops.
    Stop(Stop),
    /// The command failed in a way that lets it run again: it goes back to the start of the todo list, and the rebase
    /// stops for the reason given.
    Reschedule(String),
}

/// What `do_pick_commit()` came to, as `enum pick_result` says.
enum Picked {
    /// The commit was made.
    Ok,
    /// The changes of the commit didn't apply without conflicts, or no commit was made of them for the reason given.
    Conflicts(Option<String>),
    /// The commit became empty and was dropped.
    Dropped,
    /// The command failed before it changed anything, for the reason given, and runs again later.
    Error(String),
}

/// What a merge of the changes of a commit came to, as `do_recursive_merge()` returns it.
pub(super) enum Merged {
    /// The changes applied without conflicts.
    Clean,
    /// The changes applied with conflicts, which the index and the worktree hold.
    Conflicts,
    /// The merge couldn't start or its result couldn't be checked out, for the reason given.
    Refused(String),
}

impl Sequencer<'_> {
    /// Perform the commands of `progress` until the todo list is done or a command stops the rebase, as `pick_commits()`
    /// does, and end the rebase when the list is done.
    pub(super) fn pick_commits(&mut self, progress: &mut Progress) -> Result<Outcome> {
        self.dir.clear_stop()?;
        loop {
            let next = progress
                .list
                .lines
                .get(progress.current)
                .and_then(|line| line.item.command());
            if next == Some(Command::Exec) {
                allow_exec(self.repo, self.programs)?;
            }
            let Some(begun) = self.dir.begin(progress)? else {
                break;
            };
            self.apply_edits(begun.edits)?;
            let Some(instruction) = progress.list.lines[begun.index].item.instruction().cloned() else {
                progress.current += 1;
                continue;
            };
            if instruction.command == Command::Break {
                return Ok(Outcome::Stopped(self.stopped_at_head()?));
            }
            self.message.clear();
            self.have_message = false;
            let step = match instruction.command {
                Command::Pick | Command::Edit | Command::Reword | Command::Fixup | Command::Squash => {
                    self.pick_one_commit(progress, &instruction)?
                }
                Command::Exec => self.exec_command(progress, instruction.arg.as_bstr())?,
                Command::Label => self.do_label(instruction.arg.as_bstr())?,
                Command::Reset => self.do_reset(instruction.arg.as_bstr())?,
                Command::Merge => self.do_merge(progress, &instruction)?,
                Command::UpdateRef => self.do_update_ref(instruction.arg.as_bstr())?,
                Command::Revert => Step::Reschedule("'revert' isn't supported in a rebase".into()),
                Command::Noop | Command::Drop | Command::Break => Step::Next { check_todo: false },
            };
            match step {
                Step::Next { check_todo } => {
                    if check_todo && self.todo_changed(progress)? {
                        *progress = self.read_progress()?;
                    } else {
                        progress.current += 1;
                    }
                }
                Step::Stop(stop) => return Ok(Outcome::Stopped(stop)),
                Step::Reschedule(reason) => {
                    let edits = self.dir.reschedule(progress)?;
                    self.apply_edits(edits)?;
                    let line = progress.list.lines[progress.current].text.clone();
                    let mut text = BString::from(reason.as_str());
                    text.push(b'\n');
                    text.extend_from_slice(&rescheduled_advice(line.as_bstr()));
                    return Ok(Outcome::Stopped(Stop {
                        reason: StopReason::Rescheduled { line, reason },
                        message: text,
                    }));
                }
            }
        }
        self.finish()
    }

    /// Return `true` if the todo list differs from the commands after the current one of `progress`, as
    /// `reread_todo_if_changed()` finds it after a program may have edited it.
    fn todo_changed(&self, progress: &Progress) -> Result<bool> {
        let on_disk = self.dir.read_file(name::TODO)?.unwrap_or_default();
        let expected: Vec<u8> = progress
            .list
            .lines
            .iter()
            .skip(progress.current + 1)
            .flat_map(|line| line.text.iter().copied())
            .collect();
        Ok(on_disk != expected)
    }

    /// Stop for `break`, as `stopped_at_head()` reports it.
    fn stopped_at_head(&self) -> Result<Stop> {
        let message = match self.head_commit()? {
            Some(head) => {
                let commit = self.repo.find_commit(head)?;
                let subject = subject_line(commit.decode()?.message).to_owned();
                format!("Stopped at {} ({subject})", self.short_id(&head))
            }
            None => "Stopped at HEAD".into(),
        };
        Ok(Stop {
            reason: StopReason::Break,
            message: message.into(),
        })
    }

    /// Perform `instruction`, a command that picks a commit, as `pick_one_commit()` does.
    fn pick_one_commit(&mut self, progress: &Progress, instruction: &Instruction) -> Result<Step> {
        let commit = instruction
            .commit
            .ok_or_else(|| message!("'{}' needs a commit", instruction.command.name()).validation_error())?;
        let final_fixup = is_final_fixup(progress);
        let (picked, check_todo) = self.do_pick_commit(instruction, commit, final_fixup)?;
        let arg = instruction.arg.as_bstr();
        Ok(match picked {
            Picked::Error(reason) => Step::Reschedule(reason),
            picked if instruction.command == Command::Edit => {
                let (reason, conflicts, to_amend) = match picked {
                    Picked::Conflicts(None) => (StopReason::Conflicts { commit: Some(commit) }, true, false),
                    Picked::Conflicts(Some(reason)) => (StopReason::Uncommitted { commit, reason }, true, false),
                    Picked::Dropped => (StopReason::Edit { commit }, false, false),
                    Picked::Ok | Picked::Error(_) => (StopReason::Edit { commit }, false, true),
                };
                let stopped = if conflicts {
                    String::new()
                } else {
                    format!("Stopped at {}...  {arg}", self.short_id(&commit))
                };
                Step::Stop(self.error_with_patch(Some(commit), arg, conflicts, to_amend, reason, stopped)?)
            }
            Picked::Ok => {
                let head = self.head_commit()?;
                self.dir.record_rewritten(commit, progress.peek_command(1), head)?;
                Step::Next { check_todo }
            }
            Picked::Dropped => {
                if final_fixup {
                    self.dir.flush_rewritten_pending(self.head_commit()?)?;
                }
                Step::Next { check_todo }
            }
            Picked::Conflicts(reason) if instruction.command.is_fixup() => {
                self.dir.write_failed_squash_messages()?;
                Step::Stop(self.error_with_patch(
                    Some(commit),
                    arg,
                    true,
                    true,
                    stop_reason(commit, reason),
                    String::new(),
                )?)
            }
            Picked::Conflicts(reason) => {
                let head = self.head_commit()?;
                let to_amend = instruction.command == Command::Reword
                    && head.is_some()
                    && (head == Some(commit) || head == self.state.squash_onto);
                Step::Stop(self.error_with_patch(
                    Some(commit),
                    arg,
                    true,
                    to_amend,
                    stop_reason(commit, reason),
                    String::new(),
                )?)
            }
        })
    }

    /// Pick `commit` for `instruction` as `do_pick_commit()` does, as the last command of a chain of `fixup` and
    /// `squash` commands if `final_fixup`, and return what came of it with whether a program ran that may have changed
    /// the todo list.
    fn do_pick_commit(
        &mut self,
        instruction: &Instruction,
        commit: ObjectId,
        final_fixup: bool,
    ) -> Result<(Picked, bool)> {
        let command = instruction.command;
        let sub_action = command.name();
        let git_dir = self.git_dir();
        let merge_message = git_dir.join(MERGE_MSG);
        let mut flags = Flags::default();
        let mut message_file = merge_message.clone();
        let head = self.head_commit()?;
        let mut unborn = head.is_none();
        if self.state.squash_onto.is_some() && head == self.state.squash_onto {
            if command.is_fixup() {
                return Ok((Picked::Error("cannot fixup root commit".into()), false));
            }
            flags.create_root = true;
            unborn = true;
        }
        let empty_tree = ObjectId::empty_tree(self.repo.object_hash());
        let head_tree = match head {
            Some(head) => self.tree_of(head)?,
            None => empty_tree,
        };
        if let Some(reason) = self.dirty_index(if unborn { empty_tree } else { head_tree })? {
            return Ok((Picked::Error(reason), false));
        }

        let original = self.repo.find_commit(commit)?;
        let parents: Vec<ObjectId> = original.parent_ids().map(crate::Id::detach).collect();
        if parents.len() > 1 {
            return Ok((
                Picked::Error(format!("commit {commit} is a merge but no -m option was given.")),
                false,
            ));
        }
        let parent = parents.first().copied();
        let decoded = original.decode()?;
        let raw_message = BString::from(decoded.message);
        let author_header = BString::from(decoded.author);
        let author = decoded.author()?.to_owned()?;
        let commit_tree = decoded.tree();
        drop(decoded);
        let subject = subject_line(raw_message.as_bstr()).to_owned();
        let label = format!("{} ({subject})", self.short_id(&commit));
        let parent_label = format!("parent of {label}");
        let author_script = AuthorScript::from_author_header(author_header.as_bstr());

        if self.allow_ff
            && !command.is_fixup()
            && ((parent.is_some() && parent == head) || (parent.is_none() && unborn))
        {
            self.dir.write_author_script(&author_script)?;
            if let Some(reason) = self.fast_forward_to(commit, head)? {
                return Ok((Picked::Error(reason), false));
            }
            if command != Command::Reword {
                return Ok((Picked::Ok, false));
            }
            return Ok((self.reword(sub_action)?, true));
        }

        self.message = message_from_subject(raw_message.as_bstr()).to_owned();
        self.have_message = true;
        let author = (!command.is_fixup()).then_some(author);
        if command.is_fixup() {
            if let Some(reason) = self.update_squash_messages(command, instruction.flags, commit)? {
                return Ok((Picked::Error(reason), false));
            }
            flags.amend = true;
            if !final_fixup {
                message_file = self.dir.file(name::SQUASH_MESSAGE);
            } else if self.dir.has_file(name::FIXUP_MESSAGE) {
                message_file = self.dir.file(name::FIXUP_MESSAGE);
            } else {
                let destination = git_dir.join("SQUASH_MSG");
                remove_file(&destination)?;
                write_file(&destination, &read_file(&self.dir.file(name::SQUASH_MESSAGE))?)?;
                remove_file(&merge_message)?;
                message_file = destination;
                flags.edit = true;
            }
        }
        self.dir.write_author_script(&author_script)?;
        let base_tree = match parent {
            Some(parent) => self.tree_of(parent)?,
            None => empty_tree,
        };
        let base_label = if parent.is_some() {
            parent_label.as_str()
        } else {
            "(empty tree)"
        };
        let merged = self.do_recursive_merge(
            base_tree,
            head_tree,
            commit_tree,
            base_label.into(),
            label.as_str().into(),
        )?;
        let conflicts = match merged {
            Merged::Clean => false,
            Merged::Conflicts => true,
            Merged::Refused(reason) => return Ok((Picked::Error(reason), false)),
        };
        write_file(&merge_message, &self.message)?;
        if conflicts {
            // `print_advice()` removes `CHERRY_PICK_HEAD` again as the rebase commits the resolution itself.
            self.apply_edits([refs::delete(refs::CHERRY_PICK_HEAD)?])?;
            return Ok((Picked::Conflicts(None), false));
        }
        if matches!(command, Command::Pick | Command::Reword | Command::Edit) {
            self.apply_edits([refs::cherry_pick_head(commit)])?;
        }

        let mut dropped = false;
        match self.allow_empty(commit_tree, base_tree)? {
            AllowEmpty::Yes => flags.allow_empty = true,
            AllowEmpty::Drop => {
                dropped = true;
                self.apply_edits([refs::delete(refs::CHERRY_PICK_HEAD)?, refs::delete(refs::AUTO_MERGE)?])?;
                remove_file(&merge_message)?;
            }
            AllowEmpty::No => {}
        }
        let mut check_todo = false;
        let picked = if dropped {
            Picked::Dropped
        } else {
            let mut committed =
                self.do_commit(Some(&message_file), author.as_ref(), sub_action, flags, Some(commit))?;
            check_todo = flags.edit;
            if command == Command::Reword && matches!(committed, Committed::Made) {
                check_todo = true;
                committed = match self.reword(sub_action)? {
                    Picked::Conflicts(Some(reason)) => Committed::Refused(reason),
                    _ => committed,
                };
            }
            match committed {
                Committed::Made => Picked::Ok,
                Committed::Refused(reason) => Picked::Conflicts(Some(reason)),
            }
        };
        if final_fixup && matches!(picked, Picked::Ok | Picked::Dropped) {
            for file in [name::FIXUP_MESSAGE, name::SQUASH_MESSAGE, name::CURRENT_FIXUPS] {
                self.dir.remove_file(file)?;
            }
        }
        Ok((picked, check_todo))
    }

    /// Let the editor edit the message of the commit of `HEAD`, which `reword` picked, and amend it, as
    /// `do_pick_commit()` does after `fast_forward_edit`.
    fn reword(&self, sub_action: &str) -> Result<Picked> {
        let flags = Flags {
            edit: true,
            verify: true,
            amend: true,
            allow_empty: true,
            ..Default::default()
        };
        Ok(match self.git_commit(None, sub_action, flags)? {
            Committed::Made => Picked::Ok,
            Committed::Refused(reason) => Picked::Conflicts(Some(reason)),
        })
    }

    /// Return why the index keeps a command from picking a commit onto `tree`, as `error_dirty_index()` says it, or
    /// `None` if the index records `tree`.
    fn dirty_index(&self, tree: ObjectId) -> Result<Option<String>> {
        let index = self.repo.read_index_file()?;
        if index.entries().iter().any(|entry| entry.stage_raw() != 0) {
            return Ok(Some("Rebasing is not possible because you have unmerged files.".into()));
        }
        let written = index.write_tree(index.tree(), self.repo, Default::default())?;
        Ok((written.tree_id != tree).then(|| "your local changes would be overwritten by rebase.".into()))
    }

    /// Decide what to do with the commit of `commit_tree`, whose parent has `parent_tree`, if the index records the tree
    /// of `HEAD`, as `allow_empty()` does: a commit that was empty already is kept, and one that became empty is kept,
    /// dropped or stops the rebase as `--empty` says.
    fn allow_empty(&self, commit_tree: ObjectId, parent_tree: ObjectId) -> Result<AllowEmpty> {
        let index = self.repo.read_index_file()?;
        let index_tree = index.write_tree(index.tree(), self.repo, Default::default())?.tree_id;
        let head_tree = match self.head_commit()? {
            Some(head) => self.tree_of(head)?,
            None => ObjectId::empty_tree(self.repo.object_hash()),
        };
        Ok(if index_tree != head_tree {
            AllowEmpty::No
        } else if commit_tree == parent_tree || self.state.options.keep_redundant_commits {
            AllowEmpty::Yes
        } else if self.state.options.drop_redundant_commits {
            AllowEmpty::Drop
        } else {
            AllowEmpty::No
        })
    }

    /// Apply the changes from `base_tree` to `next_tree` to the index and the worktree at `head_tree`, as
    /// `do_recursive_merge()` does with the labels `base_label` and `next_label`. With conflicts, the message of the
    /// command gets the conflicted paths, as `append_conflicts_hint()` adds them.
    pub(super) fn do_recursive_merge(
        &mut self,
        base_tree: ObjectId,
        head_tree: ObjectId,
        next_tree: ObjectId,
        base_label: &BStr,
        next_label: &BStr,
    ) -> Result<Merged> {
        let labels = gix_merge::blob::builtin_driver::text::Labels {
            ancestor: Some(base_label),
            current: Some("HEAD".into()),
            other: Some(next_label),
        };
        let options = self.repo.tree_merge_options()?;
        let mut outcome = self
            .repo
            .merge_trees(base_tree, head_tree, next_tree, labels, options)?;
        self.apply_merge(head_tree, &mut outcome)
    }

    /// Apply `outcome`, a merge into `head_tree`, to the index and the worktree as `merge_switch_to_result()` does, and
    /// add the conflicted paths to the message of the command.
    pub(super) fn apply_merge(
        &mut self,
        head_tree: ObjectId,
        outcome: &mut crate::merge::tree::Outcome<'_>,
    ) -> Result<Merged> {
        let applied = match self.repo.apply_merge_result(head_tree, outcome, self.should_interrupt) {
            Ok(applied) => applied,
            Err(err) => {
                return match err.downcast_any_ref::<crate::merge::apply::Error>() {
                    Some(refused) => Ok(Merged::Refused(refused.to_string())),
                    None => Err(err),
                };
            }
        };
        if !applied.conflicts {
            return Ok(Merged::Clean);
        }
        let index = self.repo.read_index_file()?;
        if self.cleanup == Some(crate::commit::message::Cleanup::Scissors) {
            self.message.push(b'\n');
            add_commented_lines(
                &mut self.message,
                self.comment.as_bstr(),
                format!(
                    "{}Do not modify or remove the line above.\nEverything below it will be ignored.\n",
                    crate::commit::message::CUT_LINE
                )
                .as_bytes(),
            );
            let comment = self.comment.clone();
            self.message.extend_from_slice(&comment);
        }
        self.message.push(b'\n');
        let mut hint = BString::from("Conflicts:\n");
        let mut previous: Option<&BStr> = None;
        for entry in index.entries().iter().filter(|entry| entry.stage_raw() != 0) {
            let path = entry.path(&index);
            if previous != Some(path) {
                hint.push(b'\t');
                hint.extend_from_slice(path);
                hint.push(b'\n');
                previous = Some(path);
            }
        }
        let comment = self.comment.clone();
        add_commented_lines(&mut self.message, comment.as_bstr(), &hint);
        Ok(Merged::Conflicts)
    }

    /// Check out `to` from `from` with `HEAD` following, as `fast_forward_to()` does with the reflog message
    /// `rebase: fast-forward`, or return why the checkout was refused.
    pub(super) fn fast_forward_to(&self, to: ObjectId, from: Option<ObjectId>) -> Result<Option<String>> {
        if let Some(refused) = self.switch_worktree(from, to)? {
            return Ok(Some(refused));
        }
        let expected = match from {
            Some(from) => PreviousValue::MustExistAndMatch(Target::Object(from)),
            None => PreviousValue::Any,
        };
        self.repo.edit_reference(
            RefEdit::update(
                gix_ref::FullName::try_from("HEAD").or_error()?,
                to,
                expected,
                "rebase: fast-forward",
            )
            .with_deref(true),
        )?;
        Ok(None)
    }

    /// Stop the rebase at the command of `commit`, whose todo list argument is `arg`, as `error_with_patch()` does: the
    /// message that the command prepared and the message of `commit` are kept for `git rebase --continue`, `REBASE_HEAD`
    /// points to `commit`, and `amend` names `HEAD` if `to_amend`. `stopped` is what Git printed before.
    pub(super) fn error_with_patch(
        &self,
        commit: Option<ObjectId>,
        arg: &BStr,
        conflicts: bool,
        to_amend: bool,
        reason: StopReason,
        stopped: String,
    ) -> Result<Stop> {
        let commit_message = match commit {
            Some(commit) => {
                let commit = self.repo.find_commit(commit)?;
                Some(BString::from(message_from_subject(commit.decode()?.message)))
            }
            None => None,
        };
        let amend = if to_amend { Some(self.require_head()?) } else { None };
        let edits = self.dir.stop(&StopFiles {
            commit,
            patch: None,
            prepared_message: self.have_message.then_some(self.message.as_slice()),
            commit_message: commit_message.as_ref().map(|message| message.as_slice()),
            amend,
        })?;
        self.apply_edits(edits)?;
        let mut message = stopped;
        let mut add = |line: &str| {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(line);
        };
        if to_amend {
            let gpg_sign = if self.state.options.gpg_sign.is_some() {
                "'-S'"
            } else {
                ""
            };
            add(&format!(
                "You can amend the commit now, with\n\n  git commit --amend {gpg_sign}\n\nOnce you are satisfied with your changes, run\n\n  git rebase --continue"
            ));
        } else if conflicts {
            match commit {
                Some(commit) => add(&format!("Could not apply {}... {arg}", self.short_id(&commit))),
                None => add(&format!("Could not merge {arg}")),
            }
        }
        Ok(Stop {
            reason,
            message: message.into(),
        })
    }

    /// Run the command of an `exec` line as `do_exec()` does, and stop as Git stops if it fails or leaves changes,
    /// putting it back into the todo list with `rebase.rescheduleFailedExec`.
    fn exec_command(&self, progress: &Progress, command: &BStr) -> Result<Step> {
        let executed = self.exec(command)?;
        if executed.success && !executed.dirty {
            return Ok(Step::Next { check_todo: true });
        }
        let rescheduled = !executed.success && self.state.options.reschedule_failed_exec;
        if rescheduled {
            let edits = self.dir.reschedule(progress)?;
            self.apply_edits(edits)?;
        }
        let mut message = if executed.success {
            format!(
                "execution succeeded: {command}\nbut left changes to the index and/or the working tree.\nCommit or stash your changes, and then run\n\n  git rebase --continue\n"
            )
        } else {
            format!(
                "execution failed: {command}\n{}You can fix the problem, and then run\n\n  git rebase --continue\n",
                if executed.dirty {
                    "and made changes to the index and/or the working tree.\n"
                } else {
                    ""
                }
            )
        };
        if rescheduled {
            let line = progress.list.lines[progress.current].text.as_bstr();
            message.push_str(&rescheduled_advice(line).to_str_lossy());
        }
        Ok(Step::Stop(Stop {
            reason: StopReason::Exec {
                command: command.to_owned(),
                success: executed.success,
                dirty: executed.dirty,
                rescheduled,
            },
            message: message.into(),
        }))
    }

    /// End the rebase as `pick_commits()` does once the todo list is done: the branch points to `HEAD` and `HEAD` to
    /// the branch, `post-rewrite` runs with the rewritten commits, the branches of `update-ref` commands are updated, and
    /// the state of the rebase is removed.
    fn finish(&self) -> Result<Outcome> {
        let head = self.require_head()?;
        let finish = self.dir.finish(&self.state, head, self.reflog_action.as_bstr())?;
        self.apply_edits(finish.branch_edits)?;
        #[cfg(feature = "hooks")]
        if let Some(rewritten) = finish.rewritten {
            let invocation = gix_hook::Invocation {
                args: vec!["rebase".into()],
                stdin: gix_hook::invocation::Stdin::Bytes(rewritten),
                ..gix_hook::Invocation::new("post-rewrite")
            };
            self.repo.hooks()?.run(&invocation)?;
        }
        let mut failed = BString::default();
        for edit in finish.update_ref_edits {
            let name = edit.name.as_bstr().to_owned();
            if self.repo.edit_reference(edit).is_err() {
                failed.push(b'\t');
                failed.extend_from_slice(&name);
                failed.push(b'\n');
            }
        }
        if !failed.is_empty() {
            bail!(message!("Failed to update the following refs with --update-refs:\n{failed}").validation());
        }
        let edits = self.dir.remove()?;
        self.apply_edits(edits)?;
        Ok(Outcome::Finished {
            head,
            branch: self.state.head_name.clone(),
        })
    }

    /// Update the messages of a chain of `fixup` and `squash` commands for `command` with `flags`, which melds `commit`
    /// into the chain, as `update_squash_messages()` does, or return why it can't.
    fn update_squash_messages(&self, command: Command, flags: TodoFlags, commit: ObjectId) -> Result<Option<String>> {
        let comment = self.comment.as_bstr();
        let current_fixups = self.dir.read_line(name::CURRENT_FIXUPS)?.unwrap_or_default();
        let mut count = if current_fixups.is_empty() {
            0
        } else {
            1 + current_fixups.find_iter("\n").count()
        };
        let fixup_flag = command == Command::Fixup && (flags.replace_fixup_message || flags.edit_fixup_message);
        let seen_squash = current_fixups.starts_with(b"squash") || current_fixups.find("\nsquash").is_some();
        let mut buf = BString::default();
        if count > 0 {
            buf = match self.dir.read_file(name::SQUASH_MESSAGE)? {
                Some(content) if !content.is_empty() => content.into(),
                _ => {
                    let path = self.dir.file(name::SQUASH_MESSAGE);
                    return Ok(Some(format!("could not read '{}'", path.display())));
                }
            };
            let end = if buf.starts_with(comment) {
                buf.find_byte(b'\n').unwrap_or(buf.len())
            } else {
                0
            };
            let header = format!("{comment} This is a combination of {} commits.", count + 2);
            buf.splice(..end, header.into_bytes());
            if fixup_flag && !seen_squash {
                update_squash_message_for_fixup(&mut buf, comment);
            }
        } else {
            let Some(head) = self.head_commit()? else {
                return Ok(Some("need a HEAD to fixup".into()));
            };
            let head_commit = self.repo.find_commit(head)?;
            let decoded = head_commit.decode()?;
            let body = message_from_subject(decoded.message);
            if command == Command::Fixup && flags == TodoFlags::default() {
                self.dir.write_file(name::FIXUP_MESSAGE, body)?;
            }
            buf.extend_from_slice(format!("{comment} This is a combination of 2 commits.\n{comment} ").as_bytes());
            buf.extend_from_slice(if fixup_flag {
                b"The 1st commit message will be skipped:"
            } else {
                b"This is the 1st commit message:"
            });
            buf.extend_from_slice(b"\n\n");
            if fixup_flag {
                add_commented_lines(&mut buf, comment, body);
            } else {
                buf.extend_from_slice(body);
            }
        }
        let melded = self.repo.find_commit(commit)?;
        let decoded = melded.decode()?;
        let body = message_from_subject(decoded.message);
        if command == Command::Squash || fixup_flag {
            let commented_len = if body.starts_with(b"amend!")
                || ((command == Command::Squash || seen_squash)
                    && (body.starts_with(b"squash!") || body.starts_with(b"fixup!")))
            {
                commit_subject_length(body)
            } else {
                0
            };
            count += 1;
            buf.extend_from_slice(format!("\n{comment} This is the commit message #{}:\n\n", count + 1).as_bytes());
            add_commented_lines(&mut buf, comment, &body[..commented_len]);
            let fixup_start = buf.len();
            buf.extend_from_slice(&body[commented_len..]);
            if fixup_flag
                && !seen_squash
                && flags.replace_fixup_message
                && (self.dir.has_file(name::FIXUP_MESSAGE) || !self.dir.has_file(name::SQUASH_MESSAGE))
            {
                let fixup_message = skip_blank_lines(buf[fixup_start..].as_bstr()).to_owned();
                self.dir.write_file(name::FIXUP_MESSAGE, &fixup_message)?;
            } else {
                self.dir.remove_file(name::FIXUP_MESSAGE)?;
            }
        } else {
            count += 1;
            buf.extend_from_slice(
                format!("\n{comment} The commit message #{} will be skipped:\n\n", count + 1).as_bytes(),
            );
            add_commented_lines(&mut buf, comment, body);
        }
        self.dir.write_file(name::SQUASH_MESSAGE, &buf)?;
        self.dir.add_current_fixup(command, flags, commit)?;
        Ok(None)
    }
}

/// What to do with a commit that leaves the tree of `HEAD` as it is, as `allow_empty()` says.
enum AllowEmpty {
    /// Commit it as an empty commit.
    Yes,
    /// Leave it to `git commit`, which refuses it, or commit it as it isn't empty.
    No,
    /// Drop it.
    Drop,
}

/// Return the reason of a stop for `commit` whose changes didn't apply, or whose commit was refused for `reason`.
fn stop_reason(commit: ObjectId, reason: Option<String>) -> StopReason {
    match reason {
        None => StopReason::Conflicts { commit: Some(commit) },
        Some(reason) => StopReason::Uncommitted { commit, reason },
    }
}

/// Return `true` if the current command of `progress` is the last `fixup` or `squash` of its chain, as
/// `is_final_fixup()` tells.
fn is_final_fixup(progress: &Progress) -> bool {
    let lines = &progress.list.lines;
    if !lines
        .get(progress.current)
        .and_then(|line| line.item.command())
        .is_some_and(Command::is_fixup)
    {
        return false;
    }
    for line in lines.iter().skip(progress.current + 1) {
        if line.item.command().is_some_and(Command::is_fixup) {
            return false;
        }
        if !line.item.is_noop() {
            break;
        }
    }
    true
}

/// Return the advice of Git for a command that it put back into the todo list, `rescheduled_advice`, with the line of
/// the command.
fn rescheduled_advice(line: &BStr) -> BString {
    let mut out = BString::from("Could not execute the todo command\n\n    ");
    out.extend_from_slice(line);
    if !line.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(
        b"\nIt has been rescheduled; To edit the command before continuing, please\nedit the todo list first:\n\n    git rebase --edit-todo\n    git rebase --continue\n",
    );
    out
}

/// Append `text` to `out` with each line commented out by `comment` and a space unless the line is empty or starts
/// with a tab, completing the last line, as `strbuf_add_commented_lines()` does.
pub(super) fn add_commented_lines(out: &mut BString, comment: &BStr, text: &[u8]) {
    for line in text.lines_with_terminator() {
        out.extend_from_slice(comment);
        if !line.starts_with(b"\n") && !line.starts_with(b"\t") {
            out.push(b' ');
        }
        out.extend_from_slice(line);
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
}

/// Append `text` to `out` like [`add_commented_lines()`], but keep its leading lines that are comments already as they
/// are, as the `add_commented_lines()` of `sequencer.c` does to avoid commenting subjects twice.
fn add_commented_lines_once(out: &mut BString, comment: &BStr, text: &[u8]) {
    let mut rest = text;
    while rest.starts_with(comment) {
        let end = rest.find_byte(b'\n').map_or(rest.len(), |end| end + 1);
        out.extend_from_slice(&rest[..end]);
        rest = &rest[end..];
    }
    add_commented_lines(out, comment, rest);
}

/// Return the length of the first paragraph of `body` with its line endings, as `commit_subject_length()` finds it.
fn commit_subject_length(body: &BStr) -> usize {
    let mut position = 0;
    while position < body.len() {
        let rest = body[position..].as_bstr();
        if skip_blank_lines(rest).len() != rest.len() {
            break;
        }
        position = rest.find_byte(b'\n').map_or(body.len(), |end| position + end + 1);
    }
    position
}

/// Comment out the commit messages of `message`, the messages of a chain of `fixup` and `squash` commands so far, that
/// aren't commented out yet, as `update_squash_message_for_fixup()` does for `fixup -C` and `fixup -c`, saying that
/// they will be skipped.
fn update_squash_message_for_fixup(message: &mut BString, comment: &BStr) {
    let header = |text: String| {
        let mut line = BString::from(comment);
        line.push(b' ');
        line.extend_from_slice(text.as_bytes());
        line.push(b'\n');
        line
    };
    let mut kept = header("This is the 1st commit message:".into());
    let mut skipped = header("The 1st commit message will be skipped:".into());
    let original = std::mem::take(message);
    let mut commented = false;
    let copy = |out: &mut BString, text: &[u8], commented: bool| {
        if commented {
            add_commented_lines_once(out, comment, text);
        } else {
            out.extend_from_slice(text);
        }
    };
    let (mut start, mut position, mut number) = (0, Some(0), 1);
    while let Some(at) = position {
        let rest = &original[at..];
        let off = usize::from(at > start + 1 && original[at - 2] == b'\n');
        if rest.starts_with(&kept) {
            copy(message, &original[start..at - off], commented);
            if off == 1 {
                message.push(b'\n');
            }
            message.extend_from_slice(&skipped);
            let mut next = at + kept.len();
            if original.get(next) == Some(&b'\n') {
                message.push(b'\n');
                next += 1;
            }
            start = next;
            position = Some(next);
            commented = true;
        } else if rest.starts_with(&skipped) {
            copy(message, &original[start..at - off], commented);
            start = at - off;
            position = Some(at + skipped.len());
            commented = false;
        } else {
            position = rest.find_byte(b'\n').map(|end| at + end + 1);
            continue;
        }
        number += 1;
        kept = header(format!("This is the commit message #{number}:"));
        skipped = header(format!("The commit message #{number} will be skipped:"));
    }
    copy(message, &original[start..], commented);
}
