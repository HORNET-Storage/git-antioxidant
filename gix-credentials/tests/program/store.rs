//! Compare Git's `credential-store`, which runs in this process for the helper `store`, with what Git did in the
//! scenarios of `make_credential_store_records.sh`.

use std::path::Path;

use bstr::{BString, ByteSlice};
use gix_credentials::{
    Program,
    helper::{Action, Cascade},
    protocol::{Context, ContextOptions},
};
use gix_testtools::Result;

/// The scenarios of the fixture, each a repository in which Git ran the steps of the scenario.
const SCENARIOS: &[&str] = &["file", "defaults", "xdg", "both", "xdg-unset", "tilde", "locked"];

/// The credentials files of the scenarios, relative to their directory.
const FILES: &[&str] = &[
    "creds",
    "creds.lock",
    "home/creds",
    "home/.git-credentials",
    "config/git/credentials",
    "home/.config/git/credentials",
];

#[test]
fn store_reads_and_writes_credentials_files_as_git_does() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let root = std::path::absolute(gix_testtools::scripted_fixture_read_only(
        "make_credential_store_records.sh",
    )?)?;
    let tmp = gix_testtools::tempfile::tempdir()?;
    let trace = tmp.path().join("git.trace");
    // Git writes this file if it's started.
    let _trace = gix_testtools::Env::new().set("GIT_TRACE", utf8(&trace)?);
    for scenario in SCENARIOS {
        run_steps(&root.join(scenario), &tmp.path().join(scenario), scenario)?;
    }
    assert!(!trace.exists(), "`store` runs without Git");
    Ok(())
}

/// Run the steps that Git ran in the scenario at `expected` with gix-credentials in `dir`, which starts with the
/// initial files of the scenario and gets `HOME` and `XDG_CONFIG_HOME` as Git had them, and compare what each step
/// printed and the credentials files after it with Git's.
fn run_steps(expected: &Path, dir: &Path, scenario: &str) -> Result {
    copy_dir(&expected.join("initial"), dir)?;
    std::fs::create_dir_all(dir.join("home"))?;
    std::fs::create_dir_all(dir.join("config"))?;
    let env = gix_testtools::Env::new().set("HOME", utf8(&dir.join("home"))?);
    let _env = if std::fs::read_to_string(expected.join("xdg-config-home"))? == "unset" {
        env.unset("XDG_CONFIG_HOME")
    } else {
        env.set("XDG_CONFIG_HOME", utf8(&dir.join("config"))?)
    };
    // Like Git in the repository of the scenario, `store --file=creds` uses `creds` there.
    let _cwd = gix_testtools::set_current_dir(dir)?;
    let helper = std::fs::read_to_string(expected.join("helper"))?;
    let mut cascade = Cascade::default()
        .use_http_path(true)
        .extend([Program::from_custom_definition(helper)?]);
    let prompt = gix_prompt::Options {
        mode: gix_prompt::Mode::Disable,
        askpass: None,
    };
    for n in 1.. {
        let step = expected.join("steps").join(n.to_string());
        if !step.is_dir() {
            break;
        }
        let op = std::fs::read_to_string(step.join("op"))?;
        let input = std::fs::read(step.join("input"))?;
        let output = match op.as_str() {
            "fill" => {
                let action = Action::Get(Context::from_bytes(&input, ContextOptions::default())?);
                let outcome = cascade
                    .invoke(action, prompt.clone())?
                    .ok_or("`store` has a credential for each `fill` of the fixture")?;
                Context::try_from(&outcome.next)?.to_bstring()
            }
            "approve" | "reject" => {
                let action = if op == "approve" {
                    Action::Store(input.into())
                } else {
                    Action::Erase(input.into())
                };
                assert!(
                    cascade.invoke(action, prompt.clone())?.is_none(),
                    "{scenario}, step {n}: storing and erasing have no outcome"
                );
                BString::default()
            }
            _ => return Err(format!("{scenario}, step {n}: unknown operation {op:?}").into()),
        };
        assert_eq!(
            output.as_bstr(),
            std::fs::read(step.join("output"))?.as_bstr(),
            "{scenario}, step {n} ({op}): the output is what `git credential {op}` printed"
        );
        for file in FILES {
            assert_eq!(
                std::fs::read(dir.join(file)).ok().map(BString::from),
                std::fs::read(step.join("files").join(file)).ok().map(BString::from),
                "{scenario}, step {n} ({op}): `{file}` is as Git left it"
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in FILES {
            if let (Ok(actual), Ok(git)) = (
                std::fs::metadata(dir.join(file)),
                std::fs::metadata(expected.join(file)),
            ) {
                assert_eq!(
                    actual.permissions().mode() & 0o777,
                    git.permissions().mode() & 0o777,
                    "{scenario}: `{file}` has the permissions that Git gave it"
                );
            }
        }
    }
    Ok(())
}

/// Return `path` for a variable of the environment.
fn utf8(path: &Path) -> Result<String> {
    Ok(path.to_str().ok_or("temporary paths are UTF-8")?.to_owned())
}

/// Copy the files in `src` and its subdirectories to `dst`.
fn copy_dir(src: &Path, dst: &Path) -> Result {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_dir(&path, &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
