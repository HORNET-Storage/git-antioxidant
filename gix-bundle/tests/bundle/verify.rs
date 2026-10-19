use std::sync::atomic::AtomicBool;

use gix_testtools::TestResult;

use crate::{fixture, git_results, objects, open_bundle, revs, tips};

#[test]
fn agrees_with_git_bundle_verify() -> TestResult {
    let dir = fixture()?;
    for git in git_results(&dir)? {
        let (header, _pack) = open_bundle(&dir, &git.bundle)?;
        let outcome = header.verify_prerequisites(
            objects(&dir, &git.receiver)?,
            &tips(&dir, &git.receiver)?,
            &AtomicBool::new(false),
        );
        match outcome {
            Ok(()) => assert!(
                git.verify,
                "{} in {}: git refused the bundle, which was accepted",
                git.bundle, git.receiver
            ),
            Err(err) => {
                assert!(
                    !git.verify,
                    "{} in {}: git accepted the bundle, which was refused: {err:?}",
                    git.bundle, git.receiver
                );
                assert!(
                    err.is_not_found(),
                    "{} in {}: the refusal reports missing objects: {err:?}",
                    git.bundle,
                    git.receiver
                );
            }
        }
    }
    Ok(())
}

#[test]
fn names_missing_prerequisites() -> TestResult {
    let dir = fixture()?;
    let (header, _pack) = open_bundle(&dir, "incremental")?;
    let err = header
        .verify_prerequisites(objects(&dir, "empty")?, &[], &AtomicBool::new(false))
        .expect_err("the receiver has no objects");
    assert!(
        err.is_not_found(),
        "a missing prerequisite is reported as not found: {err:?}"
    );
    let base_commit_id = revs(&dir)?.base_commit_id;
    assert!(
        err.to_string().contains(&base_commit_id.to_string()),
        "the error names the missing prerequisite {base_commit_id}: {err}"
    );
    Ok(())
}

#[test]
fn names_what_unconnected_prerequisites_lack() -> TestResult {
    let dir = fixture()?;
    let (header, _pack) = open_bundle(&dir, "incremental")?;
    let err = header
        .verify_prerequisites(objects(&dir, "unconnected")?, &[], &AtomicBool::new(false))
        .expect_err("the receiver lacks the tree of the prerequisite");
    assert!(
        err.is_not_found(),
        "an object missing from the history of a prerequisite is reported as not found: {err:?}"
    );
    let base_tree_id = revs(&dir)?.base_tree_id;
    assert!(
        err.to_string().contains(&base_tree_id.to_string()),
        "the error names the missing tree {base_tree_id}: {err}"
    );
    Ok(())
}

#[test]
fn trusts_the_history_of_complete_tips() -> TestResult {
    let dir = fixture()?;
    let (header, _pack) = open_bundle(&dir, "incremental")?;
    let base_commit_id = revs(&dir)?.base_commit_id;
    header.verify_prerequisites(
        objects(&dir, "unconnected")?,
        &[base_commit_id],
        &AtomicBool::new(false),
    )?;
    Ok(())
}

#[test]
fn stops_when_interrupted() -> TestResult {
    let dir = fixture()?;
    let (header, _pack) = open_bundle(&dir, "incremental")?;
    let err = header
        .verify_prerequisites(objects(&dir, "unconnected")?, &[], &AtomicBool::new(true))
        .expect_err("the interrupt is noticed while the history of the prerequisite is checked");
    assert!(err.is_cancelled(), "the interrupt is reported as cancellation: {err:?}");
    Ok(())
}
