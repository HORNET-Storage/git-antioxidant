//! The fixture records the state that `git merge` left when it stopped before its commit, which `gix-merge` reads and
//! writes byte for byte, and the removal of that state by `git merge --abort`.

use std::path::{Path, PathBuf};

use bstr::{BString, ByteSlice};
use gix_hash::ObjectId;
use gix_merge::state::{State, conflicts_hint};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

/// The script that makes the cases.
const FIXTURE: &str = "make_merge_state.sh";

/// The files of the state of a merge that `gix-merge` reads and writes.
const STATE_FILES: [&str; 3] = ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE"];

/// The `.git` directory of the case `name`.
fn git_dir(name: &str) -> TestResult<PathBuf> {
    Ok(gix_testtools::scripted_fixture_read_only(FIXTURE)?
        .join(name)
        .join(".git"))
}

/// The ids of the commits that the merge in `git_dir` merges, as `git rev-parse` printed them.
fn heads(git_dir: &Path) -> TestResult<Vec<ObjectId>> {
    let content = std::fs::read(git_dir.join("heads"))?;
    let mut heads = Vec::new();
    for line in content.lines() {
        heads.push(ObjectId::from_hex(line)?);
    }
    Ok(heads)
}

/// The content of the file `name` in `dir`, or `None` if it doesn't exist.
fn content(dir: &Path, name: &str) -> TestResult<Option<BString>> {
    let path = dir.join(name);
    Ok(if path.exists() {
        Some(std::fs::read(path)?.into())
    } else {
        None
    })
}

/// The state of the merge in `git_dir`, which has to be in progress.
fn read(git_dir: &Path) -> TestResult<State> {
    Ok(State::read(git_dir, gix_testtools::object_hash())?.ok_or("the merge is in progress")?)
}

/// Write `state` into an empty directory and assert that it holds the files of the state in `git_dir`, byte for byte,
/// and nothing else.
fn assert_written_as_git_wrote(name: &str, state: &State, git_dir: &Path) -> TestResult {
    let tmp = gix_testtools::tempfile::tempdir()?;
    state.write(tmp.path())?;
    for file in STATE_FILES {
        assert_eq!(
            content(tmp.path(), file)?,
            content(git_dir, file)?,
            "{name}: {file} is written as git wrote it"
        );
    }
    let mut written: Vec<_> = std::fs::read_dir(tmp.path())?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    written.sort();
    assert_eq!(
        written,
        ["MERGE_HEAD", "MERGE_MODE", "MERGE_MSG"],
        "{name}: only the files of the state remain, without their lock files"
    );
    Ok(())
}

#[test]
fn a_merge_that_no_commit_stopped_is_read_as_git_wrote_it() -> TestResult {
    let git_dir = git_dir("no-commit")?;
    assert_eq!(
        read(&git_dir)?,
        State {
            heads: heads(&git_dir)?,
            message: Some("Merge branch 'side'\n".into()),
            no_ff: false,
        },
        "the merged commit, the message of the merge followed by a line feed, and an empty MERGE_MODE"
    );
    Ok(())
}

#[test]
fn a_merge_that_no_ff_keeps_from_fast_forwarding_is_read_with_its_mode() -> TestResult {
    let git_dir = git_dir("no-ff")?;
    assert_eq!(
        read(&git_dir)?,
        State {
            heads: heads(&git_dir)?,
            message: Some("Merge branch 'side'\n".into()),
            no_ff: true,
        },
        "MERGE_MODE holds no-ff"
    );
    Ok(())
}

#[test]
fn merged_tags_and_several_merged_commits_are_read_as_listed() -> TestResult {
    for name in ["tag", "octopus"] {
        let git_dir = git_dir(name)?;
        let state = read(&git_dir)?;
        assert_eq!(
            state.heads,
            heads(&git_dir)?,
            "{name}: MERGE_HEAD lists the id of each merged object on a line of its own"
        );
        assert_eq!(
            state.message,
            content(&git_dir, "MERGE_MSG")?,
            "{name}: the message is read as it is"
        );
    }
    Ok(())
}

#[test]
fn the_state_that_git_wrote_is_written_byte_for_byte() -> TestResult {
    for name in [
        "no-commit",
        "no-ff",
        "conflict",
        "scissors",
        "comment",
        "tag",
        "octopus",
    ] {
        let git_dir = git_dir(name)?;
        assert_written_as_git_wrote(name, &read(&git_dir)?, &git_dir)?;
    }
    Ok(())
}

#[test]
fn the_message_of_a_merge_with_conflicts_ends_with_the_hint_that_lists_them() -> TestResult {
    for (name, comment_prefix, scissors) in [
        ("conflict", "#", false),
        ("scissors", "#", true),
        ("comment", ";", false),
    ] {
        let git_dir = git_dir(name)?;
        let index: gix_index::State = gix_index::File::at(
            git_dir.join("index"),
            gix_testtools::object_hash(),
            false,
            Default::default(),
        )?
        .into();
        let mut message = BString::from("Merge branch 'side'\n");
        message.extend_from_slice(&conflicts_hint(&index, comment_prefix.into(), scissors));
        let state = State {
            heads: heads(&git_dir)?,
            message: Some(message),
            no_ff: false,
        };
        assert_eq!(
            state,
            read(&git_dir)?,
            "{name}: the message of the merge is followed by the hint about the conflicts in dir/file and file"
        );
        assert_written_as_git_wrote(name, &state, &git_dir)?;
    }
    Ok(())
}

#[test]
fn the_state_is_removed_as_git_removes_it_when_the_merge_is_aborted() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_writable(FIXTURE)?;
    let git_dir = fixture.path().join("abort").join(".git");
    let aborted = fixture.path().join("abort-git").join(".git");
    for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", "MERGE_RR", "AUTO_MERGE"] {
        assert!(
            git_dir.join(name).is_file(),
            "{name} belongs to the merge that stopped with conflicts that rerere recorded"
        );
    }

    State::remove(&git_dir)?;
    for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", "MERGE_RR"] {
        assert!(
            !git_dir.join(name).exists() && !aborted.join(name).exists(),
            "{name} is removed, as git removes it"
        );
    }
    assert!(
        git_dir.join("AUTO_MERGE").is_file() && !aborted.join("AUTO_MERGE").exists(),
        "AUTO_MERGE is a reference, which git deletes along with the state and gix-merge leaves to the references"
    );
    assert_eq!(
        State::read(&git_dir, gix_testtools::object_hash())?,
        None,
        "no merge is in progress without MERGE_HEAD"
    );
    State::remove(&git_dir)?;
    Ok(())
}

#[test]
fn missing_files_besides_merge_head_are_read_as_git_commit_reads_them() -> TestResult {
    let tmp = gix_testtools::tempfile::tempdir()?;
    let heads = heads(&git_dir("octopus")?)?;
    let state = State {
        heads: heads.clone(),
        message: None,
        no_ff: false,
    };
    std::fs::write(tmp.path().join("MERGE_MSG"), "a message that has to go\n")?;
    state.write(tmp.path())?;
    assert_eq!(
        content(tmp.path(), "MERGE_MSG")?,
        None,
        "a state without a message removes MERGE_MSG"
    );
    std::fs::remove_file(tmp.path().join("MERGE_MODE"))?;
    assert_eq!(
        read(tmp.path())?,
        state,
        "without MERGE_MSG and MERGE_MODE, there is no message and the merge may fast-forward"
    );

    std::fs::write(tmp.path().join("MERGE_MODE"), "no-ff\n")?;
    assert!(
        !read(tmp.path())?.no_ff,
        "git commit only takes MERGE_MODE for no-ff if it holds exactly that"
    );
    Ok(())
}

#[test]
fn a_merge_head_with_a_line_that_isnt_an_object_id_is_corrupt() -> TestResult {
    let tmp = gix_testtools::tempfile::tempdir()?;
    let head = heads(&git_dir("no-commit")?)?[0];
    for (content, line) in [
        (format!("{head}\n\n"), String::new()),
        (format!("{head}\r\n"), format!("{head}\r")),
        ("HEAD\n".to_owned(), "HEAD".to_owned()),
        (
            format!("{}\n", &head.to_string()[1..]),
            head.to_string()[1..].to_owned(),
        ),
    ] {
        std::fs::write(tmp.path().join("MERGE_HEAD"), &content)?;
        let err = State::read(tmp.path(), gix_testtools::object_hash()).expect_err("the line isn't an object id");
        assert_eq!(
            err.to_string(),
            format!("Corrupt MERGE_HEAD file ({line})"),
            "{content:?}: the line is named as git names it"
        );
    }

    std::fs::write(tmp.path().join("MERGE_HEAD"), format!("{head}"))?;
    assert_eq!(read(tmp.path())?.heads, [head], "the last line may lack its line feed");
    std::fs::write(tmp.path().join("MERGE_HEAD"), "")?;
    assert!(
        read(tmp.path())?.heads.is_empty(),
        "an empty MERGE_HEAD lists no commits, but a merge is in progress"
    );
    Ok(())
}
