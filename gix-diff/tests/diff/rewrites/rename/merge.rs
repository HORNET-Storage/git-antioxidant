//! The renames of one side of a merge, among files whose blobs are kept in memory. Where a merge with git 2.55 shows
//! which renames it found, the documentation of a test says what that merge does with the same files.

use std::collections::HashMap;

use bstr::{BString, ByteSlice};
use gix_diff::rewrites::{
    rename::{
        Blobs, Content, Entry, MAX_SCORE,
        merge::{
            DirectoryRelevance::{self, ForAncestor, ForSelf, NotRelevant},
            Options, Outcome, Source, SourceRelevance, detect,
        },
    },
    strmap,
};
use gix_hash::ObjectId;
use gix_object::tree::EntryKind;
use gix_testtools::TestResult;

/// A deleted file whose content the merge needs where the file went.
const CONTENT: Option<SourceRelevance> = Some(SourceRelevance::Content);
/// A deleted file whose new location only shows where its directory went.
const LOCATION: Option<SourceRelevance> = Some(SourceRelevance::Location);
/// A deleted file whose rename doesn't matter to the merge.
const IRRELEVANT: Option<SourceRelevance> = None;

/// One side of a merge, with the blobs of its files.
#[derive(Default)]
struct Side {
    /// The files that the side deleted, with why the merge needs to know where they went, in the order of the merge.
    deleted: Vec<(BString, ObjectId, Option<SourceRelevance>)>,
    /// The files that the side added, in the order of the merge.
    added: Vec<(BString, ObjectId)>,
    /// The directories that the side removed, with how much the merge needs to know where they went.
    dirs_removed: HashMap<BString, DirectoryRelevance>,
    blobs: HashMap<ObjectId, Vec<u8>>,
}

impl Side {
    fn delete(mut self, location: &str, data: &[u8], relevance: Option<SourceRelevance>) -> Self {
        let id = self.blob(data);
        self.deleted.push((location.into(), id, relevance));
        self
    }

    fn add(mut self, location: &str, data: &[u8]) -> Self {
        let id = self.blob(data);
        self.added.push((location.into(), id));
        self
    }

    fn remove_directory(mut self, location: &str, relevance: DirectoryRelevance) -> Self {
        self.dirs_removed.insert(location.into(), relevance);
        self
    }

    fn blob(&mut self, data: &[u8]) -> ObjectId {
        let id = gix_object::compute_hash(gix_testtools::object_hash(), gix_object::Kind::Blob, data)
            .expect("the blobs of a test can be hashed");
        self.blobs.insert(id, data.to_vec());
        id
    }

    /// Find the renames of the side with `options`, and return them as `R<similarity> <source> <destination>` in the
    /// order of the destinations, along with all that was found.
    fn renames(&self, options: Options) -> TestResult<(Vec<String>, Outcome)> {
        let sources: Vec<_> = self
            .deleted
            .iter()
            .map(|(location, id, relevance)| Source {
                entry: entry(location, id),
                relevance: *relevance,
            })
            .collect();
        let destinations: Vec<_> = self.added.iter().map(|(location, id)| entry(location, id)).collect();
        let outcome = detect(
            &sources,
            &destinations,
            &self.dirs_removed,
            &mut MemoryBlobs(&self.blobs),
            options,
        )?;
        let renames = outcome
            .renames
            .iter()
            .map(|rename| {
                format!(
                    "R{:03} {} {}",
                    rename.score * 100 / MAX_SCORE,
                    self.deleted[rename.source].0,
                    self.added[rename.destination].0
                )
            })
            .collect();
        Ok((renames, outcome))
    }
}

fn entry<'a>(location: &'a BString, id: &'a ObjectId) -> Entry<'a> {
    Entry {
        location: location.as_bstr(),
        mode: EntryKind::Blob.into(),
        id,
    }
}

/// Blobs kept in memory by their ids.
struct MemoryBlobs<'a>(&'a HashMap<ObjectId, Vec<u8>>);

impl MemoryBlobs<'_> {
    fn data(&self, entry: &Entry<'_>) -> &[u8] {
        self.0.get(entry.id).expect("every file of a test has its blob")
    }
}

impl Blobs for MemoryBlobs<'_> {
    fn size(&mut self, entry: &Entry<'_>) -> gix_error::Result<u64> {
        Ok(self.data(entry).len() as u64)
    }

    fn content<'buf>(&mut self, entry: &Entry<'_>, buf: &'buf mut Vec<u8>) -> gix_error::Result<(&'buf [u8], Content)> {
        buf.clear();
        buf.extend_from_slice(self.data(entry));
        let data: &'buf [u8] = buf;
        Ok((data, Content::from_data(data)))
    }
}

/// The lines `lines` of the text of `family`, which all have the same length, so that two texts of a family that have
/// 90% of their lines in common are 90% similar.
fn text(family: &str, lines: std::ops::Range<usize>) -> Vec<u8> {
    lines
        .flat_map(|line| format!("{family} {line:04}\n").into_bytes())
        .collect()
}

/// For each directory, the directories that renames moved its files to and the number of these renames.
fn directory_renames(counts: &[(&str, &[(&str, usize)])]) -> HashMap<BString, HashMap<BString, usize>> {
    counts
        .iter()
        .map(|(directory, destinations)| {
            (
                (*directory).into(),
                destinations
                    .iter()
                    .map(|(destination, renames)| ((*destination).into(), *renames))
                    .collect(),
            )
        })
        .collect()
}

/// The directory renames counted for an identical file moved from `source` to `destination` by a side that removed
/// the directories `removed`.
fn counts_of(
    source: &str,
    destination: &str,
    removed: &[(&str, DirectoryRelevance)],
) -> TestResult<HashMap<BString, HashMap<BString, usize>>> {
    let data = text("moved", 0..20);
    let mut side = Side::default().delete(source, &data, CONTENT).add(destination, &data);
    for &(directory, relevance) in removed {
        side = side.remove_directory(directory, relevance);
    }
    Ok(side.renames(Options::default())?.1.directory_renames)
}

#[test]
fn identical_files_are_paired_whatever_their_relevance_in_the_order_of_the_merge() -> TestResult {
    let data = text("same", 0..20);
    let (renames, _) = Side::default()
        .delete("u/one", &data, IRRELEVANT)
        .delete("v/two", &data, IRRELEVANT)
        .add("w/three", &data)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 u/one w/three"],
        "the first of the identical sources is taken"
    );

    let (renames, _) = Side::default()
        .delete("v/two", &data, IRRELEVANT)
        .delete("u/one", &data, IRRELEVANT)
        .add("w/three", &data)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 v/two w/three"],
        "unlike `git diff`, a merge doesn't order the files by their locations"
    );
    Ok(())
}

/// git 2.55 doesn't pair files whose blob is empty in a merge: if one side moves an empty `e` to `f` and the other side
/// changes `e`, `e` is in conflict as deleted by one side and modified by the other.
#[test]
fn empty_files_are_paired_only_if_asked() -> TestResult {
    let side = || {
        Side::default()
            .delete("old/empty", b"", CONTENT)
            .delete("old/full", &text("full", 0..20), CONTENT)
            .add("new/empty", b"")
            .add("new/full", &text("full", 0..20))
    };
    let (renames, _) = side().renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 old/full new/full"],
        "a merge leaves empty files unpaired"
    );

    let (renames, _) = side().renames(Options {
        rename_empty: true,
        ..Options::default()
    })?;
    assert_eq!(
        renames,
        ["R100 old/empty new/empty", "R100 old/full new/full"],
        "unless they are to be paired"
    );
    Ok(())
}

/// With `-X find-renames=100%`, git 2.55 pairs only identical files and detects no directory rename: a file that the
/// other side added to `old` stays there, while with the default threshold it is moved to `new`, in conflict for its
/// location.
#[test]
fn the_highest_threshold_pairs_only_identical_files_and_counts_no_directory_renames() -> TestResult {
    let side = || {
        Side::default()
            .delete("old/a", &text("a", 0..20), LOCATION)
            .delete("old/b", &text("b", 0..20), LOCATION)
            .add("new/a", &text("a", 0..20))
            .add("new/b", &text("b", 1..21))
            .remove_directory("old", ForSelf)
    };
    let (renames, outcome) = side().renames(Options {
        min_score: MAX_SCORE,
        ..Options::default()
    })?;
    assert_eq!(renames, ["R100 old/a new/a"], "only the identical file is paired");
    assert_eq!(
        outcome.directory_renames,
        directory_renames(&[]),
        "and no rename is counted"
    );

    let (renames, outcome) = side().renames(Options::default())?;
    assert_eq!(renames, ["R100 old/a new/a", "R095 old/b new/b"]);
    assert_eq!(
        outcome.directory_renames,
        directory_renames(&[("old", &[("new", 2)])]),
        "by default, the identical and the similar file count for their directory"
    );
    Ok(())
}

#[test]
fn sources_without_relevance_are_paired_only_with_identical_files() -> TestResult {
    let (renames, _) = Side::default()
        .delete("a/one", &text("one", 0..20), IRRELEVANT)
        .delete("a/two", &text("two", 0..20), CONTENT)
        .delete("a/three", &text("three", 0..20), IRRELEVANT)
        .add("b/one", &text("one", 1..21))
        .add("b/three", &text("three", 0..20))
        .add("b/two", &text("two", 1..21))
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 a/three b/three", "R095 a/two b/two"],
        "`a/one` isn't paired by its file name although it is as similar as `a/two`"
    );
    Ok(())
}

/// git 2.55 merges a change of `a/f` into `b/g` if the first side also deleted an unrelated `c/f` that the other side
/// kept, and into `b/f` otherwise.
#[test]
fn sources_without_relevance_still_share_their_file_names() -> TestResult {
    let side = || {
        Side::default()
            .delete("a/f", &text("f", 0..20), CONTENT)
            .add("b/f", &text("f", 4..24))
            .add("b/g", &text("f", 1..21))
            .remove_directory("a", NotRelevant)
    };
    let (renames, outcome) = side()
        .delete("c/f", &text("g", 0..20), IRRELEVANT)
        .remove_directory("c", NotRelevant)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R095 a/f b/g"],
        "two deleted files are named `f`, so `a/f` goes to the most similar file"
    );
    assert_eq!(
        outcome.directory_renames,
        directory_renames(&[]),
        "the renames of directories that don't matter aren't returned"
    );

    let (renames, _) = side().renames(Options::default())?;
    assert_eq!(
        renames,
        ["R080 a/f b/f"],
        "a file name of one deleted and one added file pairs them"
    );
    Ok(())
}

/// git 2.55 merges a change of `old/x` into `new/x` if the first side moved `old/a` unchanged to `new/a`, and into the
/// more similar `lib/x` otherwise.
#[test]
fn a_shared_file_name_goes_to_the_directory_that_identical_files_went_to() -> TestResult {
    let (renames, _) = Side::default()
        .delete("old/a", &text("a", 0..20), IRRELEVANT)
        .delete("old/x", &text("x", 0..20), CONTENT)
        .add("lib/x", &text("x", 1..21))
        .add("new/a", &text("a", 0..20))
        .add("new/x", &text("x", 4..24))
        .remove_directory("old", NotRelevant)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 old/a new/a", "R080 old/x new/x"],
        "`old/a` went to `new`, so `old/x` goes to `new/x`"
    );

    let (renames, _) = Side::default()
        .delete("old/x", &text("x", 0..20), CONTENT)
        .add("lib/x", &text("x", 1..21))
        .add("new/x", &text("x", 4..24))
        .remove_directory("old", NotRelevant)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R095 old/x lib/x"],
        "without an identical file, the more similar one is taken"
    );
    Ok(())
}

/// git 2.55 merges a change of `old/x` into `n2/x`, although `n1/x` is more similar and the first side moved as many
/// files unchanged to `n1` as to `n2`, whether or not `n1` was counted first, because its hash map of these
/// directories visits `n2` first.
#[test]
fn equally_common_directories_are_suggested_in_the_order_of_gits_hash_map() -> TestResult {
    assert_eq!(
        strmap::iteration_order(["n1", "n2"]),
        [1, 0],
        "git's hash map visits `n2` before `n1`"
    );
    let (renames, _) = Side::default()
        .delete("old/a", &text("a", 0..20), IRRELEVANT)
        .delete("old/b", &text("b", 0..20), IRRELEVANT)
        .delete("old/x", &text("x", 0..20), CONTENT)
        .add("n1/a", &text("a", 0..20))
        .add("n1/x", &text("x", 1..21))
        .add("n2/b", &text("b", 0..20))
        .add("n2/x", &text("x", 4..24))
        .remove_directory("old", NotRelevant)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 old/a n1/a", "R100 old/b n2/b", "R080 old/x n2/x"],
        "`n1` was counted first, but `n2` is suggested"
    );
    Ok(())
}

#[test]
fn renames_count_for_the_removed_directories_above_them_while_their_names_agree() -> TestResult {
    let all_matter = [("a", ForSelf), ("a/b", ForSelf), ("a/b/c", ForSelf)];
    assert_eq!(
        counts_of("a/b/c/f", "x/b/c/f", &all_matter)?,
        directory_renames(&[("a/b/c", &[("x/b/c", 1)]), ("a/b", &[("x/b", 1)]), ("a", &[("x", 1)])]),
        "each directory above counts while the names of the directories left are the same in both locations"
    );
    assert_eq!(
        counts_of("a/b/c/f", "x/y/c/f", &all_matter)?,
        directory_renames(&[("a/b/c", &[("x/y/c", 1)]), ("a/b", &[("x/y", 1)])]),
        "`a/b` and `x/y` have different names, so `a` doesn't count"
    );
    assert_eq!(
        counts_of(
            "a/b/c/f",
            "x/b/c/f",
            &[("a", ForSelf), ("a/b", ForAncestor), ("a/b/c", ForSelf)]
        )?,
        directory_renames(&[("a/b/c", &[("x/b/c", 1)]), ("a", &[("x", 1)])]),
        "a directory that matters only for those above it doesn't count, but the walk goes on"
    );
    assert_eq!(
        counts_of("a/b/c/f", "x/b/c/f", &[("a/b", ForSelf), ("a/b/c", NotRelevant)])?,
        directory_renames(&[]),
        "a directory that doesn't matter ends the walk, and its own count isn't returned"
    );
    assert_eq!(
        counts_of("a/b/f", "x/b/f", &[("a", ForSelf)])?,
        directory_renames(&[]),
        "a directory that wasn't removed ends the walk before it starts"
    );
    assert_eq!(
        counts_of("a/f", "f", &[("a", ForSelf)])?,
        directory_renames(&[("a", &[("", 1)])]),
        "files can go to the root, whose path is empty"
    );
    Ok(())
}

#[test]
fn sources_needed_only_for_a_directory_rename_are_left_out_once_its_majority_is_known() -> TestResult {
    let side = |relevance| {
        Side::default()
            .delete("old/1", &text("one", 0..20), LOCATION)
            .delete("old/2", &text("two", 0..20), LOCATION)
            .delete("old/3", &text("three", 0..20), LOCATION)
            .delete("old/u", &text("u", 0..20), relevance)
            .add("new/1", &text("one", 0..20))
            .add("new/2", &text("two", 0..20))
            .add("new/3", &text("three", 0..20))
            .add("new/v", &text("u", 2..22))
            .remove_directory("old", ForSelf)
    };
    let (renames, outcome) = side(LOCATION).renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 old/1 new/1", "R100 old/2 new/2", "R100 old/3 new/3"],
        "three files of `old` went to `new`, which one more file can't outnumber, so `old/u` isn't paired"
    );
    assert_eq!(outcome.directory_renames, directory_renames(&[("old", &[("new", 3)])]));

    let (renames, outcome) = side(CONTENT).renames(Options::default())?;
    assert_eq!(
        renames,
        [
            "R100 old/1 new/1",
            "R100 old/2 new/2",
            "R100 old/3 new/3",
            "R090 old/u new/v"
        ],
        "a file whose content matters is paired regardless"
    );
    assert_eq!(outcome.directory_renames, directory_renames(&[("old", &[("new", 4)])]));

    let (renames, outcome) = Side::default()
        .delete("old/1", &text("one", 0..20), LOCATION)
        .delete("old/u", &text("u", 0..20), LOCATION)
        .delete("old/w", &text("w", 0..20), LOCATION)
        .add("new/1", &text("one", 0..20))
        .add("new/v", &text("u", 2..22))
        .add("new/z", &text("w", 2..22))
        .remove_directory("old", ForSelf)
        .renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 old/1 new/1", "R090 old/u new/v", "R090 old/w new/z"],
        "two files of unknown destination could still outnumber the one known, so they are paired"
    );
    assert_eq!(outcome.directory_renames, directory_renames(&[("old", &[("new", 3)])]));
    Ok(())
}

#[test]
fn a_source_still_matters_below_a_directory_whose_rename_is_undecided() -> TestResult {
    let side = || {
        Side::default()
            .delete("top/other/w", &text("w", 0..20), LOCATION)
            .delete("top/sub/1", &text("one", 0..20), LOCATION)
            .delete("top/sub/2", &text("two", 0..20), LOCATION)
            .delete("top/sub/u", &text("u", 0..20), LOCATION)
            .add("x/other/z", &text("w", 2..22))
            .add("x/sub/1", &text("one", 0..20))
            .add("x/sub/2", &text("two", 0..20))
            .add("x/sub/v", &text("u", 2..22))
            .remove_directory("top/other", ForAncestor)
            .remove_directory("top/sub", ForSelf)
    };
    let (renames, outcome) = side().renames(Options::default())?;
    assert_eq!(
        renames,
        ["R100 top/sub/1 x/sub/1", "R100 top/sub/2 x/sub/2"],
        "two of the three files of `top/sub` went to `x/sub`, which the third can't outnumber, and `top/other` matters \
         only for `top`, which wasn't removed"
    );
    assert_eq!(
        outcome.directory_renames,
        directory_renames(&[("top/other", &[]), ("top/sub", &[("x/sub", 2)])]),
        "`top/other` counted only a file whose destination was unknown"
    );

    let (renames, outcome) = side().remove_directory("top", ForSelf).renames(Options::default())?;
    assert_eq!(
        renames,
        [
            "R090 top/other/w x/other/z",
            "R100 top/sub/1 x/sub/1",
            "R100 top/sub/2 x/sub/2",
            "R090 top/sub/u x/sub/v"
        ],
        "only two of the four files of `top` are known to have gone to `x`, so where `top` went is still open and the \
         files below it are paired"
    );
    assert_eq!(
        outcome.directory_renames,
        directory_renames(&[
            ("top", &[("x", 4)]),
            ("top/other", &[("x/other", 1)]),
            ("top/sub", &[("x/sub", 3)])
        ])
    );
    Ok(())
}

#[test]
fn the_limit_bounds_only_the_sources_that_still_matter() -> TestResult {
    let side = || {
        Side::default()
            .delete("a/p", &text("p", 0..20), CONTENT)
            .delete("a/q", &text("q", 0..20), CONTENT)
            .delete("a/r", &text("r", 0..20), IRRELEVANT)
            .add("b/p2", &text("p", 2..22))
            .add("b/q2", &text("q", 2..22))
    };
    let (renames, outcome) = side().renames(Options {
        limit: 1,
        ..Options::default()
    })?;
    assert_eq!(
        (renames.len(), outcome.needed_limit),
        (0, Some(2)),
        "two sources that matter and two destinations need a limit of 2"
    );

    let (renames, outcome) = side().renames(Options {
        limit: 2,
        ..Options::default()
    })?;
    assert_eq!(renames, ["R090 a/p b/p2", "R090 a/q b/q2"]);
    assert_eq!(outcome.needed_limit, None);
    Ok(())
}
