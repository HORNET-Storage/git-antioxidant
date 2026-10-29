//! The states of rebases that Git stopped, which `gix-sequencer` reads and writes byte for byte as it takes the same
//! steps, and the reference log messages of rebases that ended or were aborted.

use bstr::{BString, ByteSlice};
use gix_ref::FullName;
use gix_sequencer::{
    rebase::{AuthorScript, Changes, Commit, Dir, Options, State, Stop, UpdateRef, name, reflog_message},
    refs,
    todo::{ById, Command, Flags, Format, default_comment},
};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, assert_points_as_git_wrote, assert_same_files, hash, log_message, new_target};

/// The reference log action of the rebases.
const ACTION: &str = "rebase";

impl Case {
    /// The state of a rebase of `topic` onto `onto` with `options`, as the case names its commits.
    fn state(&self, options: Options) -> TestResult<State> {
        Ok(State {
            head_name: Some(FullName::try_from("refs/heads/topic")?),
            onto: self.rev("onto"),
            orig_head: self.rev("orig-head"),
            options,
            squash_onto: None,
        })
    }

    /// The state directory of the rebase of the case.
    fn rebase_dir(&self) -> Dir {
        Dir::new(&self.git_dir, hash())
    }
}

/// A state directory of a rebase in a new temporary git directory, which holds `state` as the start of a rebase writes it.
fn started(state: &State) -> TestResult<(gix_testtools::tempfile::TempDir, Dir)> {
    let tmp = gix_testtools::tempfile::tempdir()?;
    let dir = Dir::new(tmp.path(), hash());
    dir.start(state)?;
    Ok((tmp, dir))
}

/// Write the author of the commit `name` of `case` to `author-script` of `dir`, as a `pick` does before it applies it.
fn write_author_script(case: &Case, dir: &Dir, name: &str) -> TestResult {
    let (author, _) = case.author_and_message(name)?;
    Ok(dir.write_author_script(&AuthorScript::from_author_header(author.as_bstr()))?)
}

#[test]
fn edit_stops_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("edit")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(state, case.state(Options::default())?);

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 1, Format::default())?;
    ours.clear_stop()?;
    let begun = ours.begin(&mut progress)?.ok_or("the edit begins")?;
    assert_eq!(progress.list.lines[begun.index].item.command(), Some(Command::Edit));
    let b = case.rev("b");
    write_author_script(&case, &ours, "b")?;
    let (_, message) = case.author_and_message("b")?;
    let patch = theirs.read_file(name::PATCH)?.ok_or("Git wrote the patch")?;
    let edits = ours.stop(&Stop {
        commit: Some(b),
        patch: Some(&patch),
        prepared_message: None,
        commit_message: Some(&message),
        amend: Some(b),
    })?;
    assert_eq!(edits.len(), 1);
    assert_points_as_git_wrote(&case, &edits[0], "REBASE_HEAD")?;
    assert_eq!(
        new_target(&refs::orig_head(state.orig_head, ACTION.into())),
        Some(case.ref_target("ORIG_HEAD")?)
    );
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn continue_after_an_edit_records_the_commit_as_git_does() -> TestResult {
    let case = Case::new("continued")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(state, case.state(Options::default())?);

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 1, Format::default())?;
    ours.clear_stop()?;
    ours.begin(&mut progress)?.ok_or("the edit begins")?;
    let b = case.rev("b");
    write_author_script(&case, &ours, "b")?;
    let (_, message) = case.author_and_message("b")?;
    ours.stop(&Stop {
        commit: Some(b),
        patch: Some(b"the patch of b"),
        prepared_message: None,
        commit_message: Some(&message),
        amend: Some(b),
    })?;

    let mut progress = ours.progress(default_comment(), &mut ById::new(&case.objects, hash()))?;
    assert_eq!((progress.current, progress.done, progress.total), (0, 2, 4));
    let outcome = ours.prepare_continue(&state, &progress, Changes::default(), Some(b), None)?;
    assert_eq!(
        outcome.commit, None,
        "nothing is staged, and the commit is amended already"
    );
    assert!(outcome.remove_merge_message);
    assert_eq!(
        outcome
            .edits
            .iter()
            .map(|edit| edit.name.as_bstr().to_owned())
            .collect::<Vec<_>>(),
        [BString::from("CHERRY_PICK_HEAD")]
    );
    ours.continue_stopped(&progress, Some(b))?;
    ours.clear_stop()?;
    let begun = ours.begin(&mut progress)?.ok_or("the break begins")?;
    assert_eq!(progress.list.lines[begun.index].item.command(), Some(Command::Break));
    assert_eq!(ours.rewritten()?, [(b, b)], "the edited commit is its own rewrite");
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn break_stops_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("break")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(
        state,
        case.state(Options {
            quiet: true,
            ignore_date: true,
            drop_redundant_commits: true,
            ..Default::default()
        })?
    );

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 0, Format::default())?;
    ours.clear_stop()?;
    let begun = ours.begin(&mut progress)?.ok_or("the break begins")?;
    assert_eq!(progress.list.lines[begun.index].item.command(), Some(Command::Break));
    assert_eq!(
        begun
            .edits
            .iter()
            .map(|edit| edit.name.as_bstr().to_owned())
            .collect::<Vec<_>>(),
        [BString::from("AUTO_MERGE"), "REBASE_HEAD".into()]
    );
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn all_options_are_kept_as_git_keeps_them() -> TestResult {
    let case = Case::new("options")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(
        state,
        case.state(Options {
            verbose: true,
            strategy: Some("ort".into()),
            strategy_options: vec!["patience".into(), "ignore-space-change".into()],
            rerere_autoupdate: Some(true),
            gpg_sign: Some("key".into()),
            signoff: true,
            keep_redundant_commits: true,
            committer_date_is_author_date: true,
            reschedule_failed_exec: true,
            trailers: vec![
                "Reviewed-by: R <r@example.com>".into(),
                "Acked-by: A <a@example.com>".into()
            ],
            ..Default::default()
        })?
    );

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 0, Format::default())?;
    ours.clear_stop()?;
    ours.begin(&mut progress)?.ok_or("the break begins")?;
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn conflict_stops_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("conflict")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(state, case.state(Options::default())?);

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 0, Format::default())?;
    ours.clear_stop()?;
    for (commit, made) in [("a", "new-a"), ("b", "new-b")] {
        ours.begin(&mut progress)?.ok_or("a pick begins")?;
        write_author_script(&case, &ours, commit)?;
        ours.record_rewritten(case.rev(commit), progress.peek_command(1), Some(case.rev(made)))?;
        progress.current += 1;
    }
    ours.begin(&mut progress)?.ok_or("the pick of c begins")?;
    write_author_script(&case, &ours, "c")?;
    let (_, message) = case.author_and_message("c")?;
    let patch = theirs.read_file(name::PATCH)?.ok_or("Git wrote the patch")?;
    let prepared = theirs.read_file(name::MESSAGE)?.ok_or("Git wrote the message")?;
    assert!(
        prepared.starts_with(&message),
        "the message of c, followed by the conflicts"
    );
    let edits = ours.stop(&Stop {
        commit: Some(case.rev("c")),
        patch: Some(&patch),
        prepared_message: Some(&prepared),
        commit_message: Some(&message),
        amend: None,
    })?;
    assert_points_as_git_wrote(&case, &edits[0], "REBASE_HEAD")?;
    assert_eq!(
        ours.rewritten()?,
        [(case.rev("a"), case.rev("new-a")), (case.rev("b"), case.rev("new-b"))]
    );
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn failed_exec_stops_with_the_state_that_git_writes() -> TestResult {
    for (case, reschedule) in [("exec", false), ("exec-rescheduled", true)] {
        let case = Case::new(case)?;
        let theirs = case.rebase_dir();
        let state = theirs.state()?;
        assert_eq!(
            state,
            case.state(Options {
                reschedule_failed_exec: reschedule,
                ..Default::default()
            })?
        );

        let (_tmp, ours) = started(&state)?;
        let mut progress = ours.start_todo(&case.edited_list()?, 1, Format::default())?;
        ours.clear_stop()?;
        let begun = ours.begin(&mut progress)?.ok_or("the exec begins")?;
        assert_eq!(progress.list.lines[begun.index].item.command(), Some(Command::Exec));
        if reschedule {
            assert!(
                ours.reschedule(&progress)?.is_empty(),
                "an exec has no commit for REBASE_HEAD"
            );
        }
        assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])?;
    }
    Ok(())
}

#[test]
fn label_records_its_reference_for_removal() -> TestResult {
    let case = Case::new("label")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;

    let (_tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 0, Format::default())?;
    ours.clear_stop()?;
    let begun = ours.begin(&mut progress)?.ok_or("the label begins")?;
    let instruction = progress.list.lines[begun.index].item.instruction().ok_or("a command")?;
    assert_eq!(instruction.command, Command::Label);
    let edit = refs::label(instruction.arg.as_bstr(), state.onto)?;
    assert_points_as_git_wrote(&case, &edit, "refs/rewritten/onto")?;
    ours.add_label(instruction.arg.as_bstr())?;
    progress.current += 1;
    ours.begin(&mut progress)?.ok_or("the break begins")?;
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])?;

    let edits = ours.remove()?;
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit.name.as_bstr().to_owned())
            .collect::<Vec<_>>(),
        [BString::from("refs/rewritten/onto")],
        "the end of the rebase removes the references of its labels"
    );
    assert!(!ours.is_in_progress());
    Ok(())
}

#[test]
fn update_ref_records_head_for_its_branch() -> TestResult {
    let case = Case::new("update-ref")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;

    let (_tmp, ours) = started(&state)?;
    let list = case.edited_list()?;
    ours.filter_update_refs(&list, &mut |name| (name == "refs/heads/side").then(|| case.rev("side")))?;
    let mut progress = ours.start_todo(&list, 1, Format::default())?;
    ours.clear_stop()?;
    let begun = ours.begin(&mut progress)?.ok_or("the update-ref begins")?;
    let instruction = progress.list.lines[begun.index].item.instruction().ok_or("a command")?;
    assert_eq!(instruction.command, Command::UpdateRef);
    ours.record_update_ref(instruction.arg.as_bstr(), case.rev("a"))?;
    progress.current += 1;
    ours.begin(&mut progress)?.ok_or("the break begins")?;
    assert_eq!(
        ours.update_refs()?,
        [UpdateRef {
            name: "refs/heads/side".into(),
            before: case.rev("side"),
            after: case.rev("a"),
        }]
    );
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])
}

#[test]
fn finish_writes_the_reference_logs_that_git_writes() -> TestResult {
    let case = Case::new("finished")?;
    let state = case.state(Options::default())?;
    let (_tmp, ours) = started(&state)?;
    let finish = ours.finish(&state, case.rev("head"), ACTION.into())?;
    assert_eq!(finish.branch_edits.len(), 2);
    let branch = &finish.branch_edits[0];
    assert_eq!(branch.name.as_bstr(), "refs/heads/topic".as_bytes().as_bstr());
    assert_eq!(new_target(branch), Some(case.rev("head")));
    let topic_log = case.log_messages("refs/heads/topic")?;
    assert_eq!(log_message(branch), topic_log.last().map(|message| message.as_bstr()));
    let head_log = case.log_messages("HEAD")?;
    assert_eq!(
        log_message(&finish.branch_edits[1]),
        head_log.last().map(|message| message.as_bstr())
    );
    let start = reflog_message(ACTION.into(), Some("start"), Some(b"checkout main"));
    assert!(head_log.contains(&start), "{start} in {head_log:?}");
    assert_eq!(finish.rewritten, None);
    assert!(finish.update_ref_edits.is_empty());
    Ok(())
}

#[test]
fn abort_writes_the_reference_log_that_git_writes() -> TestResult {
    let case = Case::new("aborted")?;
    let state = case.state(Options::default())?;
    let (_tmp, ours) = started(&state)?;
    let edits = ours.abort(&state, ACTION.into())?;
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit.name.as_bstr().to_owned())
            .collect::<Vec<_>>(),
        [
            BString::from("refs/heads/topic"),
            "HEAD".into(),
            "CHERRY_PICK_HEAD".into(),
            "REVERT_HEAD".into(),
            "AUTO_MERGE".into(),
            "REBASE_HEAD".into()
        ]
    );
    assert_eq!(new_target(&edits[0]), Some(state.orig_head));
    let head_log = case.log_messages("HEAD")?;
    assert_eq!(log_message(&edits[1]), head_log.last().map(|message| message.as_bstr()));
    Ok(())
}

#[test]
fn dropped_commits_are_reported_as_git_reports_them() -> TestResult {
    let case = Case::new("dropped")?;
    let theirs = case.rebase_dir();
    let mut objects = ById::new(&case.objects, hash());
    let options = gix_sequencer::todo::ParseOptions::default();
    let backup = theirs.read_file(name::TODO_BACKUP)?.ok_or("Git wrote the backup")?;
    let (old, _) = gix_sequencer::todo::List::parse(&backup, options, &mut objects)?;
    let new = case.edited_list()?;
    let missing = gix_sequencer::todo::missing_commits(&old, &new);
    let shorts = case.shorts()?;
    let report = gix_sequencer::todo::missing_commits_report(&missing, &mut |id| {
        let name = ["b", "c"]
            .into_iter()
            .find(|name| case.rev(name) == id)
            .ok_or_else(|| gix_error::message!("{id} is dropped"))?;
        Ok(shorts[name].clone())
    })?;
    let output = case.file("output")?;
    assert!(output.contains_str(&report), "{report}\nin\n{output}");
    assert!(theirs.has_file(name::DROPPED));
    Ok(())
}

#[test]
fn failed_fixup_stops_in_its_chain_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("squash")?;
    let theirs = case.rebase_dir();
    let state = theirs.state()?;
    assert_eq!(state, case.state(Options::default())?);
    let (a, b, c, head) = (case.rev("a"), case.rev("b"), case.rev("c"), case.rev("head"));

    let (tmp, ours) = started(&state)?;
    let mut progress = ours.start_todo(&case.edited_list()?, 0, Format::default())?;
    ours.clear_stop()?;
    // `pick a` and `squash b` leave their commits pending as rewritten until the chain ends.
    ours.begin(&mut progress)?.ok_or("the pick begins")?;
    write_author_script(&case, &ours, "a")?;
    ours.record_rewritten(a, progress.peek_command(1), None)?;
    progress.current += 1;
    ours.begin(&mut progress)?.ok_or("the squash begins")?;
    write_author_script(&case, &ours, "b")?;
    ours.add_current_fixup(Command::Squash, Flags::default(), b)?;
    ours.record_rewritten(b, progress.peek_command(1), None)?;
    progress.current += 1;
    // `fixup c` adds itself to the chain and its message, which the caller makes, before its merge conflicts.
    ours.begin(&mut progress)?.ok_or("the fixup begins")?;
    let squash_message = theirs
        .read_file(name::SQUASH_MESSAGE)?
        .ok_or("Git wrote the messages so far")?;
    ours.write_file(name::SQUASH_MESSAGE, &squash_message)?;
    ours.add_current_fixup(Command::Fixup, Flags::default(), c)?;
    write_author_script(&case, &ours, "c")?;
    ours.write_failed_squash_messages()?;
    let (_, message) = case.author_and_message("c")?;
    let patch = theirs.read_file(name::PATCH)?.ok_or("Git wrote the patch")?;
    let edits = ours.stop(&Stop {
        commit: Some(c),
        patch: Some(&patch),
        prepared_message: None,
        commit_message: Some(&message),
        amend: Some(head),
    })?;
    assert_points_as_git_wrote(&case, &edits[0], "REBASE_HEAD")?;
    assert_eq!(ours.current_fixups()?, [(Command::Squash, b), (Command::Fixup, c)]);
    assert_same_files(theirs.path(), ours.path(), &[name::TODO_BACKUP])?;
    assert_eq!(
        std::fs::read(tmp.path().join("MERGE_MSG"))?.as_bstr(),
        case.file("MERGE_MSG")?.as_bstr(),
        "the merge gets the messages of the chain too"
    );

    // `git rebase --continue` with the conflict resolved commits into the chain with the message so far.
    let progress = ours.progress(default_comment(), &mut ById::new(&case.objects, hash()))?;
    let staged = Changes {
        unstaged: false,
        uncommitted: true,
    };
    assert_eq!(
        ours.prepare_continue(&state, &progress, staged, Some(head), None)?
            .commit,
        Some(Commit {
            amend: true,
            edit_message: true,
            cleanup_message: false,
            message_file: Some(ours.file(name::MESSAGE)),
            final_fixup: false,
            remove_current_fixups: true,
        })
    );
    // `git rebase --skip` leaves `c` out of the chain, which then ends with the message of `HEAD` to edit.
    assert_eq!(
        ours.prepare_continue(&state, &progress, Changes::default(), Some(head), None)?
            .commit,
        Some(Commit {
            amend: true,
            edit_message: true,
            cleanup_message: false,
            message_file: None,
            final_fixup: true,
            remove_current_fixups: true,
        })
    );
    assert_eq!(ours.current_fixups()?, [(Command::Squash, b)]);
    let mut expected = BString::from("squash ");
    expected.extend_from_slice(b.to_hex().to_string().as_bytes());
    assert_eq!(
        ours.read_file(name::CURRENT_FIXUPS)?.map(BString::from),
        Some(expected),
        "Git 2.56 removes the line feed before the skipped command, which leaves none after the remaining one"
    );
    Ok(())
}
