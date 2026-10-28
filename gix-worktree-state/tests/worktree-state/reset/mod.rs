//! The fixture records what `git reset` made of each of its cases in a copy of the case for each mode: the index and the
//! worktree that it left, or the path that it refused. Each test resets a copy of a case as git did and compares the
//! outcome with that of git. The cases where this crate deviates from git, and the errors, are checked by the tests
//! themselves.

mod deviation;
mod error;
mod git;

use std::sync::atomic::AtomicBool;

use bstr::{BStr, BString, ByteSlice};
use gix_hash::oid;
use gix_index::State;
use gix_testtools::TestResult;
use gix_worktree_state::{
    reset::{self, Mode, Outcome},
    switch::{Changes, Context, Refusals},
};
use pretty_assertions::assert_eq;

use crate::switch::{
    Case, STAT, Snapshot, assert_cache_tree_covers_git, assert_cache_tree_is_valid, changes, quoted, records, worktree,
};

/// The script that makes the cases.
const FIXTURE: &str = "make_reset.sh";

/// How git reset a copy of a case, which is named after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reset {
    /// `git reset --hard new`.
    Hard,
    /// `git reset --mixed new`.
    Mixed,
    /// `git reset --merge new`.
    Merge,
    /// `git reset --keep new`.
    Keep,
    /// `git reset new -- <path>...` with the paths of `.git/pathspec`.
    Paths,
}

impl Reset {
    /// The name of the copy of a case that git reset this way, after the name of the case.
    fn name(self) -> &'static str {
        match self {
            Reset::Hard => "hard",
            Reset::Mixed => "mixed",
            Reset::Merge => "merge",
            Reset::Keep => "keep",
            Reset::Paths => "paths",
        }
    }
}

/// The case `name` of the fixture, along with its copy that git reset as `reset` says.
fn case(name: &'static str, reset: Reset) -> TestResult<Case> {
    let root = std::path::absolute(gix_testtools::scripted_fixture_read_only(FIXTURE)?)?;
    Ok(Case {
        name,
        dir: root.join(name),
        git: root.join(format!("{name}-{}", reset.name())),
    })
}

/// Reset `index`, the index of `case`, to the tree of `new` as `reset` says, with `should_interrupt` as the flag that
/// interrupts it, and return the outcome. The outer result fails if the reset can't be set up.
fn reset_index(
    case: &Case,
    index: &mut State,
    reset: Reset,
    should_interrupt: &AtomicBool,
) -> TestResult<gix_error::Result<Outcome>> {
    let (old_tree, new_tree) = case.trees()?;
    let mode = match reset {
        Reset::Hard => Mode::Hard,
        Reset::Merge => Mode::Merge,
        Reset::Keep => Mode::Keep {
            head_tree: old_tree.ok_or("a keep reset needs the tree of HEAD")?,
        },
        Reset::Mixed | Reset::Paths => {
            let pathspec = pathspec(case, reset)?;
            let changes = reset::mixed(
                index,
                &new_tree,
                &case.objects()?,
                |path| is_selected(&pathspec, path),
                case.options()?.checkout.validate,
                STAT,
            );
            return Ok(changes.map(Outcome::Reset));
        }
    };
    reset_to(case, index, &new_tree, mode, case.objects()?, should_interrupt)
}

/// Reset `index`, the index of `case`, and its worktree to `tree` in `mode` with the objects of `objects`, from the
/// directory in which git ran, with `should_interrupt` as the flag that interrupts the reset. The outer result fails if
/// the reset can't be set up.
fn reset_to<O>(
    case: &Case,
    index: &mut State,
    tree: &oid,
    mode: Mode,
    objects: O,
    should_interrupt: &AtomicBool,
) -> TestResult<gix_error::Result<Outcome>>
where
    O: gix_object::Find + gix_object::Exists + Send + Clone,
{
    let options = case.options()?;
    let git_dir_realpath = gix_path::realpath(case.git_dir())?;
    let current_dir = case.current_dir()?;
    let ctx = Context {
        stack: case.stack(index, options.checkout.fs.ignore_case)?,
        git_dir_realpath: &git_dir_realpath,
        current_dir: &current_dir,
        should_interrupt,
    };
    Ok(gix_worktree_state::reset(
        index, &case.dir, tree, mode, objects, ctx, options,
    ))
}

/// The paths whose entries `reset` resets in `case`: those of `.git/pathspec` for `Reset::Paths`, and all paths
/// otherwise, which an empty list stands for.
fn pathspec(case: &Case, reset: Reset) -> TestResult<Vec<BString>> {
    if reset != Reset::Paths {
        return Ok(Vec::new());
    }
    let paths = std::fs::read(case.git_dir().join("pathspec"))?;
    Ok(paths.lines().map(|line| line.as_bstr().to_owned()).collect())
}

/// Return `true` if `pathspec` is empty, or if `path` is one of its paths or in one of them, as git matches paths without
/// wildcards.
fn is_selected(pathspec: &[BString], path: &BStr) -> bool {
    pathspec.is_empty()
        || pathspec.iter().any(|spec| {
            path.strip_prefix(spec.as_slice())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
        })
}

/// Reset a copy of the case `name` as git did with `reset`, and assert that this leaves what git left. If git reset the
/// copy, the index holds the entries and the worktree the files that git left, the outcome counts the entries that
/// changed, and wherever the cache tree of git is valid, so is that of the reset. If git refused to reset the copy, the
/// path that it named is refused, and the index and the worktree are left as they were.
fn assert_matches_git(name: &'static str, reset: Reset) -> TestResult<Outcome> {
    let (case, _tmp) = case(name, reset)?.writable_copy()?;
    let before = case.index()?;
    let snapshot = Snapshot::take(&case, &before)?;
    let mut index = before.clone();
    let outcome = reset_index(&case, &mut index, reset, &AtomicBool::new(false))??;
    match (&outcome, case.git_error()?) {
        (Outcome::Reset(changed), None) => assert_reset_as_git(&case, &before, &index, changed)?,
        (Outcome::Refused(refusals), Some(error)) => {
            assert_eq!(
                *refusals,
                git_refusals(&error, &before)?,
                "{name}: the path that git refused with {reset:?} is refused"
            );
            snapshot.assert_unchanged(&case, &index)?;
        }
        (outcome, error) => {
            return Err(format!(
                "{name}: git and the reset disagree about resetting with {reset:?}: {outcome:?}, {error:?}"
            )
            .into());
        }
    }
    Ok(outcome)
}

/// Assert that `index`, which the reset of `case` from `before` left with the outcome `changed`, and the worktree of
/// `case` hold what git left, that the outcome counts the entries that changed, and that all files were checked out and
/// removed.
fn assert_reset_as_git(case: &Case, before: &State, index: &State, changed: &Changes) -> TestResult {
    let name = case.name;
    let git = case.git_index()?;
    assert_eq!(
        records(index),
        records(&git),
        "{name}: the entries are those that git left"
    );
    assert_eq!(
        worktree(&case.dir)?,
        worktree(&case.git)?,
        "{name}: the worktree holds what git left in it"
    );
    assert_eq!(
        (changed.added, changed.modified, changed.removed),
        changes(before, index),
        "{name}: the outcome counts the entries that were added, modified and removed"
    );
    let checkout = &changed.checkout;
    assert!(
        checkout.errors.is_empty()
            && checkout.collisions.is_empty()
            && changed.checkout_error.is_none()
            && changed.unremoved.is_empty(),
        "{name}: all files were checked out and removed: {changed:?}"
    );
    assert_cache_tree_is_valid(case, index)?;
    assert_cache_tree_covers_git(name, index.tree(), git.tree());
    Ok(())
}

/// The refusals that `error` stands for, which git printed when it refused to reset `index`: the path that git named with
/// the sentence of its group, or the conflicts of `index` if git refused a keep reset in the middle of a merge.
fn git_refusals(error: &str, index: &State) -> TestResult<Refusals> {
    let mut refusals = Refusals::default();
    for line in error.lines() {
        let (paths, path) =
            if let Some(path) = quoted(line, "error: Entry '", "' would be overwritten by merge. Cannot merge.") {
                (&mut refusals.staged_changes, path)
            } else if let Some(path) = quoted(line, "error: Entry '", "' not uptodate. Cannot merge.") {
                (&mut refusals.unstaged_changes, path)
            } else if let Some(path) = quoted(line, "error: Updating '", "' would lose untracked files in it") {
                (&mut refusals.directories_with_untracked_files, path)
            } else if let Some(path) = quoted(
                line,
                "error: Refusing to remove '",
                "' since it is the current working directory.",
            ) {
                (&mut refusals.current_dir, path)
            } else if let Some(path) = quoted(
                line,
                "error: Untracked working tree file '",
                "' would be overwritten by merge.",
            ) {
                (&mut refusals.untracked_overwritten, path)
            } else if let Some(path) = quoted(
                line,
                "error: Untracked working tree file '",
                "' would be removed by merge.",
            ) {
                (&mut refusals.untracked_removed, path)
            } else if line == "fatal: Cannot do a keep reset in the middle of a merge." {
                refusals.unmerged = conflicts(index);
                continue;
            } else if line == "fatal: Could not reset index file to revision 'new'." {
                continue;
            } else {
                return Err(format!("git printed an unexpected line: {line:?}").into());
            };
        paths.push(path.into());
    }
    Ok(refusals)
}

/// The paths of the conflicts of `index`.
fn conflicts(index: &State) -> Vec<BString> {
    let mut paths: Vec<BString> = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .map(|entry| entry.path(index).to_owned())
        .collect();
    paths.dedup();
    paths
}

/// What a reset changed if it happened, or the test fails.
fn done(outcome: Outcome) -> TestResult<Changes> {
    match outcome {
        Outcome::Reset(changed) => Ok(changed),
        Outcome::Refused(refusals) => Err(format!("the reset is refused: {refusals:?}").into()),
    }
}

/// The paths that a reset refused, or the test fails if it happened.
fn refused(outcome: Outcome) -> TestResult<Refusals> {
    match outcome {
        Outcome::Refused(refusals) => Ok(refusals),
        Outcome::Reset(changed) => Err(format!("the reset happened: {changed:?}").into()),
    }
}
