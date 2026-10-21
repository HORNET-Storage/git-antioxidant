//! What is refused, which leaves the index as it was.

use std::sync::atomic::AtomicBool;

use filetime::FileTime;
use gix_add::{Context, add_all};
use gix_index::{State, entry, extension::Tree};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, Objects, Record, pathspec_search, records, repository_head};

/// What `index` holds: its entries with their stat information, its timestamp and its cache tree.
fn contents(index: &State) -> (Vec<(Record, entry::Stat)>, FileTime, Option<Tree>) {
    let stats = index.entries().iter().map(|entry| entry.stat);
    (
        records(index).into_iter().zip(stats).collect(),
        index.timestamp(),
        index.tree().cloned(),
    )
}

#[test]
fn an_untracked_repository_without_a_commit_is_refused_as_git_refuses_it() -> TestResult {
    let case = Case::new("nested-repository-without-commit")?;
    let git_error = case.git_error()?.ok_or("git refuses a repository without a commit")?;
    let before = case.index()?;
    assert_eq!(
        records(&case.index_after()?),
        records(&before),
        "git leaves the index as it was"
    );

    let mut index = before.clone();
    let err = case
        .add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)?
        .expect_err("a repository without a commit can't be added");
    assert_eq!(
        format!("error: {}", err.error()),
        git_error.lines().next().unwrap_or_default(),
        "the error is the first one that git reported"
    );
    assert_eq!(contents(&index), contents(&before), "the index is left as it was");
    Ok(())
}

#[test]
fn indices_with_entries_that_are_excluded_from_a_sparse_checkout_are_refused() -> TestResult {
    for (name, is_sparse) in [("sparse-index", true), ("sparse-checkout", false)] {
        let case = Case::new(name)?;
        let mut index = case.index()?;
        assert_eq!(index.is_sparse(), is_sparse, "{name}: only a sparse index is sparse");
        assert!(
            index
                .entries()
                .iter()
                .any(|entry| entry.flags.contains(entry::Flags::SKIP_WORKTREE)),
            "{name}: entries are excluded from the sparse checkout"
        );
        let before = contents(&index);

        let err = case
            .add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)?
            .expect_err("sparse checkouts aren't supported");
        assert!(err.is_unsupported(), "{name}: {err:?}");
        assert_eq!(
            err.error().to_string(),
            "Indices with entries that are excluded from a sparse checkout can't be added to",
            "{name}"
        );
        assert_eq!(contents(&index), before, "{name}: the index is left as it was");
    }
    Ok(())
}

#[test]
fn a_stack_without_exclude_patterns_is_refused() -> TestResult {
    let case = Case::new("changes")?;
    let mut index = case.index()?;
    let before = contents(&index);

    let options = case.options(None)?;
    let git_dir_realpath = gix_path::realpath(case.git_dir())?;
    let current_dir = gix_fs::current_dir(options.fs.precompose_unicode)?;
    let should_interrupt = AtomicBool::new(false);
    let stack = gix_worktree::Stack::from_state_and_ignore_case(
        &case.dir,
        options.fs.ignore_case,
        gix_worktree::stack::State::AttributesStack(Default::default()),
        &index,
        index.path_backing(),
    );
    let ctx = Context {
        pathspec: pathspec_search(&[])?,
        stack,
        filter: case.filter()?,
        git_dir_realpath: &git_dir_realpath,
        current_dir: &current_dir,
        should_interrupt: &should_interrupt,
    };
    let objects = Objects::new(&case)?;
    let err = add_all(
        &mut index,
        &case.dir,
        objects.handle.clone(),
        repository_head,
        ctx,
        options,
    )
    .expect_err("the stack has to provide exclude patterns");
    assert_eq!(
        err.error().to_string(),
        "The stack has to provide both attributes and exclude patterns"
    );
    assert_eq!(contents(&index), before, "the index is left as it was");
    Ok(())
}

#[test]
fn an_interrupted_call_leaves_the_index_unchanged() -> TestResult {
    let case = Case::new("changes")?;
    let mut index = case.index()?;
    let before = contents(&index);

    let result = case.add_interruptible(
        &mut index,
        &[],
        &Objects::new(&case)?,
        case.options(None)?,
        &AtomicBool::new(true),
    )?;
    assert!(result.is_err(), "an interrupted call fails, but returned {result:?}");
    assert_eq!(contents(&index), before, "the index is left as it was");
    Ok(())
}
