//! The cases where a reset deviates from Git, which git didn't reset.

use std::sync::atomic::AtomicBool;

use gix_index::entry::Stat;
use gix_testtools::TestResult;
use gix_worktree_state::switch::Refusals;
use pretty_assertions::assert_eq;

use crate::{
    reset::{Reset, case, done, refused, reset_index},
    switch::Snapshot,
};

#[test]
fn a_hard_reset_refuses_the_current_working_directory_anywhere_in_a_directory_that_a_file_replaces() -> TestResult {
    let (case, _tmp) = case("dev-cwd", Reset::Hard)?.writable_copy()?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;
    let refusals = refused(reset_index(&case, &mut index, Reset::Hard, &AtomicBool::new(false))??)?;
    assert_eq!(
        refusals,
        Refusals {
            current_dir: vec!["d".into()],
            ..Default::default()
        },
        "the directory that the current working directory is in is refused, where git removes it"
    );
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn the_entries_that_a_mixed_reset_changes_or_adds_record_no_stat_information() -> TestResult {
    let (case, _tmp) = case("clean", Reset::Mixed)?.writable_copy()?;
    let mut index = case.index()?;
    done(reset_index(&case, &mut index, Reset::Mixed, &AtomicBool::new(false))??)?;
    for path in ["a", "d/g", "new-dir/n", "to-dir/y", "to-file"] {
        let entry = index
            .entry_by_path(path.into())
            .ok_or_else(|| format!("{path} is an entry of the new tree"))?;
        assert_eq!(
            entry.stat,
            Stat::default(),
            "{path}: the entry isn't refreshed, where git refreshes it, so the next comparison reads its file"
        );
    }
    Ok(())
}
