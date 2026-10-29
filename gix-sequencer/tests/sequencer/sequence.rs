//! The states of cherry-pick and revert sequences that Git stopped, which `gix-sequencer` reads and writes byte for byte
//! as it takes the same steps.

use bstr::{BStr, BString, ByteSlice};
use gix_sequencer::{
    sequence::{Action, Cleanup, Continue, DIR, Dir, Options, Progress, Rollback, todo_line},
    todo::{List, Resolved, default_comment},
};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, assert_same_files, copy_dir, hash};

/// Take the steps of the sequence of `action` in `case`, which applied `commits` with the commit at which each left
/// `HEAD` after it, in a new git directory, and assert that they write the state that Git left.
fn assert_steps_write_the_state_that_git_writes(
    case: &Case,
    action: Action,
    commits: &[(&str, &str)],
    options: &Options,
) -> TestResult {
    let tmp = gix_testtools::tempfile::tempdir()?;
    let ours = Dir::new(tmp.path(), hash());
    ours.start(action, Some(case.rev("head")), options, false)?;
    let shorts = case.shorts()?;
    let mut lines = Vec::new();
    for (commit, _) in commits {
        let subject = case.subject(commit)?;
        lines.push(todo_line(
            action,
            case.rev(commit),
            shorts[*commit].as_bstr(),
            subject.as_bstr(),
        ));
    }
    let mut progress = Progress {
        list: List { lines },
        current: 0,
    };
    for (_, head) in commits {
        assert_eq!(ours.begin(&progress)?, Some(progress.current));
        ours.update_abort_safety(Some(case.rev(head)))?;
        progress.current += 1;
    }
    assert_same_files(Dir::new(&case.git_dir, hash()).path(), ours.path(), &[])
}

/// A resolver of the abbreviated ids of `case`, which names its commits as Git names them in the todo list of a sequence.
fn resolver(case: &Case) -> TestResult<impl FnMut(&BStr) -> gix_error::Result<Resolved> + '_> {
    let shorts = case.shorts()?;
    Ok(move |name: &BStr| {
        Ok(match shorts.iter().find(|(_, short)| short.as_bstr() == name) {
            Some((commit, _)) => Resolved::Commit {
                id: case.rev(commit),
                is_merge: false,
            },
            None => Resolved::Nothing,
        })
    })
}

/// The names of the references that `edits` change.
fn names(edits: &[gix_ref::transaction::RefEdit]) -> Vec<BString> {
    edits.iter().map(|edit| edit.name.as_bstr().to_owned()).collect()
}

#[test]
fn cherry_pick_stops_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("cherry-pick")?;
    let theirs = Dir::new(&case.git_dir, hash());
    let opts = case.file("sequencer/opts")?;
    let options = Options::from_bytes(&opts)?;
    assert_eq!(
        options,
        Options {
            edit: Some(false),
            allow_empty: true,
            allow_empty_message: true,
            keep_redundant_commits: true,
            signoff: true,
            record_origin: true,
            strategy: Some("ort".into()),
            strategy_options: vec!["patience".into(), "ignore-space-change".into()],
            rerere_autoupdate: Some(true),
            default_message_cleanup: Some(Cleanup::Scissors),
            ..Default::default()
        }
    );
    assert_eq!(options.to_bytes(), opts, "written as Git writes it");
    assert_eq!(theirs.options()?, options);
    assert_eq!(theirs.head()?, case.rev("head"));
    assert_eq!(theirs.last_action()?, Some(Action::CherryPick));
    assert_steps_write_the_state_that_git_writes(
        &case,
        Action::CherryPick,
        &[("a", "new-a"), ("b", "new-b"), ("c", "new-b")],
        &options,
    )?;

    let head = Some(case.rev("new-b"));
    assert!(theirs.rollback_is_safe(head)?);
    assert_eq!(theirs.rollback(true, head)?, Rollback::ResetTo(case.rev("head")));
    assert_eq!(
        theirs.rollback(true, Some(case.rev("new-a")))?,
        Rollback::Moved,
        "HEAD moved since the last pick"
    );
    theirs.skip(Action::CherryPick, true, head)?;

    let Continue::Sequence { commit, edit, progress } =
        theirs.prepare_continue(Action::CherryPick, true, default_comment(), &mut resolver(&case)?)?
    else {
        return Err("a sequence continues".into());
    };
    assert!(commit, "the stopped pick is committed first");
    assert_eq!(edit, Some(false));
    assert_eq!(progress.current, 1, "the stopped pick is done then");
    assert_eq!(
        progress
            .list
            .instructions()
            .map(|instruction| instruction.commit)
            .collect::<Vec<_>>(),
        [Some(case.rev("c"))]
    );

    let error = theirs
        .start(Action::Revert, head, &Options::default(), true)
        .err()
        .ok_or("a sequence is in progress")?;
    let git_says = case.file("in-progress")?;
    assert!(
        git_says.starts_with(format!("error: {error}\n").as_bytes()),
        "{error}\nas Git says it in\n{git_says}"
    );

    let tmp = gix_testtools::tempfile::tempdir()?;
    copy_dir(theirs.path(), &tmp.path().join(DIR))?;
    let copy = Dir::new(tmp.path(), hash());
    assert_eq!(
        names(&copy.post_commit_cleanup(true, false)?),
        [BString::from("CHERRY_PICK_HEAD"), "AUTO_MERGE".into()]
    );
    assert!(!copy.is_in_progress(), "the stopped pick was the last");
    Ok(())
}

#[test]
fn revert_stops_with_the_state_that_git_writes() -> TestResult {
    let case = Case::new("revert")?;
    let theirs = Dir::new(&case.git_dir, hash());
    let opts = case.file("sequencer/opts")?;
    let options = Options::from_bytes(&opts)?;
    assert_eq!(
        options,
        Options {
            edit: Some(false),
            signoff: true,
            default_message_cleanup: Some(Cleanup::Verbatim),
            ..Default::default()
        }
    );
    assert_eq!(options.to_bytes(), opts, "written as Git writes it");
    assert_eq!(theirs.last_action()?, Some(Action::Revert));
    assert_steps_write_the_state_that_git_writes(&case, Action::Revert, &[("g", "new-g"), ("f1", "new-g")], &options)?;

    let head = Some(case.rev("new-g"));
    assert_eq!(theirs.rollback(true, head)?, Rollback::ResetTo(case.rev("head")));
    let error = theirs
        .progress(Action::CherryPick, default_comment(), &mut resolver(&case)?)
        .err()
        .ok_or("the todo list is that of a revert")?;
    assert_eq!(error.to_string(), "cannot cherry-pick during a revert.");
    let Continue::Sequence { progress, .. } =
        theirs.prepare_continue(Action::Revert, true, default_comment(), &mut resolver(&case)?)?
    else {
        return Err("a sequence continues".into());
    };
    assert_eq!(
        progress
            .list
            .instructions()
            .map(|instruction| instruction.commit)
            .collect::<Vec<_>>(),
        [Some(case.rev("f1"))]
    );

    let tmp = gix_testtools::tempfile::tempdir()?;
    copy_dir(theirs.path(), &tmp.path().join(DIR))?;
    let copy = Dir::new(tmp.path(), hash());
    assert_eq!(
        names(&copy.post_commit_cleanup(false, true)?),
        [BString::from("REVERT_HEAD"), "AUTO_MERGE".into()]
    );
    assert!(!copy.is_in_progress(), "the stopped revert was the last");
    Ok(())
}

#[test]
fn options_are_read_as_git_reads_them() -> TestResult {
    let options = Options::from_bytes(
        b"[options]\n\tno-commit = yes\n\tmainline = 2\n\tgpg-sign = \" key\"\n\tstrategy-option = a\n\tstrategy-option = b\n",
    )?;
    assert_eq!(
        options,
        Options {
            no_commit: true,
            mainline: Some(2),
            gpg_sign: Some(" key".into()),
            strategy_options: vec!["a".into(), "b".into()],
            ..Default::default()
        }
    );
    assert_eq!(
        options.to_bytes(),
        "[options]\n\tno-commit = true\n\tmainline = 2\n\tgpg-sign = \" key\"\n\tstrategy-option = a\n\tstrategy-option = b\n",
        "a value with a space at an end is quoted"
    );
    assert!(
        Options::from_bytes(b"[options]\n\tsignoff = 1\n").is_err(),
        "booleans are words"
    );
    assert!(Options::from_bytes(b"[options]\n\tunknown = true\n").is_err());
    assert!(Options::from_bytes(b"[other]\n\tsignoff = true\n").is_err());
    assert_eq!(Options::default().to_bytes(), "", "no file without options");
    Ok(())
}
