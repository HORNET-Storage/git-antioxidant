use gix::hook::{Found, Hook, Skipped};
use gix_testtools::TestResult;

fn repo(name: &str, options: gix::open::Options) -> TestResult<gix::Repository> {
    Ok(crate::named_subrepo_opts("make_hooks_repos.sh", name, options)?)
}

#[test]
fn the_hooks_directory_is_that_of_the_common_git_directory() -> TestResult {
    let repo = repo("default", crate::restricted())?;
    let hooks = repo.hooks()?;
    assert_eq!(hooks.hooks_dir(), std::path::absolute(repo.common_dir().join("hooks"))?);
    assert_eq!(
        hooks.working_dir(),
        std::path::absolute(repo.workdir().expect("the repository has a worktree"))?,
        "hooks run in the root of the worktree"
    );
    assert_eq!(
        hooks.find("pre-commit"),
        Found::default(),
        "the samples of `git init` aren't hooks"
    );
    Ok(())
}

#[test]
fn bare_repositories_run_hooks_in_their_git_directory() -> TestResult {
    let repo = repo("bare.git", crate::restricted())?;
    let hooks = repo.hooks()?;
    let git_dir = std::path::absolute(repo.git_dir())?;
    assert_eq!(hooks.hooks_dir(), git_dir.join("hooks"));
    assert_eq!(hooks.working_dir(), git_dir);
    Ok(())
}

#[test]
fn a_relative_hooks_path_is_relative_to_the_worktree() -> TestResult {
    let repo = repo("hooks-path", crate::restricted())?;
    let hooks = repo.hooks()?;
    assert_eq!(
        hooks.hooks_dir(),
        std::path::absolute(repo.workdir().expect("the repository has a worktree"))?.join("custom-hooks"),
        "`core.hooksPath` replaces the `hooks` directory"
    );
    Ok(())
}

#[test]
fn configured_hooks_come_from_the_hook_sections() -> TestResult {
    let repo = repo("configured", crate::restricted())?;
    assert_eq!(
        repo.hooks()?.find("pre-commit"),
        Found {
            hooks: vec![Hook::Command {
                name: "linter".into(),
                command: "echo lint".into(),
            }],
            skipped: vec![Skipped::Disabled { name: "off".into() }],
        }
    );
    Ok(())
}

#[test]
fn invalid_hook_configuration_is_an_error() -> TestResult {
    let repo = repo("invalid", crate::restricted())?;
    let err = repo
        .hooks()
        .expect_err("a hook named like an event can't be added to an event, as with Git");
    assert!(err.is_validation(), "{err:?}");
    Ok(())
}

#[test]
fn hooks_need_a_fully_trusted_git_directory() -> TestResult {
    let repo = repo("configured", crate::restricted().with(gix_sec::Trust::Reduced))?;
    let err = repo
        .hooks()
        .expect_err("hooks start programs, which reduced trust doesn't allow");
    assert!(err.is_permission_denied(), "{err:?}");
    Ok(())
}
