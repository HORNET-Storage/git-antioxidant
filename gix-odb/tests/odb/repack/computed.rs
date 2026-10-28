//! The fixture `make_repack_deltas.sh` records the packs that `git repack -d -l --geometric=2` wrote for copies of its
//! cases, with one thread and variants of the configuration, and what `git verify-pack -v` reports of them. Other
//! copies are repacked here with the same options, and the new pack is compared with git's: it holds the same objects
//! in the same order, stores the same objects as deltas of the same bases and at the same depths, with deltas of the
//! same length, and each of its entries holds the same data, the object or the delta, as git's. Only the compressed
//! data of the entries that are compressed anew can differ, as `gix-zlib` gives other bytes than the zlib of git, and so
//! the packs differ in size by as much as these entries differ in length. With several threads, the deltas depend on
//! the speed of the threads, and the new pack is checked with `git fsck` and `git verify-pack` instead.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use gix_hash::ObjectId;
use gix_odb::repack::{Options, Written};
use gix_testtools::TestResult;

use super::Case;

/// The script that makes the cases and records what git does with them.
const FIXTURE: &str = "make_repack_deltas.sh";

/// An entry of a pack, as `git verify-pack -v` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: ObjectId,
    kind: String,
    /// The size of the object, or of the delta if the entry is a delta.
    size: u64,
    /// How many bytes the entry takes in the pack.
    size_in_pack: u64,
    offset: u64,
    /// The amount of deltas that build the object and the id of the base of its delta, if the entry is a delta.
    delta: Option<(u32, ObjectId)>,
}

/// The entries of a pack in the order of their offsets, as `git verify-pack -v` reports them in `report`.
fn entries(report: &str) -> TestResult<Vec<Entry>> {
    let hex_len = gix_testtools::object_hash().len_in_hex();
    let is_object = |id: &str, kind: &str| id.len() == hex_len && matches!(kind, "commit" | "tree" | "blob" | "tag");
    let mut entries = Vec::new();
    for line in report.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (id, kind, size, packed, offset, delta) = match fields.as_slice() {
            [id, kind, size, packed, offset] if is_object(id, kind) => (id, kind, size, packed, offset, None),
            [id, kind, size, packed, offset, depth, base] if is_object(id, kind) => {
                let delta = (depth.parse()?, ObjectId::from_hex(base.as_bytes())?);
                (id, kind, size, packed, offset, Some(delta))
            }
            _ => continue,
        };
        entries.push(Entry {
            id: ObjectId::from_hex(id.as_bytes())?,
            kind: (*kind).to_owned(),
            size: size.parse()?,
            size_in_pack: packed.parse()?,
            offset: offset.parse()?,
            delta,
        });
    }
    entries.sort_by_key(|entry| entry.offset);
    Ok(entries)
}

/// The data of the entry at `offset` of the pack `data`, decompressed: the object, or the delta if the entry is a
/// delta.
fn entry_data(data: &gix_pack::data::File, offset: u64) -> TestResult<Vec<u8>> {
    let entry = data.entry(offset)?;
    let mut out = vec![0; usize::try_from(entry.decompressed_size)?];
    data.decompress_entry(&entry, &mut gix_zlib::Inflate::default(), &mut out)?;
    Ok(out)
}

/// A copy of the case `name` of the fixture, repacked here.
struct Repacked {
    case: Case,
    written: Written,
    /// What `git verify-pack -v` reports of the pack that was written.
    entries: Vec<Entry>,
}

/// Repack a copy of the case `name` with `options`, after giving its packs the times at which they were modified when
/// git repacked its copies, and check the copy with `git fsck`.
fn repack(name: &'static str, options: Options) -> TestResult<Repacked> {
    let case = Case::of(FIXTURE, name)?;
    for line in case.record("mtimes")? {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [pack, seconds, _stamp] = fields.as_slice() else {
            return Err(format!("{name}: the line {line:?} of the times of the packs is malformed").into());
        };
        set_modified(&case.pack_dir().join(pack), seconds.parse()?)?;
    }
    let outcome = case.repack(options)?;
    case.git(["fsck", "--full", "--no-dangling"])?;
    let written = outcome
        .pack
        .ok_or("the combined packs and the loose objects were packed")?;
    let report = case.git([
        OsString::from("verify-pack"),
        "-v".into(),
        written.index_path.clone().into_os_string(),
    ])?;
    let entries = entries(&report)?;
    Ok(Repacked { case, written, entries })
}

/// Give the file at `path` the modification time of `seconds` since the epoch, although git made it read-only.
fn set_modified(path: &Path, seconds: i64) -> TestResult {
    let time = filetime::FileTime::from_unix_time(seconds, 0);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // A read-only file can be opened to write its attributes, which include its times.
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(path)?;
        filetime::set_file_handle_times(&file, None, Some(time))?;
    }
    #[cfg(not(windows))]
    filetime::set_file_mtime(path, time)?;
    Ok(())
}

/// The pack that git wrote for the variant `variant` of the case of `repacked`, and what `git verify-pack -v` reports
/// of it.
fn git_pack(repacked: &Repacked, variant: &str) -> TestResult<(PathBuf, Vec<Entry>)> {
    let dir = repacked
        .case
        .tmp
        .path()
        .join(format!("{}.{variant}", repacked.case.name));
    let report = std::fs::read_to_string(dir.join("verify-pack"))?;
    Ok((dir.join("git.pack"), entries(&report)?))
}

/// Repack a copy of the case `name` with `options` and check that the new pack is git's pack of `variant` but for the
/// compression of the entries that are compressed anew.
fn assert_as_git(name: &'static str, variant: &str, options: Options) -> TestResult {
    let repacked = repack(name, options)?;
    let (git_path, git) = git_pack(&repacked, variant)?;
    let ours = &repacked.entries;
    let label = format!("{name} ({variant})");
    assert_eq!(
        ours.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        git.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        "{label}: the new pack holds git's objects in git's order"
    );
    assert!(
        git.iter().filter(|entry| entry.delta.is_some()).count() >= 20,
        "{label}: git stores many objects as deltas"
    );

    let object_hash = gix_testtools::object_hash();
    let (our_data, git_data) = (
        gix_pack::data::File::at(&repacked.written.data_path, object_hash)?,
        gix_pack::data::File::at(&git_path, object_hash)?,
    );
    let mut size_difference = 0;
    for (our, git) in ours.iter().zip(&git) {
        assert_eq!(
            (&our.kind, our.size, our.delta),
            (&git.kind, git.size, git.delta),
            "{label}: {} is stored as git stores it, whole or as a delta of the same length of the same base at the \
             same depth",
            our.id
        );
        assert!(
            entry_data(&our_data, our.offset)? == entry_data(&git_data, git.offset)?,
            "{label}: the entry of {} holds the data of git's entry",
            our.id
        );
        size_difference += i64::try_from(our.size_in_pack)? - i64::try_from(git.size_in_pack)?;
    }
    let file_len = |path: &Path| -> TestResult<i64> { Ok(i64::try_from(std::fs::metadata(path)?.len())?) };
    assert_eq!(
        file_len(&repacked.written.data_path)? - file_len(&git_path)?,
        size_difference,
        "{label}: the packs differ in size only by the lengths of their entries"
    );
    Ok(())
}

/// Options with a single thread, and otherwise the defaults of git.
fn one_thread() -> Options {
    Options {
        thread_limit: Some(1),
        ..Options::default()
    }
}

#[test]
fn versions_get_the_deltas_of_git() -> TestResult {
    assert_as_git("versions", "default", one_thread())
}

/// Whether the installed Git stores the case `versions` as the tests with a small window expect: Git before 2.51
/// writes an object whole that later Git, like gix, stores as a delta. The fixture has no archive to take the packs of
/// later Git from, as it records those of the installed Git.
fn git_stores_small_windows_as_expected() -> bool {
    *gix_testtools::GIT_BASH_VERSION >= (2, 51, 0)
}

#[test]
fn versions_get_the_deltas_of_git_with_a_window_of_one_object() -> TestResult {
    if !git_stores_small_windows_as_expected() {
        return Ok(());
    }
    assert_as_git(
        "versions",
        "window1",
        Options {
            window: 1,
            ..one_thread()
        },
    )
}

#[test]
fn versions_get_the_deltas_of_git_with_a_window_memory() -> TestResult {
    if !git_stores_small_windows_as_expected() {
        return Ok(());
    }
    assert_as_git(
        "versions",
        "window-memory",
        Options {
            window_memory: 8 * 1024,
            ..one_thread()
        },
    )
}

/// With a depth of 2 or 1, chains of deltas that are copied are cut, and objects that copied deltas depend on can only
/// be deltas in chains that remain within the depth.
#[test]
fn versions_get_the_deltas_of_git_with_shallow_depths() -> TestResult {
    assert_as_git(
        "versions",
        "depth2",
        Options {
            delta_depth: 2,
            ..one_thread()
        },
    )?;
    assert_as_git(
        "versions",
        "depth1",
        Options {
            delta_depth: 1,
            ..one_thread()
        },
    )
}

/// Objects larger than `core.bigFileThreshold` aren't searched, and of an object whose delta can be copied, the length
/// of the delta counts.
#[test]
fn versions_get_the_deltas_of_git_with_a_big_file_threshold() -> TestResult {
    assert_as_git(
        "versions",
        "threshold",
        Options {
            big_file_threshold: 2000,
            ..one_thread()
        },
    )
}

/// When no computed delta is kept, each is computed again when it is written, which gives the same deltas.
#[test]
fn deltas_that_are_computed_again_are_those_of_git() -> TestResult {
    assert_as_git(
        "versions",
        "default",
        Options {
            delta_cache_size: 1,
            ..one_thread()
        },
    )
}

/// Chains of deltas that are longer than the depth are cut, and the objects that the cut chains leave whole are
/// searched for deltas that keep the chains that depend on them within the depth.
#[test]
fn chains_get_the_deltas_of_git() -> TestResult {
    assert_as_git("chain", "default", one_thread())?;
    assert_as_git(
        "chain",
        "depth3",
        Options {
            delta_depth: 3,
            ..one_thread()
        },
    )
}

/// With several threads, the new pack holds the objects that git's pack holds, in chains of at most 50 deltas, and is
/// about as large as the pack that git wrote with as many threads.
#[test]
fn several_threads_find_deltas_like_git() -> TestResult {
    for name in ["versions", "chain"] {
        let repacked = repack(
            name,
            Options {
                thread_limit: Some(4),
                ..Options::default()
            },
        )?;
        let (git_path, git) = git_pack(&repacked, "threads")?;
        let ids = |entries: &[Entry]| {
            let mut ids: Vec<_> = entries.iter().map(|entry| entry.id).collect();
            ids.sort();
            ids
        };
        assert_eq!(
            ids(&repacked.entries),
            ids(&git),
            "{name}: the new pack holds the objects of git's pack"
        );
        assert!(
            repacked
                .entries
                .iter()
                .all(|entry| entry.delta.is_none_or(|(depth, _)| depth <= 50)),
            "{name}: chains of deltas are at most 50 deltas long"
        );
        let (ours, git) = (
            std::fs::metadata(&repacked.written.data_path)?.len(),
            std::fs::metadata(&git_path)?.len(),
        );
        assert!(
            ours <= git + git / 20,
            "{name}: the new pack has {ours} bytes, not much more than the {git} bytes of git's pack"
        );
    }
    Ok(())
}
