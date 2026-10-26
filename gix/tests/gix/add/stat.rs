//! The timestamp of the index, the sizes that entries record when their files may have changed after they were compared,
//! and the cache tree.

use std::time::{Duration, SystemTime};

use filetime::FileTime;
use gix_index::{State, entry, extension::Tree};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::add::{Case, Objects};

/// The first second of the year 2000, a time long before the tests run.
const YEAR_2000: i64 = 946_684_800;

/// The stat information of the entry at `path` in `index`.
fn stat(index: &State, path: &str) -> TestResult<entry::Stat> {
    Ok(index
        .entry_by_path(path.into())
        .ok_or_else(|| format!("{path} is tracked"))?
        .stat)
}

/// The node of the directory `name` among the children of `tree`.
fn child<'a>(tree: &'a Tree, name: &str) -> TestResult<&'a Tree> {
    Ok(tree
        .children
        .iter()
        .find(|child| child.name.as_slice() == name.as_bytes())
        .ok_or_else(|| format!("the cache tree has a node for {name}"))?)
}

#[test]
fn the_timestamp_of_the_index_is_the_time_at_which_the_call_began() -> TestResult {
    let case = Case::new("changes")?;
    let mut index = case.index()?;
    let start = FileTime::now();
    case.add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)??;
    let end = FileTime::now();
    assert!(
        start <= index.timestamp() && index.timestamp() <= end,
        "{:?} is between {start:?} and {end:?}",
        index.timestamp()
    );
    Ok(())
}

#[test]
fn entries_whose_files_were_modified_after_the_call_began_record_a_size_of_zero() -> TestResult {
    let (case, _tmp) = Case::writable("racy")?;
    let long_ago = FileTime::from_unix_time(YEAR_2000, 0);
    filetime::set_file_mtime(case.dir.join("old"), long_ago)?;
    filetime::set_file_mtime(case.dir.join("outside").join("kept"), long_ago)?;
    let later = FileTime::from_system_time(SystemTime::now() + Duration::from_secs(3600));
    filetime::set_file_mtime(case.dir.join("future"), later)?;
    let mut index = case.index()?;
    index.set_timestamp(FileTime::from_unix_time(YEAR_2000 + 60, 0));

    let outcome = case.add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)??;
    assert_eq!(
        (outcome.added, outcome.modified, outcome.removed),
        (0, 0, 0),
        "no file changed its content"
    );
    assert_eq!(
        stat(&index, "old")?.size,
        4,
        "files modified before the call keep their size"
    );
    assert_eq!(stat(&index, "outside/kept")?.size, 5);
    assert_eq!(
        stat(&index, "future")?.size,
        0,
        "a file modified after the call began may change again unnoticed, so that the next comparison has to read it"
    );
    Ok(())
}

#[test]
fn entries_outside_of_the_pathspec_record_a_size_of_zero_if_their_files_may_have_changed_since_the_index_was_written()
-> TestResult {
    let case = Case::new("racy")?;
    let before = case.index()?;
    let modified = stat(&before, "outside/kept")?.mtime;
    let modified_secs = i64::from(modified.secs);
    for (timestamp, size, reason) in [
        (
            FileTime::from_unix_time(modified_secs, modified.nsecs),
            0,
            "the file was modified when the index was written, so it may have changed after that unnoticed",
        ),
        (
            FileTime::from_unix_time(modified_secs + 86_400, 0),
            5,
            "the file was modified before the index was written",
        ),
    ] {
        let mut index = before.clone();
        index.set_timestamp(timestamp);
        case.add(
            &mut index,
            &["old", "future"],
            &Objects::new(&case)?,
            case.options(None)?,
        )??;
        assert_eq!(stat(&index, "outside/kept")?.size, size, "{reason}");
    }
    Ok(())
}

#[test]
fn entries_whose_files_only_changed_their_stat_information_keep_the_cache_tree_valid() -> TestResult {
    let case = Case::new("modification-time")?;
    let mut index = case.index()?;
    let cache_tree = index.tree().cloned().ok_or("the commit left a cache tree")?;

    let outcome = case.add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)??;
    assert_eq!(
        (outcome.added, outcome.modified, outcome.removed),
        (0, 0, 0),
        "no file changed its content"
    );
    let modified = FileTime::from_last_modification_time(&std::fs::symlink_metadata(case.dir.join("a"))?);
    assert_eq!(
        i64::from(stat(&index, "a")?.mtime.secs),
        modified.unix_seconds(),
        "the entry records the new modification time of its file"
    );
    assert_eq!(
        index.tree(),
        Some(&cache_tree),
        "the cache tree stays valid along the path of the entry"
    );
    Ok(())
}

#[test]
fn a_change_invalidates_the_cache_tree_only_along_its_path() -> TestResult {
    let case = Case::new("cache-tree")?;
    let mut index = case.index()?;
    let before = index.tree().cloned().ok_or("the commit left a cache tree")?;
    assert_eq!(
        child(&before, "unchanged")?.num_entries,
        Some(1),
        "the directory without changes is valid before"
    );

    case.add(&mut index, &[], &Objects::new(&case)?, case.options(None)?)??;
    let after = index.tree().ok_or("the cache tree is kept")?;
    let changed = child(after, "changed")?;
    assert_eq!(
        (
            after.num_entries,
            changed.num_entries,
            child(changed, "deeper")?.num_entries
        ),
        (None, None, None),
        "the root, the directory of the change and the directory between them are invalid"
    );
    assert_eq!(
        child(after, "unchanged")?,
        child(&before, "unchanged")?,
        "the directory without changes keeps its tree"
    );
    Ok(())
}
