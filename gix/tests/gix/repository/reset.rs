//! Resetting with `gix`, compared with what `git reset` does.

use std::sync::atomic::AtomicBool;

use gix::reset::{Error, Mode, Options, Outcome, Refusals};

use super::commit::baseline::Scenario;

/// The files of the Git directory that a reset writes or removes, which `gix` has to leave as Git does.
const STATE: &[&str] = &[
    "HEAD",
    "ORIG_HEAD",
    "MERGE_HEAD",
    "MERGE_MSG",
    "MERGE_MODE",
    "SQUASH_MSG",
    "AUTO_MERGE",
];

fn scenario(name: &str) -> crate::Result<Scenario> {
    Scenario::new("make_reset_repos.sh", name)
}

/// Return the commit `generations` first parents before the commit of `HEAD` in `repo`, which `HEAD~<generations>`
/// names.
fn ancestor(repo: &gix::Repository, generations: usize) -> crate::Result<gix::ObjectId> {
    let mut id = repo.head_id()?.detach();
    for _ in 0..generations {
        id = repo
            .find_commit(id)?
            .parent_ids()
            .next()
            .expect("the commit has a parent")
            .detach();
    }
    Ok(id)
}

/// Reset in the scenario `name` in `mode` to `HEAD~<generations>`, named so in the reflog, or to `HEAD` without
/// `generations`, as Git did there, and return the scenario with what `gix` did.
fn reset(
    name: &str,
    mode: Mode,
    generations: Option<usize>,
    reflog_action: Option<&str>,
) -> crate::Result<(Scenario, gix::Result<Outcome>)> {
    let scenario = scenario(name)?;
    let commit = generations
        .map(|generations| ancestor(&scenario.repo, generations))
        .transpose()?;
    let options = Options {
        revision: generations.map(|generations| format!("HEAD~{generations}").into()),
        reflog_action: reflog_action.map(Into::into),
    };
    let outcome = scenario.repo.reset(commit, mode, options, &AtomicBool::new(false));
    Ok((scenario, outcome))
}

/// Reset as `reset()` does and assert that `gix` left the repository as Git did.
fn resets_as_git(name: &str, mode: Mode, generations: Option<usize>) -> crate::Result<Outcome> {
    let (scenario, outcome) = reset(name, mode, generations, None)?;
    let outcome = outcome?;
    scenario.assert_same_as_git(STATE)?;
    Ok(outcome)
}

/// Reset as `reset()` does, assert that `gix` refused and left the repository as Git did, and return why it refused
/// along with the messages of Git.
fn refuses_as_git(name: &str, mode: Mode, generations: Option<usize>) -> crate::Result<(Error, Vec<String>)> {
    let (scenario, outcome) = reset(name, mode, generations, None)?;
    let err = outcome.expect_err("Git refused to reset as well");
    scenario.assert_same_as_git(STATE)?;
    let refusal = err
        .downcast_any_ref::<Error>()
        .cloned()
        .unwrap_or_else(|| panic!("{name}: a refusal to reset, not {err:?}"));
    Ok((refusal, scenario.git_messages()?))
}

/// Assert that `refusal` has the messages of Git.
fn assert_messages(refusal: &Error, messages: &[String]) {
    assert_eq!(
        refusal.to_string().lines().collect::<Vec<_>>(),
        messages,
        "the messages of Git"
    );
}

#[test]
fn soft() -> crate::Result {
    let outcome = resets_as_git("soft", Mode::Soft, Some(1))?;
    assert_ne!(outcome.commit_id, outcome.previous, "HEAD moved");
    Ok(())
}

#[test]
fn mixed() -> crate::Result {
    resets_as_git("mixed", Mode::Mixed, Some(1))?;
    Ok(())
}

#[test]
fn mixed_without_commit_logs_unchanged_head() -> crate::Result {
    let outcome = resets_as_git("mixed-head", Mode::Mixed, None)?;
    assert_eq!(
        outcome.commit_id, outcome.previous,
        "HEAD stays, and only its reflog has the reset, as in Git"
    );
    Ok(())
}

#[test]
fn hard() -> crate::Result {
    resets_as_git("hard", Mode::Hard, Some(2))?;
    Ok(())
}

#[test]
fn merge() -> crate::Result {
    resets_as_git("merge", Mode::Merge, Some(1))?;
    Ok(())
}

#[test]
fn merge_refuses_unstaged_changes_of_changing_files() -> crate::Result {
    let (refusal, messages) = refuses_as_git("merge-refused", Mode::Merge, Some(1))?;
    assert_eq!(
        refusal,
        Error::Refused {
            revision: "HEAD~1".into(),
            refusals: Refusals {
                unstaged_changes: vec!["dir/b".into()],
                ..Default::default()
            },
        }
    );
    assert_messages(&refusal, &messages);
    Ok(())
}

#[test]
fn keep() -> crate::Result {
    resets_as_git("keep", Mode::Keep, Some(1))?;
    Ok(())
}

#[test]
fn keep_refuses_local_changes_of_changing_files() -> crate::Result {
    let (refusal, messages) = refuses_as_git("keep-refused", Mode::Keep, Some(1))?;
    assert_eq!(
        refusal,
        Error::Refused {
            revision: "HEAD~1".into(),
            refusals: Refusals {
                unstaged_changes: vec!["dir/b".into()],
                ..Default::default()
            },
        }
    );
    assert_messages(&refusal, &messages);
    Ok(())
}

#[test]
fn soft_and_keep_are_refused_in_a_merge() -> crate::Result {
    for (name, mode) in [("soft-in-merge", Mode::Soft), ("keep-in-merge", Mode::Keep)] {
        let (refusal, messages) = refuses_as_git(name, mode, Some(1))?;
        assert_eq!(refusal, Error::InMerge { mode });
        assert_messages(&refusal, &messages);
    }
    Ok(())
}

#[test]
fn hard_reset_ends_a_merge() -> crate::Result {
    resets_as_git("hard-in-merge", Mode::Hard, None)?;
    Ok(())
}

#[test]
fn merge_reset_aborts_a_merge() -> crate::Result {
    resets_as_git("abort-merge", Mode::Merge, None)?;
    Ok(())
}

#[test]
fn paths_from_a_commit() -> crate::Result {
    let scenario = scenario("paths")?;
    let commit = ancestor(&scenario.repo, 2)?;
    scenario.repo.reset_paths(Some(commit), ["a", "dir", "d"])?;
    scenario.assert_same_as_git(STATE)?;
    Ok(())
}

#[test]
fn paths_from_head() -> crate::Result {
    let scenario = scenario("paths-head")?;
    scenario.repo.reset_paths(None, ["a"])?;
    scenario.assert_same_as_git(STATE)?;
    Ok(())
}

#[test]
fn unborn_head() -> crate::Result {
    for (name, mode) in [("unborn", Mode::Mixed), ("unborn-hard", Mode::Hard)] {
        let outcome = resets_as_git(name, mode, None)?;
        assert_eq!(outcome.commit_id, None, "{name}: HEAD stays unborn");
        assert_eq!(outcome.previous, None);
    }
    Ok(())
}

#[test]
fn keep_needs_a_commit() -> crate::Result {
    let (refusal, messages) = refuses_as_git("unborn-keep", Mode::Keep, None)?;
    assert_eq!(refusal, Error::InvalidHead);
    assert_eq!(
        messages.first().map(String::as_str),
        Some(refusal.to_string().as_str()),
        "Git adds that it couldn't reset the index"
    );
    Ok(())
}

#[test]
fn detached_head() -> crate::Result {
    resets_as_git("detached", Mode::Hard, Some(1))?;
    Ok(())
}

#[test]
fn reflog_action() -> crate::Result {
    let (scenario, outcome) = reset("reflog-action", Mode::Hard, Some(1), Some("custom"))?;
    outcome?;
    scenario.assert_same_as_git(STATE)?;
    Ok(())
}

#[test]
fn bare_repositories() -> crate::Result {
    resets_as_git("bare-soft", Mode::Soft, Some(1))?;
    let (refusal, messages) = refuses_as_git("bare-mixed", Mode::Mixed, Some(1))?;
    assert_eq!(refusal, Error::BareRepository { mode: Mode::Mixed });
    assert_messages(&refusal, &messages);
    Ok(())
}
