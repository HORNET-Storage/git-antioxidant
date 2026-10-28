use bstr::ByteSlice;
use gix_hook::{Config, Context, Hook, Hooks, Skipped};
use gix_testtools::Result;

use crate::{assert_listed_as_git, configured, fixture, hooks_of, hooks_with, read_text, scenario};

#[test]
fn hooks_are_found_where_git_found_them() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let repo = root.join("repo");
    let common_dir = repo.join(".git");
    let hooks = hooks_of(&repo)?;
    assert_eq!(
        hooks.hooks_dir(),
        common_dir.join("hooks"),
        "the hooks directory is that of the common Git directory"
    );
    assert_eq!(hooks.working_dir(), repo, "hooks run in the root of the worktree");

    let linked = root.join("linked");
    let git_dir = common_dir.join("worktrees").join("linked");
    let linked_hooks = Hooks::new(
        Context {
            git_dir: &git_dir,
            common_dir: &common_dir,
            worktree: Some(&linked),
        },
        Config::default(),
    )?;
    assert_eq!(
        linked_hooks.hooks_dir(),
        hooks.hooks_dir(),
        "linked worktrees share the hooks of the main worktree"
    );
    assert_eq!(
        linked_hooks.working_dir(),
        linked,
        "hooks run in the root of the linked worktree"
    );

    let bare = root.join("remote.git");
    let bare_hooks = Hooks::new(
        Context {
            git_dir: &bare,
            common_dir: &bare,
            worktree: None,
        },
        Config::default(),
    )?;
    assert_eq!(
        bare_hooks.hooks_dir(),
        bare.join("hooks"),
        "bare repositories have hooks too"
    );
    assert_eq!(
        bare_hooks.working_dir(),
        bare,
        "hooks of a bare repository run in its Git directory"
    );

    for (hooks, event) in [
        (&hooks, "pre-commit"),
        (&linked_hooks, "pre-commit"),
        (&bare_hooks, "reference-transaction"),
    ] {
        assert_eq!(
            hooks.find(event).hooks,
            [Hook::Program {
                path: hooks.hooks_dir().join(event)
            }],
            "the program named after {event} in the hooks directory is found"
        );
    }
    assert_eq!(
        hooks.find("pre-auto-gc"),
        Default::default(),
        "an event without hooks has none"
    );
    Ok(())
}

#[test]
fn relative_hooks_path_is_taken_from_the_root_of_the_worktree() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let repo = root.join("hooks-path");
    let hooks = hooks_with(
        &repo,
        Config {
            hooks_path: Some("custom-hooks".into()),
            ..Config::default()
        },
    )?;
    assert_eq!(
        hooks.hooks_dir(),
        repo.join("custom-hooks"),
        "Git ran the hook in `custom-hooks`, although it was started in a subdirectory"
    );
    assert_eq!(hooks.find("pre-commit").hooks.len(), 1, "the hook there is found");
    Ok(())
}

#[test]
fn optional_hooks_path_that_is_missing_is_ignored() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let repo = root.join("optional-hooks-path");
    let hooks = hooks_with(
        &repo,
        Config {
            hooks_path: Some(":(optional)missing-hooks".into()),
            ..Config::default()
        },
    )?;
    assert_eq!(
        hooks.hooks_dir(),
        repo.join(".git").join("hooks"),
        "Git ran the hook of the hooks directory of the repository"
    );
    Ok(())
}

#[test]
fn configured_hooks_are_listed_as_git_lists_them() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    for (repo, events) in [
        ("configured", &["pre-commit", "post-commit"][..]),
        ("config-rules", &["pre-commit", "post-commit", "pre-push"]),
    ] {
        let dir = scenario(&root, repo);
        let hooks = hooks_with(
            &root.join(repo),
            Config {
                configured: configured(&dir.join("config"))?,
                ..Config::default()
            },
        )?;
        for event in events {
            let listing = read_text(&dir.join(format!("list-{event}")))?;
            assert_listed_as_git(&hooks.find(event), &listing, event);
        }
    }
    Ok(())
}

#[test]
fn programs_that_are_not_executable_are_skipped_as_git_skips_them() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let hooks = hooks_of(&root.join("not-executable"))?;
    let found = hooks.find("pre-commit");
    let git_ignored_it = std::fs::read(scenario(&root, "not-executable").join("err"))?
        .contains_str("hook was ignored because it's not set as executable");
    assert_eq!(
        git_ignored_it,
        cfg!(unix),
        "only on Unix are files without permission to execute them ignored"
    );
    let path = hooks.hooks_dir().join("pre-commit");
    if git_ignored_it {
        assert_eq!(
            found.skipped,
            [Skipped::NotExecutable { path }],
            "Git ignored the hook, as it isn't executable"
        );
        assert!(found.hooks.is_empty(), "the hook doesn't run");
    } else {
        assert_eq!(found.hooks, [Hook::Program { path }], "Git for Windows ran the hook");
        assert!(found.skipped.is_empty(), "nothing is skipped");
    }
    Ok(())
}
