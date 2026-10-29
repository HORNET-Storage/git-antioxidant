//! Compare how helpers named in the configuration run with what Git did in the scenarios of
//! `make_named_helper_records.sh`.

use std::path::{Path, PathBuf};

use bstr::{BString, ByteSlice};
use gix_credentials::{
    Program,
    helper::{Action, Cascade},
    protocol::Context,
};
use gix_testtools::Result;

/// The scenarios of the fixture, each a repository with the helper Git ran, what `git credential fill` printed, and the
/// records of the helper.
const SCENARIOS: &[&str] = &["plain", "quoted-arguments", "shell-arguments", "http-path"];

/// Return the root of the fixture, whose `bin` holds `git-credential-recorder`, the program of the helper `recorder`.
fn fixture() -> Result<PathBuf> {
    Ok(std::path::absolute(gix_testtools::scripted_fixture_read_only(
        "make_named_helper_records.sh",
    )?)?)
}

/// Return `path` for a variable of the environment, with slashes on Windows, where the helper runs in Git's shell.
fn env_path(path: &Path) -> Result<String> {
    let path = path.to_str().ok_or("temporary paths are UTF-8")?;
    Ok(if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    })
}

/// Return `PATH` with `dir` in front, as the fixture put the directory of `git-credential-recorder` there.
fn path_with(dir: &Path) -> Result<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(dir.to_owned()).chain(std::env::split_paths(&path)))?;
    Ok(path.into_string().map_err(|_| "`PATH` is UTF-8")?)
}

/// Do with gix-credentials what the fixture did with `git credential` in `scenario`: obtain a credential for the URL of
/// the fixture from the helper of the scenario, then approve and reject it. Return the context of the credential, as
/// `git credential fill` prints it.
fn fill_approve_reject(scenario: &Path) -> Result<BString> {
    let helper = std::fs::read_to_string(scenario.join("helper"))?;
    let use_http_path = std::fs::read_to_string(scenario.join("use-http-path"))? == "true";
    let mut cascade = Cascade::default()
        .use_http_path(use_http_path)
        .extend([Program::from_custom_definition(helper)?]);
    let prompt = gix_prompt::Options {
        mode: gix_prompt::Mode::Disable,
        askpass: None,
    };
    let outcome = cascade
        .invoke(Action::get_for_url("https://example.com/org/repo.git"), prompt.clone())?
        .ok_or("the helper answers with a credential")?;
    let filled = Context::try_from(&outcome.next)?.to_bstring();
    assert!(cascade.invoke(outcome.next.clone().store(), prompt.clone())?.is_none());
    assert!(cascade.invoke(outcome.next.erase(), prompt)?.is_none());
    Ok(filled)
}

#[test]
fn named_helpers_receive_what_git_sends_them() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let root = fixture()?;
    let tmp = gix_testtools::tempfile::tempdir()?;
    let _env = gix_testtools::Env::new()
        .set("PATH", path_with(&root.join("bin"))?)
        .set("RECORDER_MODE", "from-environment");
    for scenario in SCENARIOS {
        let expected = root.join(scenario);
        let records = tmp.path().join(scenario);
        let _records = gix_testtools::Env::new().set("CREDENTIAL_RECORDS", env_path(&records)?);
        let filled = fill_approve_reject(&expected)?;
        assert_eq!(
            filled.as_bstr(),
            std::fs::read(expected.join("fill"))?.as_bstr(),
            "{scenario}: the credential and what it's for are as `git credential fill` printed them"
        );
        assert_eq!(
            std::fs::read(&records)?.as_bstr(),
            std::fs::read(expected.join("records"))?.as_bstr(),
            "{scenario}: the helper received the arguments and requests that Git gave it to get, store and erase"
        );
    }
    Ok(())
}

#[test]
fn named_helpers_run_without_starting_git() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let root = fixture()?;
    let tmp = gix_testtools::tempfile::tempdir()?;
    let records = tmp.path().join("records");
    let trace = tmp.path().join("git.trace");
    let _env = gix_testtools::Env::new()
        .set("PATH", path_with(&root.join("bin"))?)
        .set("CREDENTIAL_RECORDS", env_path(&records)?)
        // Git writes this file if it's started.
        .set("GIT_TRACE", env_path(&trace)?);
    fill_approve_reject(&root.join("plain"))?;
    assert!(!trace.exists(), "the program of the helper runs without Git");
    assert_eq!(
        std::fs::read(&records)?.as_bstr(),
        std::fs::read(root.join("plain").join("records"))?.as_bstr(),
        "the helper ran for each request"
    );
    Ok(())
}
