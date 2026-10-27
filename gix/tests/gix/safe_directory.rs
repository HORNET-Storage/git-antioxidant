//! A port of Git's `t0033-safe-directory.sh` with Git as reference: `make_safe_directory_baseline.sh` records whether Git
//! opens a repository in each case while it considers no repository owned by the current user, so `safe.directory`
//! decides, and `gix` must decide the same way when discovering and opening it.
use std::path::Path;

use gix::{
    discover::upwards::{self, TrustPolicy},
    open::DubiousOwnership,
    sec::Trust,
};
use gix_testtools::Result;

/// A case of the baseline, see `make_safe_directory_baseline.sh`.
struct Case {
    name: String,
    git_opens: bool,
    /// The directory from which the repository is found, relative to the fixture.
    start: String,
}

fn cases(root: &Path) -> Result<Vec<Case>> {
    std::fs::read_to_string(root.join("baseline"))?
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let [name, verdict, start] = fields[..] else {
                return Err(format!("invalid baseline line {line:?}").into());
            };
            Ok(Case {
                name: name.into(),
                git_opens: match verdict {
                    "open" => true,
                    "refused" => false,
                    _ => return Err(format!("invalid verdict in {line:?}").into()),
                },
                start: start.into(),
            })
        })
        .collect()
}

/// Return the lines of the file of `case` with `extension`.
fn case_file(root: &Path, case: &str, extension: &str) -> Result<Vec<String>> {
    Ok(
        std::fs::read_to_string(root.join("cases").join(format!("{case}.{extension}")))?
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

/// Return the directory Git reported in the output of `case`, if it refused the repository due to its ownership.
fn directory_reported_by_git(root: &Path, case: &str) -> Result<Option<String>> {
    const PREFIX: &str = "fatal: detected dubious ownership in repository at '";
    Ok(case_file(root, case, "err")?
        .iter()
        .find_map(|line| line.strip_prefix(PREFIX)?.strip_suffix('\'').map(ToOwned::to_owned)))
}

fn options(cli: &[String]) -> gix::open::Options {
    gix::open::Options::default().cli_overrides(cli.iter().map(String::as_str))
}

fn trust_map(options: gix::open::Options) -> gix::sec::trust::Mapping<gix::open::Options> {
    gix::sec::trust::Mapping {
        full: options.clone(),
        reduced: options,
    }
}

/// Return `Ok(true)` if `outcome` succeeded and `Ok(false)` if it failed with [`DubiousOwnership`], whose directory is
/// also returned, or the description of any other error.
fn decision<T>(outcome: gix::Result<T>) -> std::result::Result<(bool, Option<String>), String> {
    match outcome {
        Ok(_) => Ok((true, None)),
        Err(err) => match err.downcast_any_ref::<DubiousOwnership>() {
            Some(dubious_ownership) => Ok((false, Some(dubious_ownership.directory.to_string()))),
            None => Err(format!("{err:#}")),
        },
    }
}

#[test]
fn decisions_match_git() -> Result {
    // Run alone, as each case changes the environment and the current directory.
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    // The root must be absolute as each case changes the current directory.
    let root = std::env::current_dir()?.join(gix_testtools::scripted_fixture_read_only(
        "make_safe_directory_baseline.sh",
    )?);
    let cases = cases(&root)?;
    assert!(cases.len() > 50, "all cases are recorded");

    let mut mismatches = Vec::new();
    for case in &cases {
        let config = root.join("cases").join(format!("{}.config", case.name));
        let env = case_file(&root, &case.name, "env")?;
        let cli = case_file(&root, &case.name, "cli")?;
        let mut case_env = gix_testtools::Env::new()
            .set("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .set("GIT_CONFIG_GLOBAL", config.to_str().expect("valid UTF-8"));
        for assignment in &env {
            let (name, value) = assignment.split_once('=').expect("NAME=value");
            case_env = case_env.set(name, value);
        }
        // Like `git -C <start>`, start in the directory and let `.` values refer to it. It's passed as absolute path
        // as gix takes a relative `.` within a `.git` directory for the worktree, which is unrelated to `safe.directory`.
        let start = root.join(&case.start);
        let _cwd = gix_testtools::set_current_dir(&start)?;

        let mut outcomes = vec![(
            "discovery with required full trust",
            decision(gix::ThreadSafeRepository::discover_opts(
                &start,
                upwards::Options {
                    trust: TrustPolicy::Required(Trust::Full),
                    ..Default::default()
                },
                trust_map(options(&cli)),
            )),
        )];
        let is_repository = !case.start.ends_with("/sub") && !case.start.ends_with("/refs");
        if is_repository {
            outcomes.push((
                "opening while bailing if untrusted",
                decision(gix::open_opts(&start, options(&cli).bail_if_untrusted(true))),
            ));
        }
        let discovered = gix::ThreadSafeRepository::discover_opts(&start, Default::default(), trust_map(options(&cli)))
            .map(gix::Repository::from);
        outcomes.push((
            "trust after discovery",
            discovered
                .map(|repo| (repo.git_dir_trust() == Trust::Full, None))
                .map_err(|err| format!("{err:#}")),
        ));

        let git_directory = directory_reported_by_git(&root, &case.name)?;
        for (what, outcome) in outcomes {
            match outcome {
                Ok((opens, _)) if opens != case.git_opens => mismatches.push(format!(
                    "{}: Git {} the repository, but {what} {} it",
                    case.name,
                    if case.git_opens { "opens" } else { "refuses" },
                    if opens { "opens" } else { "refuses" }
                )),
                Ok((_, Some(directory))) if git_directory.as_ref().is_some_and(|git| *git != directory) => {
                    mismatches.push(format!(
                        "{}: Git reports the directory {git_directory:?}, but {what} reports {directory:?}",
                        case.name
                    ));
                }
                Ok(_) => {}
                Err(err) => mismatches.push(format!("{}: {what} failed unexpectedly: {err}", case.name)),
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "gix decides differently than Git in {} of {} cases:\n{}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n")
    );
    Ok(())
}

/// The refusal names the directory to add to `safe.directory`, like Git does, and with it the repository opens.
#[test]
fn refusal_names_the_directory_to_allow() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let root = gix_testtools::scripted_fixture_read_only("make_safe_directory_baseline.sh")?;
    let _env = gix_testtools::Env::new().set("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1");

    let err = gix::open_opts(root.join("repo"), gix::open::Options::default().bail_if_untrusted(true))
        .expect_err("nothing allows the repository");
    assert!(err.is_permission_denied(), "{err:#}");
    let dubious_ownership = err
        .downcast_any_ref::<DubiousOwnership>()
        .expect("the dubious ownership is the reason");
    assert_eq!(
        Some(dubious_ownership.directory.to_string()),
        directory_reported_by_git(&root, "unset")?,
        "the directory is spelled like Git does"
    );
    assert_eq!(
        dubious_ownership.not_owned, None,
        "with GIT_TEST_ASSUME_DIFFERENT_OWNER, ownership isn't the reason"
    );
    let message = dubious_ownership.to_string();
    assert!(
        message.starts_with(&format!(
            "Detected dubious ownership in repository at '{}'",
            dubious_ownership.directory
        )),
        "{message}"
    );
    assert!(
        message.contains("git config --global --add safe.directory"),
        "the message tells how to allow the repository: {message}"
    );

    let allowed = format!("safe.directory={}", dubious_ownership.directory);
    let repo = gix::open_opts(
        root.join("repo"),
        gix::open::Options::default()
            .bail_if_untrusted(true)
            .config_overrides([allowed.as_str()]),
    )?;
    assert_eq!(
        repo.git_dir_trust(),
        Trust::Full,
        "values passed through the API are protected configuration"
    );
    Ok(())
}

/// Reloading or reopening a repository with its options compares the same directory as opening it, even though it's
/// reopened from its git directory, which Git would compare instead. Other repositories are compared as usual.
#[test]
fn reopening_keeps_comparing_the_same_directory() -> Result {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let root = gix_testtools::scripted_fixture_read_only("make_safe_directory_baseline.sh")?;
    let _env = gix_testtools::Env::new()
        .set("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
        .set(
            "GIT_CONFIG_GLOBAL",
            root.join("cases/match.config").to_str().expect("valid UTF-8"),
        );

    let mut repo = gix::open_opts(root.join("repo"), gix::open::Options::default().bail_if_untrusted(true))?;
    assert_eq!(repo.git_dir_trust(), Trust::Full, "the worktree is allowed");
    repo.reload()?;
    assert_eq!(
        repo.git_dir_trust(),
        Trust::Full,
        "the worktree is compared again, not the git directory the repository is reloaded from"
    );
    // The options carry the trust of the repository, which is reduced so `safe.directory` decides again.
    let reopened = gix::open_opts(repo.git_dir(), repo.open_options().clone().with(Trust::Reduced))?;
    assert_eq!(
        reopened.git_dir_trust(),
        Trust::Full,
        "reopening the git directory with the options of the repository compares the worktree as well"
    );

    let err = gix::open_opts(
        root.join("repo/.git"),
        gix::open::Options::default().bail_if_untrusted(true),
    )
    .expect_err("without them, the git directory is compared, like Git does when starting in it");
    assert!(err.downcast_any_ref::<DubiousOwnership>().is_some(), "{err:#}");
    let err = gix::open_opts(root.join("bare.git"), repo.open_options().clone().with(Trust::Reduced))
        .expect_err("another repository opened with these options compares its own directory");
    assert!(err.downcast_any_ref::<DubiousOwnership>().is_some(), "{err:#}");
    Ok(())
}
