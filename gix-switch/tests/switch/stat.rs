//! The timestamp of the index after a switch, and the stat information that its entries record.

use filetime::FileTime;
use gix_index::{State, entry};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, switched};

/// The first second of the year 2000, a time long before the tests run.
const YEAR_2000: u32 = 946_684_800;

/// The stat information of the entry at `path` in `index`.
fn stat(index: &State, path: &str) -> TestResult<entry::Stat> {
    Ok(index
        .entry_by_path(path.into())
        .ok_or_else(|| format!("{path} is tracked"))?
        .stat)
}

/// Make the entry at `path` in `index` record a file of `size` bytes that was modified at `secs`.
fn set_stat(index: &mut State, path: &str, secs: u32, size: u32) -> TestResult {
    let position = index
        .entry_index_by_path(path.into())
        .map_err(|_| format!("{path} is tracked"))?;
    let stat = &mut index.entries_mut()[position].stat;
    stat.mtime = entry::stat::Time { secs, nsecs: 0 };
    stat.size = size;
    Ok(())
}

#[test]
fn the_timestamp_of_the_index_is_the_time_at_which_the_switch_began() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let start = FileTime::now();
    switched(case.switch(&mut index)??)?;
    let end = FileTime::now();
    assert!(
        start <= index.timestamp() && index.timestamp() <= end,
        "{:?} is between {start:?} and {end:?}",
        index.timestamp()
    );
    Ok(())
}

#[test]
fn the_entries_of_checked_out_files_record_their_sizes_and_modification_times() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    switched(case.switch(&mut index)??)?;
    for path in ["a", "d/g", "new-dir/n"] {
        let metadata = std::fs::symlink_metadata(case.dir.join(path))?;
        let stat = stat(&index, path)?;
        assert_eq!(
            (u64::from(stat.size), i64::from(stat.mtime.secs)),
            (
                metadata.len(),
                FileTime::from_last_modification_time(&metadata).unix_seconds()
            ),
            "{path}: the entry records the stat information of its new file"
        );
    }
    Ok(())
}

#[test]
fn entries_that_stay_record_a_size_of_zero_if_their_files_may_have_changed_since_the_index_was_written() -> TestResult {
    let (case, _tmp) = Case::writable("kept")?;
    let mut index = case.index()?;
    index.set_timestamp(FileTime::from_unix_time(i64::from(YEAR_2000) + 60, 0));
    set_stat(&mut index, "c", YEAR_2000, 3)?;
    set_stat(&mut index, "staged", YEAR_2000 + 60, 8)?;

    switched(case.switch(&mut index)??)?;
    assert_eq!(
        stat(&index, "c")?.size,
        3,
        "the file was modified before the index was written, so the entry keeps its size"
    );
    assert_eq!(
        stat(&index, "staged")?.size,
        0,
        "the file was modified when the index was written, so it may have changed after that unnoticed, and the next \
         comparison has to read it"
    );
    Ok(())
}
