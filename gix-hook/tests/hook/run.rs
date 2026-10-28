//! Each test runs the hooks of a scenario through `gix-hook` in a process of its own, as their records depend on the
//! environment, and compares their records with those Git made.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use bstr::ByteSlice;
use gix_hook::{
    Config, Context, Hooks, Invocation,
    invocation::{
        Checkout, Commit, Editor, MessageSource, PushUpdate, RefUpdate, RefValue, Rewrite, Stdin, TransactionState,
    },
    run::{Outcome, Status},
};
use gix_testtools::{Env, Result, tempfile::TempDir};

use crate::{configured, env_path, fixture, hooks_of, hooks_with, name_of, oid, read_text, records, scenario};

/// A directory for the records of the hooks that `gix-hook` runs, which the environment of this process names to them.
struct Recording {
    dir: TempDir,
    _env: Env<'static>,
}

impl Recording {
    /// Name a new directory for records to hooks, and `root` as the base of the paths they record, in addition to the
    /// variables of `env`. This changes the environment of the process, which must be isolated.
    fn new(root: &Path, env: Env<'static>) -> Result<Self> {
        let dir = TempDir::new()?;
        let env = env
            .set("HOOK_BASE", env_path(root)?)
            .set("HOOK_RECORDS", env_path(dir.path())?);
        Ok(Recording { dir, _env: env })
    }

    /// Assert that the hooks recorded what the hooks Git ran in `name` recorded.
    fn assert_same_as_git(&self, root: &Path, name: &str) -> Result {
        assert_eq!(
            records(self.dir.path())?,
            records(&scenario(root, name))?,
            "the hooks of {name} are started as Git started them"
        );
        Ok(())
    }
}

/// The commit of the main worktree with a message from the command line, as `git commit -m` and `git merge -m` make it.
fn commit_without_editor() -> Commit<'static> {
    Commit {
        index_file: Path::new(".git/index"),
        editor: Editor::NotUsed,
    }
}

/// Assert that each outcome is that of one hook that succeeded.
fn assert_each_succeeded(outcomes: impl IntoIterator<Item = Outcome>) {
    for outcome in outcomes {
        assert_eq!(outcome.hooks.len(), 1, "one hook ran: {outcome:?}");
        assert!(outcome.success(), "the hook succeeded: {outcome:?}");
    }
}

/// Return the names of the hooks of `outcome` with their codes.
fn codes(outcome: &Outcome) -> Vec<(String, i32)> {
    outcome
        .hooks
        .iter()
        .map(|ran| (name_of(&ran.hook), ran.status.code()))
        .collect()
}

/// Run the current test again in a subprocess, isolated from the Git environment of this one, and return its output,
/// or `None` in the subprocess.
fn output_of_isolated_process() -> Result<Option<std::process::Output>> {
    const MARKER: &str = "GIX_HOOK_TEST_IN_SUBPROCESS";
    let thread = std::thread::current();
    let name = thread.name().ok_or("tests run in threads named after them")?;
    if std::env::var_os(MARKER).is_some_and(|value| value == name) {
        return Ok(None);
    }
    let config_dir = TempDir::new()?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    gix_testtools::configure_git_environment(&mut cmd, config_dir.path());
    let output = cmd
        .args(["--exact", name, "--nocapture", "--test-threads=1", "--include-ignored"])
        .env(MARKER, name)
        .stdin(std::process::Stdio::null())
        .output()?;
    Ok(Some(output))
}

#[test]
fn commit() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let commit = commit_without_editor();
    let message_file = Path::new(".git/COMMIT_EDITMSG");
    assert_each_succeeded([
        hooks.pre_commit(commit)?,
        hooks.prepare_commit_msg(commit, message_file, Some(MessageSource::Message))?,
        hooks.commit_msg(commit, message_file)?,
        hooks.post_commit(commit)?,
    ]);
    recording.assert_same_as_git(&root, "commit")
}

#[test]
fn amend() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let (old, new) = (oid(&root, "amend-old")?, oid(&root, "amend-new")?);
    assert_each_succeeded([
        hooks.prepare_commit_msg(
            commit_without_editor(),
            Path::new(".git/COMMIT_EDITMSG"),
            Some(MessageSource::Commit(OsStr::new("HEAD"))),
        )?,
        hooks.post_rewrite(Rewrite::Amend, [(&*old, &*new)])?,
    ]);
    recording.assert_same_as_git(&root, "amend")
}

#[test]
fn merge() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let commit = commit_without_editor();
    let message_file = Path::new(".git/MERGE_MSG");
    assert_each_succeeded([
        hooks.pre_merge_commit(commit)?,
        hooks.prepare_commit_msg(commit, message_file, Some(MessageSource::Merge))?,
        hooks.commit_msg(commit, message_file)?,
        hooks.post_merge(false)?,
    ]);
    recording.assert_same_as_git(&root, "merge")
}

#[test]
fn squash_merge() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    assert_each_succeeded([hooks.post_merge(true)?]);
    recording.assert_same_as_git(&root, "squash")
}

#[test]
fn checkout() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let (main, topic) = (oid(&root, "main")?, oid(&root, "topic")?);
    assert_each_succeeded([
        hooks.post_checkout(&main, &topic, Checkout::Branch)?,
        hooks.post_checkout(&topic, &main, Checkout::Branch)?,
        hooks.post_checkout(&main, &main, Checkout::Files)?,
    ]);
    recording.assert_same_as_git(&root, "checkout")
}

#[test]
fn rebase() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let (old_1, old_2) = (oid(&root, "rebase-old-1")?, oid(&root, "rebase-old-2")?);
    let (new_1, new_2) = (oid(&root, "rebase-new-1")?, oid(&root, "rebase-new-2")?);
    assert_each_succeeded([
        hooks.pre_rebase(OsStr::new("main"), None)?,
        hooks.post_checkout(&old_2, &oid(&root, "rebase-onto")?, Checkout::Branch)?,
        hooks.post_rewrite(Rewrite::Rebase, [(&*old_1, &*new_1), (&*old_2, &*new_2)])?,
    ]);
    recording.assert_same_as_git(&root, "rebase")
}

#[test]
fn pre_rebase_with_branch() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let branch = Some(OsStr::new("rebased"));
    assert_each_succeeded([
        hooks.pre_rebase(OsStr::new("main"), branch)?,
        hooks.pre_rebase(OsStr::new("--root"), branch)?,
    ]);
    recording.assert_same_as_git(&root, "pre-rebase")
}

#[test]
fn push() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let (main, old_main) = (oid(&root, "push-main")?, oid(&root, "push-old-main")?);
    let old_topic = oid(&root, "push-old-topic")?;
    let null = main.kind().null();
    let (remote, url) = (OsStr::new("origin"), OsStr::new("../remote.git"));
    assert_each_succeeded([
        hooks.pre_push(
            remote,
            url,
            [PushUpdate {
                local_ref: "refs/heads/main".into(),
                local_object: &main,
                remote_ref: "refs/heads/main".into(),
                remote_object: &old_main,
            }],
        )?,
        hooks.pre_push(
            remote,
            url,
            [PushUpdate {
                local_ref: "(delete)".into(),
                local_object: &null,
                remote_ref: "refs/heads/topic".into(),
                remote_object: &old_topic,
            }],
        )?,
    ]);
    recording.assert_same_as_git(&root, "push")
}

#[test]
fn reference_transaction() -> Result {
    use TransactionState::{Aborted, Committed, Prepared, Preparing};

    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    let (main, topic) = (oid(&root, "main")?, oid(&root, "topic")?);
    let null = main.kind().null();
    let update = |old, new, name: &'static str| RefUpdate {
        old,
        new,
        name: name.into(),
    };
    let committed = &[Preparing, Prepared, Committed][..];
    let transactions = [
        (
            update(RefValue::Object(&null), RefValue::Object(&main), "refs/heads/created"),
            committed,
        ),
        (
            update(RefValue::Object(&main), RefValue::Object(&topic), "refs/heads/created"),
            committed,
        ),
        (
            update(
                RefValue::Object(&null),
                RefValue::Symbolic("refs/heads/main".into()),
                "refs/heads/symbolic",
            ),
            committed,
        ),
        (
            update(RefValue::Object(&null), RefValue::Object(&main), "refs/heads/aborted"),
            &[Preparing, Prepared, Aborted][..],
        ),
        // The `files` backend aborts the transaction of `packed-refs` it doesn't need for the deletion.
        (
            update(RefValue::Object(&null), RefValue::Object(&null), "refs/heads/created"),
            &[Preparing, Aborted, Prepared, Committed][..],
        ),
    ];
    for (update, states) in transactions {
        for &state in states {
            assert_each_succeeded([hooks.reference_transaction(state, [update])?]);
        }
    }
    recording.assert_same_as_git(&root, "reference-transaction")
}

#[test]
fn bare_repository() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let bare = root.join("remote.git");
    let hooks = Hooks::new(
        Context {
            git_dir: &bare,
            common_dir: &bare,
            worktree: None,
        },
        Config::default(),
    )?;
    let new = oid(&root, "bare")?;
    let null = new.kind().null();
    let update = RefUpdate {
        old: RefValue::Object(&null),
        new: RefValue::Object(&new),
        name: "refs/heads/bare".into(),
    };
    for state in [
        TransactionState::Preparing,
        TransactionState::Prepared,
        TransactionState::Committed,
    ] {
        assert_each_succeeded([hooks.reference_transaction(state, [update])?]);
    }
    recording.assert_same_as_git(&root, "bare")
}

#[test]
fn new_linked_worktree() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("repo"))?;
    assert_each_succeeded([hooks.post_checkout_in_new_worktree(&oid(&root, "linked")?, &root.join("linked"))?]);
    recording.assert_same_as_git(&root, "worktree-add")
}

#[test]
fn linked_worktree() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let common_dir = root.join("repo").join(".git");
    let git_dir = common_dir.join("worktrees").join("linked");
    let worktree = root.join("linked");
    let hooks = Hooks::new(
        Context {
            git_dir: &git_dir,
            common_dir: &common_dir,
            worktree: Some(&worktree),
        },
        Config::default(),
    )?;
    let exported_git_dir = hooks
        .environment()
        .iter()
        .find(|(name, _)| name == "GIT_DIR")
        .and_then(|(_, value)| value.as_ref())
        .ok_or("Git exports GIT_DIR for linked worktrees")?;
    // Git names the index of the worktree after the Git directory it exports.
    let index_file = PathBuf::from(format!(
        "{}/index",
        exported_git_dir.to_str().ok_or("the paths of the fixture are UTF-8")?
    ));
    assert_each_succeeded([hooks.pre_commit(Commit {
        index_file: &index_file,
        editor: Editor::NotUsed,
    })?]);
    recording.assert_same_as_git(&root, "linked")
}

#[test]
fn clone() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let cloned = root.join("cloned");
    let hooks = hooks_of(&cloned)?;
    let git_dir = PathBuf::from(env_path(&gix_path::realpath(cloned.join(".git"))?)?);
    assert_each_succeeded([hooks.post_checkout_after_clone(&oid(&root, "clone-head")?, &git_dir)?]);
    recording.assert_same_as_git(&root, "clone")
}

#[test]
fn hooks_path_with_git_started_in_a_subdirectory() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_with(
        &root.join("hooks-path"),
        Config {
            hooks_path: Some("custom-hooks".into()),
            ..Config::default()
        },
    )?;
    let mut invocation = Invocation::pre_commit(commit_without_editor());
    invocation.env.push(("GIT_PREFIX".into(), Some("sub/".into())));
    assert_each_succeeded([hooks.run(&invocation)?]);
    recording.assert_same_as_git(&root, "hooks-path")
}

#[test]
fn optional_hooks_path() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_with(
        &root.join("optional-hooks-path"),
        Config {
            hooks_path: Some(":(optional)missing-hooks".into()),
            ..Config::default()
        },
    )?;
    assert_each_succeeded([hooks.pre_commit(commit_without_editor())?]);
    recording.assert_same_as_git(&root, "optional-hooks-path")
}

#[test]
fn configured_hooks() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let path = std::env::join_paths(
        std::iter::once(root.join("bin")).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )?;
    let recording = Recording::new(
        &root,
        Env::new()
            .set("HOOK_RECORDER", env_path(&root.join("record-hook"))?)
            .set("PATH", path.into_string().map_err(|_| "PATH is UTF-8")?),
    )?;
    let hooks = hooks_with(
        &root.join("configured"),
        Config {
            configured: configured(&scenario(&root, "configured").join("config"))?,
            ..Config::default()
        },
    )?;
    let commit = commit_without_editor();
    assert_eq!(
        codes(&hooks.pre_commit(commit)?),
        [("one".into(), 0), ("path".into(), 0), ("pre-commit".into(), 0)],
        "the configured hooks that are enabled run first, in the order of the configuration"
    );
    assert_each_succeeded([hooks.post_commit(commit)?]);
    recording.assert_same_as_git(&root, "configured")
}

#[test]
fn exit_codes_and_failures_to_start() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(
        &root,
        Env::new().set("HOOK_RECORDER", env_path(&root.join("record-hook"))?),
    )?;
    let repo = root.join("exit-codes");
    let hooks = hooks_with(
        &repo,
        Config {
            configured: configured(&scenario(&root, "exit-codes").join("config"))?,
            ..Config::default()
        },
    )?;
    let input = repo.join("input-of-hooks");
    let outcome = hooks.run(&Invocation {
        args: vec!["first argument".into(), "second".into()],
        stdin: Stdin::File(&input),
        ..Invocation::new("pre-commit")
    })?;
    assert_eq!(
        codes(&outcome),
        [("first".into(), 1), ("second".into(), 2), ("missing".into(), 1)],
        "hooks that fail don't stop the others, but one that can't be started stops those after it"
    );
    assert!(
        matches!(outcome.hooks.last().map(|ran| &ran.status), Some(Status::NotStarted(_))),
        "the missing program of the last hook can't be started"
    );
    assert_eq!(
        outcome.code().to_string(),
        read_text(&scenario(&root, "exit-codes").join("status"))?,
        "the codes of the hooks are combined as Git combines them"
    );
    recording.assert_same_as_git(&root, "exit-codes")
}

#[test]
fn hook_without_shebang() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("no-shebang"))?;
    let outcome = hooks.run(&Invocation::new("pre-commit"))?;
    assert_eq!(
        outcome.code().to_string(),
        read_text(&scenario(&root, "no-shebang").join("status"))?,
        "the hook runs with the shell on Unix, and can't be started on Windows, as with Git"
    );
    recording.assert_same_as_git(&root, "no-shebang")
}

#[test]
fn hook_that_is_not_executable() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let recording = Recording::new(&root, Env::new())?;
    let hooks = hooks_of(&root.join("not-executable"))?;
    let outcome = hooks.run(&Invocation::new("pre-commit"))?;
    assert!(
        outcome.success(),
        "the hook doesn't run on Unix, and succeeds on Windows"
    );
    recording.assert_same_as_git(&root, "not-executable")
}

#[test]
fn standard_output_goes_where_git_sends_it() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let Some(output) = output_of_isolated_process()? else {
        let _recording = Recording::new(&root, Env::new())?;
        let hooks = hooks_of(&root.join("repo"))?;
        assert_each_succeeded([
            hooks.pre_commit(commit_without_editor())?,
            hooks.pre_push(OsStr::new("origin"), OsStr::new("../remote.git"), [])?,
        ]);
        return Ok(());
    };
    assert!(output.status.success(), "the hooks ran in the subprocess: {output:?}");
    for (hook, name) in [("pre-commit", "commit"), ("pre-push", "push")] {
        let line = format!("stdout of {hook}");
        let dir = scenario(&root, name);
        let git_sent_it_to_stdout = std::fs::read(dir.join("out"))?.contains_str(&line);
        let git_sent_it_to_stderr = std::fs::read(dir.join("err"))?.contains_str(&line);
        assert_ne!(
            git_sent_it_to_stdout, git_sent_it_to_stderr,
            "Git sent the standard output of {hook} to one of its own"
        );
        assert_eq!(
            output.stdout.contains_str(&line),
            git_sent_it_to_stdout,
            "the standard output of {hook} goes to standard output if Git sent it there"
        );
        assert_eq!(
            output.stderr.contains_str(&line),
            git_sent_it_to_stderr,
            "the standard output of {hook} goes to standard error if Git sent it there"
        );
    }
    Ok(())
}
