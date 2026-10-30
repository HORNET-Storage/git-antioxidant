//! Rebasing with `gix`, compared with what `git rebase` does.
//!
//! The hooks, editors and commands of `exec` lines that the rebases start inherit the environment of the process, which
//! `#[serial]` tests change while they run, so these tests are `#[parallel]` and don't run at the same time as those.

use std::{collections::BTreeMap, path::Path, sync::atomic::AtomicBool};

use gix::{
    bstr::{BStr, BString, ByteSlice},
    rebase::{
        Error, Merges, Options, Outcome, Programs, StepOptions, Stop, StopReason, plumbing::script::RebaseMerges,
    },
};
use serial_test::parallel;

use super::commit::baseline::Scenario;

/// The files of the Git directory besides the state of the rebase that a rebase writes or removes, which `gix` has to
/// leave as Git does, with the records of the editors and of the commands of `exec`, and those of the hooks if they run.
fn state() -> Vec<&'static str> {
    let mut state = vec![
        "HEAD",
        "ORIG_HEAD",
        "REBASE_HEAD",
        "AUTO_MERGE",
        "CHERRY_PICK_HEAD",
        "MERGE_HEAD",
        "MERGE_MSG",
        "MERGE_MODE",
        "SQUASH_MSG",
        "editor-records",
        "exec-records",
    ];
    if cfg!(feature = "hooks") {
        state.push("hook-records");
    }
    state
}

fn scenario(name: &str) -> crate::Result<Scenario> {
    Scenario::new("make_rebase_repos.sh", name)
}

/// The options of a rebase onto `upstream` that starts the editors and commands that `git rebase` starts.
fn onto(upstream: &str) -> Options {
    Options {
        upstream: Some(upstream.into()),
        programs: Programs::Allow,
        ..Default::default()
    }
}

/// The options of an interactive rebase onto `upstream`, as [`onto()`] makes them.
fn interactive(upstream: &str) -> Options {
    Options {
        interactive: true,
        ..onto(upstream)
    }
}

/// The options of the steps of a rebase in progress that start the programs that `git rebase` starts.
fn step() -> StepOptions {
    StepOptions {
        programs: Programs::Allow,
        ..Default::default()
    }
}

fn rebase(scenario: &Scenario, options: Options) -> gix::Result<Outcome> {
    scenario.repo.rebase(options, &AtomicBool::new(false))
}

/// Open the repository of `scenario` again as one whose Git directory isn't fully trusted.
fn untrusted(scenario: &Scenario) -> crate::Result<gix::Repository> {
    Ok(gix::open_opts(
        scenario.path(),
        scenario.repo.open_options().clone().with(gix_sec::Trust::Reduced),
    )?)
}

/// Return where `outcome` stopped, which it has to.
fn stopped(outcome: Outcome) -> Stop {
    match outcome {
        Outcome::Stopped(stop) => stop,
        outcome => panic!("the rebase stops as Git stopped, but it ended with {outcome:?}"),
    }
}

/// Assert that `outcome` ended the rebase of `topic`.
fn assert_finished(outcome: &Outcome) {
    assert!(
        matches!(outcome, Outcome::Finished { branch: Some(branch), .. } if branch.as_bstr() == "refs/heads/topic"),
        "the rebase of `topic` ended as it ended with Git, but it ended with {outcome:?}"
    );
}

/// Assert that `gix` left the repository of `scenario` as Git left its own, including the state of a rebase in progress
/// but its `patch`, which `gix` doesn't write.
fn assert_same_as_git(scenario: &Scenario) -> crate::Result {
    scenario.assert_same_as_git(&state())?;
    assert_eq!(
        rebase_state(scenario.path())?,
        rebase_state(&scenario.git)?,
        "the state of the rebase in progress"
    );
    Ok(())
}

/// Assert as [`assert_same_as_git()`] does, with the copy of the repository of Git at its stop.
fn assert_same_as_git_at_stop(scenario: &mut Scenario) -> crate::Result {
    let mut stopped = scenario.git.clone().into_os_string();
    stopped.push("-stopped");
    let git = std::mem::replace(&mut scenario.git, stopped.into());
    let result = assert_same_as_git(scenario);
    scenario.git = git;
    result
}

/// Assert that Git printed each line of `message` when it stopped in `scenario`.
fn assert_printed_by_git(scenario: &Scenario, message: &BStr) -> crate::Result {
    let mut path = scenario.git.clone().into_os_string();
    path.push(".err");
    let printed = std::fs::read(path)?;
    for line in message.lines() {
        assert!(
            printed.find(line).is_some(),
            "Git printed {:?} where it stopped as well, but printed {:?}",
            line.as_bstr(),
            printed.as_bstr()
        );
    }
    Ok(())
}

/// Return the files of the state of a rebase in progress in the worktree at `root` by their path relative to the state
/// directory, but its `patch`, or nothing if no rebase is in progress.
fn rebase_state(root: &Path) -> crate::Result<BTreeMap<String, BString>> {
    let dir = root.join(".git").join("rebase-merge");
    let mut files = BTreeMap::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    let mut directories = vec![dir.clone()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(&dir)?.to_string_lossy().replace('\\', "/");
            if entry.file_type()?.is_dir() {
                directories.push(path);
            } else if relative != "patch" {
                files.insert(relative, std::fs::read(&path)?.into());
            }
        }
    }
    Ok(files)
}

/// Resolve the conflict of `path` in `repo` with `content` as `git add` does after the file was edited.
fn resolve(repo: &gix::Repository, path: &str, content: &str) -> crate::Result {
    std::fs::write(
        repo.workdir().expect("the scenarios have a worktree").join(path),
        content,
    )?;
    let id = repo.write_blob(content)?.detach();
    let mut index = repo.open_index()?;
    index.remove_entries(|_, entry_path, _| entry_path == path);
    index.dangerously_push_entry(
        Default::default(),
        id,
        gix::index::entry::Flags::empty(),
        gix::index::entry::Mode::FILE,
        path.into(),
    );
    index.sort_entries();
    index.remove_tree();
    index.write(Default::default())?;
    Ok(())
}

/// Return the refusal of `outcome`, which has to fail, with the messages of Git in `scenario`.
fn refusal(scenario: &Scenario, outcome: gix::Result<Outcome>) -> crate::Result<(Error, Vec<String>)> {
    let err = outcome.expect_err("Git refused to rebase as well");
    let refusal = err
        .downcast_any_ref::<Error>()
        .cloned()
        .unwrap_or_else(|| panic!("a refusal to rebase, not {err:?}"));
    Ok((refusal, scenario.git_messages()?))
}

/// Return `true` if the Git of the fixtures stops at the conflict of a `fixup -c` as Git 2.56 does: it records the
/// command with `-c` in `current-fixups` and says that the commit can be amended, where older Git records `fixup` and
/// says that it couldn't apply the commit. Such stops compare only with Git 2.56, and what follows them with any Git.
fn git_stops_fixup_chains_to_amend() -> bool {
    *gix_testtools::GIT_BASH_VERSION >= (2, 56, 0)
}

/// Return `true` if the Git of the fixtures writes todo lists as `gix` and Git 2.50 or newer write them: with the subject
/// of each commit after `# `, as the editors see them, as a stopped rebase keeps them and as the messages at its stops
/// print them, and with the action `merge` in the reflog messages of `merge` commands. Older Git writes the subjects
/// without `# ` and logs `merge` commands as `pick`, so todo lists, the messages that print them and the reflogs of
/// rebased merges compare only with Git 2.50 or newer.
fn git_writes_todo_lists_as_gix() -> bool {
    *gix_testtools::GIT_BASH_VERSION >= (2, 50, 0)
}

#[test]
#[parallel]
fn pick() -> crate::Result {
    let scenario = scenario("pick")?;
    assert_finished(&rebase(&scenario, onto("main"))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn onto_another_commit() -> crate::Result {
    let scenario = scenario("onto")?;
    let options = Options {
        onto: Some("main".into()),
        ..onto("topic~1")
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn given_branch_is_checked_out() -> crate::Result {
    let scenario = scenario("branch-argument")?;
    let options = Options {
        branch: Some("topic".into()),
        ..onto("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn reflog_action() -> crate::Result {
    let scenario = scenario("reflog-action")?;
    let options = Options {
        reflog_action: Some("custom".into()),
        ..onto("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn up_to_date() -> crate::Result {
    let scenario = scenario("up-to-date")?;
    assert_eq!(
        rebase(&scenario, onto("main"))?,
        Outcome::UpToDate { name: "topic".into() },
        "Git says `Current branch topic is up to date.`"
    );
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn up_to_date_branch_is_checked_out() -> crate::Result {
    let scenario = scenario("up-to-date-switch")?;
    let options = Options {
        branch: Some("topic".into()),
        ..onto("main")
    };
    assert_eq!(
        rebase(&scenario, options)?,
        Outcome::UpToDate { name: "topic".into() },
        "Git says `Current branch topic is up to date.`"
    );
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn branch_behind_its_upstream_moves_to_it() -> crate::Result {
    let scenario = scenario("fast-forward")?;
    assert_finished(&rebase(&scenario, onto("main"))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn keep_base_with_forced_replay() -> crate::Result {
    let scenario = scenario("keep-base")?;
    let options = Options {
        keep_base: true,
        force_rebase: true,
        ..onto("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn root_onto_new_root_commit() -> crate::Result {
    let scenario = scenario("root")?;
    let options = Options {
        root: true,
        programs: Programs::Allow,
        ..Default::default()
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn root_onto_another_commit() -> crate::Result {
    let scenario = scenario("root-onto")?;
    let options = Options {
        root: true,
        onto: Some("main".into()),
        programs: Programs::Allow,
        ..Default::default()
    };
    let outcome = rebase(&scenario, options)?;
    assert!(
        matches!(&outcome, Outcome::Finished { branch: Some(branch), .. } if branch.as_bstr() == "refs/heads/orphan"),
        "`orphan` is rebased, but the rebase ended with {outcome:?}"
    );
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn fork_point_of_the_configured_upstream() -> crate::Result {
    let scenario = scenario("fork-point")?;
    let options = Options {
        programs: Programs::Allow,
        ..Default::default()
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn changes_that_are_not_committed_are_refused() -> crate::Result {
    let scenario = scenario("dirty")?;
    let (refusal, messages) = refusal(&scenario, rebase(&scenario, onto("main")))?;
    assert_eq!(
        refusal,
        Error::DirtyWorktree {
            unstaged: true,
            uncommitted: false
        }
    );
    assert_eq!(
        refusal.to_string().lines().collect::<Vec<_>>(),
        messages,
        "the messages of Git"
    );
    assert_same_as_git(&scenario)
}

#[cfg(feature = "hooks")]
#[test]
#[parallel]
fn failing_pre_rebase_hook_refuses() -> crate::Result {
    let scenario = scenario("pre-rebase-refuses")?;
    let (refusal, messages) = refusal(&scenario, rebase(&scenario, onto("main")))?;
    assert_eq!(refusal, Error::PreRebaseHookRefused);
    assert_eq!(
        refusal.to_string().lines().collect::<Vec<_>>(),
        messages,
        "the messages of Git"
    );
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn branch_used_by_another_worktree_is_refused() -> crate::Result {
    let scenario = scenario("used-by-worktree")?;
    let options = Options {
        branch: Some("topic".into()),
        ..onto("main")
    };
    let (refusal, messages) = refusal(&scenario, rebase(&scenario, options))?;
    let Error::UsedByWorktree { name, path } = &refusal else {
        panic!("another worktree uses the branch, not {refusal:?}");
    };
    assert_eq!(name, "topic");
    assert_eq!(
        path.file_name(),
        Some(std::ffi::OsStr::new("other")),
        "the linked worktree in the main one"
    );
    let without_path = |message: &str| message.split_once(" at '").map(|(start, _)| start.to_owned());
    assert_eq!(
        without_path(&refusal.to_string()),
        messages.first().and_then(|message| without_path(message)),
        "the message of Git, which names the worktree of its own copy of the repository"
    );
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn exec_needs_programs() -> crate::Result {
    let scenario = scenario("exec")?;
    let options = Options {
        exec: vec!["echo ran >>.git/exec-records".into()],
        programs: Programs::Deny,
        ..interactive("main")
    };
    let err = rebase(&scenario, options).expect_err("the commands of `exec` can't run without programs");
    assert_eq!(err.downcast_any_ref::<Error>(), Some(&Error::ProgramsDenied));
    assert_eq!(rebase_state(scenario.path())?, BTreeMap::new(), "no rebase started");
    Ok(())
}

#[test]
#[parallel]
fn exec_needs_a_fully_trusted_git_directory() -> crate::Result {
    let scenario = scenario("exec")?;
    let options = Options {
        exec: vec!["echo ran >>.git/exec-records".into()],
        ..onto("main")
    };
    let err = untrusted(&scenario)?
        .rebase(options, &AtomicBool::new(false))
        .expect_err("the commands of `exec` don't run where another owner could have written the todo list");
    assert_eq!(err.downcast_any_ref::<Error>(), Some(&Error::ProgramsUntrusted));
    assert!(err.is_permission_denied(), "{err:?}");
    assert_eq!(rebase_state(scenario.path())?, BTreeMap::new(), "no rebase started");
    Ok(())
}

#[test]
#[parallel]
fn exec_lines_of_a_rebase_in_progress_need_a_fully_trusted_git_directory() -> crate::Result {
    let scenario = scenario("break")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert_eq!(stop.reason, StopReason::Break, "the rebase stops for `break`");
    let todo = scenario
        .path()
        .join(".git")
        .join("rebase-merge")
        .join("git-rebase-todo");
    let mut list = b"exec echo ran >>.git/exec-records\n".to_vec();
    list.extend(std::fs::read(&todo)?);
    std::fs::write(&todo, list)?;

    let err = untrusted(&scenario)?
        .rebase_continue(step(), &AtomicBool::new(false))
        .expect_err("the command of an `exec` line that another owner could have written doesn't run");
    assert_eq!(err.downcast_any_ref::<Error>(), Some(&Error::ProgramsUntrusted));
    assert!(
        !scenario.path().join(".git").join("exec-records").exists(),
        "the command didn't run"
    );
    assert!(
        std::fs::read(&todo)?.starts_with(b"exec echo ran"),
        "the `exec` line stays in the todo list"
    );
    Ok(())
}

/// Hooks don't run in such a repository, and only a rebase without them gets to the commits that `gpg_sign_opt` of its
/// state asks to sign. Signing with a program that doesn't exist would fail.
#[cfg(not(feature = "hooks"))]
#[test]
#[parallel]
fn a_rebase_in_progress_asks_for_no_signatures_without_trust_in_the_git_directory() -> crate::Result {
    let scenario = scenario("break")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert_eq!(stop.reason, StopReason::Break, "the rebase stops for `break`");
    std::fs::write(
        scenario.path().join(".git").join("rebase-merge").join("gpg_sign_opt"),
        "-S",
    )?;
    let repo = super::commit::baseline::open_with(scenario.path(), ["gpg.program=missing-signing-program"])?;
    let repo = gix::open_opts(
        scenario.path(),
        repo.open_options().clone().with(gix_sec::Trust::Reduced),
    )?;
    let before = repo.head_id()?;
    assert_finished(&repo.rebase_continue(step(), &AtomicBool::new(false))?);
    assert_ne!(repo.head_id()?, before, "commits were made, without a signature");
    Ok(())
}

#[test]
#[parallel]
fn interactive_without_programs_uses_the_todo_list_as_made() -> crate::Result {
    let scenario = scenario("pick")?;
    let options = Options {
        programs: Programs::Deny,
        ..interactive("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    assert_same_as_git(&scenario)
}

/// Rebase the scenario `name` onto `main` until it stops at the conflict of `one` with `upstream`, as Git stopped.
fn rebase_to_conflict(name: &str) -> crate::Result<(Scenario, Stop)> {
    let scenario = scenario(name)?;
    let stop = stopped(rebase(&scenario, onto("main"))?);
    assert!(
        matches!(stop.reason, StopReason::Conflicts { commit: Some(_) }),
        "the rebase stops at the conflict, not for {:?}",
        stop.reason
    );
    Ok((scenario, stop))
}

#[test]
#[parallel]
fn conflict_stops() -> crate::Result {
    let (scenario, stop) = rebase_to_conflict("conflict")?;
    if git_writes_todo_lists_as_gix() {
        assert_printed_by_git(&scenario, stop.message.as_bstr())?;
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn continue_after_resolved_conflict() -> crate::Result {
    let (scenario, _) = rebase_to_conflict("conflict-continue")?;
    let err = scenario
        .repo
        .rebase_continue(step(), &AtomicBool::new(false))
        .expect_err("the conflict isn't resolved yet");
    assert_eq!(
        err.downcast_any_ref::<Error>(),
        Some(&Error::UnresolvedConflicts),
        "Git refuses to continue with unmerged paths"
    );
    // The index is resolved without changing the modification time of its file, as a file system whose timestamps are
    // coarser than the time between the two writes leaves it, so the rebase only sees the resolution if it reads the
    // index again.
    let index_path = scenario.repo.index_path();
    let modified = std::fs::metadata(&index_path)?.modified()?;
    resolve(&scenario.repo, "a", "resolved\n")?;
    std::fs::File::options()
        .write(true)
        .open(&index_path)?
        .set_modified(modified)?;
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn skip_after_conflict() -> crate::Result {
    let (scenario, _) = rebase_to_conflict("conflict-skip")?;
    assert_finished(&scenario.repo.rebase_skip(step(), &AtomicBool::new(false))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn abort_after_conflict() -> crate::Result {
    let (scenario, _) = rebase_to_conflict("conflict-abort")?;
    scenario.repo.rebase_abort(step(), &AtomicBool::new(false))?;
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn quit_after_conflict() -> crate::Result {
    let (scenario, _) = rebase_to_conflict("conflict-quit")?;
    scenario.repo.rebase_quit()?;
    assert_same_as_git(&scenario)?;
    let err = scenario
        .repo
        .rebase_continue(step(), &AtomicBool::new(false))
        .expect_err("no rebase is in progress after `--quit`");
    assert_eq!(err.downcast_any_ref::<Error>(), Some(&Error::NotInProgress));
    Ok(())
}

#[test]
#[parallel]
fn reword() -> crate::Result {
    let scenario = scenario("reword")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
fn the_editor_is_a_command_line_even_if_a_file_has_its_name() -> crate::Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let scenario = scenario("reword-editor-file")?;
    {
        let _cwd = gix_testtools::set_current_dir(scenario.path())?;
        assert_finished(&rebase(&scenario, interactive("main"))?);
    }
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn squash_and_fixup() -> crate::Result {
    let scenario = scenario("squash")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn fixup_with_the_message_of_its_commit() -> crate::Result {
    let scenario = scenario("fixup-message")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[cfg(feature = "hooks")]
#[test]
#[parallel]
fn reword_stops_where_the_commit_msg_hook_fails() -> crate::Result {
    let mut scenario = scenario("reword-hook-fails")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert!(
        matches!(stop.reason, StopReason::Uncommitted { .. }),
        "the rebase stops as the amended commit wasn't made, not for {:?}",
        stop.reason
    );
    assert_printed_by_git(&scenario, stop.message.as_bstr())?;
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git_at_stop(&mut scenario)?;
    }
    std::fs::remove_file(scenario.path().join(".git").join("fail-commit-msg"))?;
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn conflict_in_fixup_chain_is_amended_when_continued() -> crate::Result {
    let mut scenario = scenario("fixup-conflict")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert!(
        matches!(stop.reason, StopReason::Conflicts { commit: Some(_) }),
        "the rebase stops at the conflict of the fixup, not for {:?}",
        stop.reason
    );
    if git_stops_fixup_chains_to_amend() {
        assert_printed_by_git(&scenario, stop.message.as_bstr())?;
        assert_same_as_git_at_stop(&mut scenario)?;
    }
    resolve(&scenario.repo, "a", "resolved\n")?;
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn conflict_in_fixup_chain_is_skipped() -> crate::Result {
    let scenario = scenario("fixup-conflict-skip")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert!(
        matches!(stop.reason, StopReason::Conflicts { commit: Some(_) }),
        "the rebase stops at the conflict of the fixup, not for {:?}",
        stop.reason
    );
    assert_finished(&scenario.repo.rebase_skip(step(), &AtomicBool::new(false))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn edit_stops_and_continues() -> crate::Result {
    let mut scenario = scenario("edit")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert!(
        matches!(stop.reason, StopReason::Edit { .. }),
        "the rebase stops for `edit`, not for {:?}",
        stop.reason
    );
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git_at_stop(&mut scenario)?;
    }
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn break_stops_and_continues() -> crate::Result {
    let mut scenario = scenario("break")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert_eq!(stop.reason, StopReason::Break, "the rebase stops for `break`");
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git_at_stop(&mut scenario)?;
    }
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn drop_and_removed_lines() -> crate::Result {
    let scenario = scenario("drop")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn missing_commits_stop_with_check_set_to_error() -> crate::Result {
    let scenario = scenario("missing-commits")?;
    let stop = stopped(rebase(&scenario, interactive("main"))?);
    assert_eq!(stop.reason, StopReason::InvalidTodoList, "the todo list drops a commit");
    if git_writes_todo_lists_as_gix() {
        assert_printed_by_git(&scenario, stop.message.as_bstr())?;
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn instruction_format_and_abbreviated_commands() -> crate::Result {
    let scenario = scenario("instruction-format")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn exec() -> crate::Result {
    let scenario = scenario("exec")?;
    let options = Options {
        exec: vec!["echo ran >>.git/exec-records".into()],
        ..interactive("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn failing_exec_is_rescheduled() -> crate::Result {
    let mut scenario = scenario("exec-fails")?;
    let options = Options {
        exec: vec!["test -f .git/exec-ok || { touch .git/exec-ok; exit 1; }".into()],
        ..onto("main")
    };
    let stop = stopped(rebase(&scenario, options)?);
    assert!(
        matches!(
            stop.reason,
            StopReason::Exec {
                success: false,
                rescheduled: true,
                ..
            }
        ),
        "the rebase stops at the failed command, which runs again, not for {:?}",
        stop.reason
    );
    assert_printed_by_git(&scenario, stop.message.as_bstr())?;
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git_at_stop(&mut scenario)?;
    }
    assert_finished(&scenario.repo.rebase_continue(step(), &AtomicBool::new(false))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn autosquash_as_configured() -> crate::Result {
    let scenario = scenario("autosquash")?;
    assert_finished(&rebase(&scenario, interactive("main"))?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}

#[test]
#[parallel]
fn update_refs_as_configured() -> crate::Result {
    let scenario = scenario("update-refs")?;
    assert_finished(&rebase(&scenario, onto("main"))?);
    assert_same_as_git(&scenario)
}

#[test]
#[parallel]
fn rebase_merges() -> crate::Result {
    let scenario = scenario("rebase-merges")?;
    let options = Options {
        rebase_merges: Merges::Rebase(RebaseMerges::NoRebaseCousins),
        ..interactive("main")
    };
    assert_finished(&rebase(&scenario, options)?);
    if git_writes_todo_lists_as_gix() {
        assert_same_as_git(&scenario)?;
    }
    Ok(())
}
