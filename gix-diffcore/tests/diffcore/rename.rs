use std::path::Path;

use gix_diff::tree::{Recorder, State, recorder::Change};
use gix_diffcore::rename::{Blobs, Content, Entry, MAX_SCORE, Options, Rename, detect};
use gix_hash::ObjectId;
use gix_object::FindExt;
use gix_odb::HeaderExt;
use gix_testtools::TestResult;

mod merge;

/// The script that makes a repository with an old and a new tree for each case and records what `git diff -M` reports
/// for them.
const FIXTURE: &str = "make_renames.sh";
/// The script that makes a case whose files end in a line without a line feed, which git counts since version 2.44.
const LAST_LINE_FIXTURE: &str = "make_unterminated_last_line.sh";

/// What `git diff -M` reported for the trees of a case.
struct Git {
    /// The changes as `R<similarity> <source> <destination>`, `A <path>`, `D <path>`, `M <path>` or `T <path>`, in the
    /// order of the paths of the destinations.
    lines: Vec<String>,
    /// The limit that git suggested in its warning because `diff.renameLimit` prevented the similarity estimates.
    needed_limit: Option<usize>,
}

/// Find the renames between the trees of `case` that are at least `percentage` similar, with a rename limit of `limit`,
/// assert that they are those that `git diff -M<percentage>%` reported with `diff.renameLimit=<limit>`, and return
/// what git reported.
fn assert_matches_git(case: &str, percentage: u32, limit: usize) -> TestResult<Git> {
    assert_matches_git_at(
        &gix_testtools::scripted_fixture_read_only(FIXTURE)?,
        case,
        percentage,
        limit,
    )
}

/// Like [`assert_matches_git()`], for a `case` among the cases in `fixture`.
fn assert_matches_git_at(fixture: &Path, case: &str, percentage: u32, limit: usize) -> TestResult<Git> {
    let dir = fixture.join(case);
    let objects = gix_odb::at(dir.join(".git").join("objects"), gix_testtools::object_hash())?;
    let [old, new] = tree_ids(&dir)?;
    let (mut old_buf, mut new_buf) = (Vec::new(), Vec::new());
    let mut recorder = Recorder::default();
    gix_diff::tree(
        objects.find_tree_iter(&old, &mut old_buf)?,
        objects.find_tree_iter(&new, &mut new_buf)?,
        State::default(),
        &objects,
        &mut recorder,
    )?;

    // Trees that were deleted or added are passed along as well, to be ignored.
    let (mut sources, mut destinations, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    for change in &recorder.records {
        match change {
            Change::Deletion {
                entry_mode, oid, path, ..
            } => sources.push(Entry {
                location: path.as_ref(),
                mode: *entry_mode,
                id: oid,
            }),
            Change::Addition {
                entry_mode, oid, path, ..
            } => destinations.push(Entry {
                location: path.as_ref(),
                mode: *entry_mode,
                id: oid,
            }),
            Change::Modification {
                previous_entry_mode,
                entry_mode,
                path,
                ..
            } if !entry_mode.is_tree() => {
                let same_type = (previous_entry_mode.is_blob() && entry_mode.is_blob())
                    || previous_entry_mode.kind() == entry_mode.kind();
                lines.push(format!("{} {path}", if same_type { 'M' } else { 'T' }));
            }
            Change::Modification { .. } => {}
        }
    }

    let options = Options {
        min_score: MAX_SCORE * percentage / 100,
        limit,
    };
    let outcome = detect(&sources, &destinations, &mut ObjectBlobs { objects: &objects }, options)?;
    let (mut source_is_paired, mut destination_is_paired) =
        (vec![false; sources.len()], vec![false; destinations.len()]);
    for rename in &outcome.renames {
        source_is_paired[rename.source] = true;
        destination_is_paired[rename.destination] = true;
        lines.push(format!(
            "R{:03} {} {}",
            rename.score * 100 / MAX_SCORE,
            sources[rename.source].location,
            destinations[rename.destination].location
        ));
    }
    for (status, entries, paired) in [
        ('D', &sources, &source_is_paired),
        ('A', &destinations, &destination_is_paired),
    ] {
        lines.extend(
            entries
                .iter()
                .zip(paired)
                .filter(|(entry, is_paired)| !**is_paired && !entry.mode.is_tree())
                .map(|(entry, _)| format!("{status} {}", entry.location)),
        );
    }
    lines.sort_by(|a, b| key_path(a).cmp(key_path(b)));

    let git = git_report(&dir, percentage, limit)?;
    pretty_assertions::assert_eq!(
        lines,
        git.lines,
        "{case}: the files are paired as `git diff -M{percentage}%` pairs them with a rename limit of {limit}"
    );
    assert_eq!(
        outcome.needed_limit, git.needed_limit,
        "{case}: a limit is reported as needed whenever git suggests one"
    );

    let reversed = detect(
        &reversed(&sources),
        &reversed(&destinations),
        &mut ObjectBlobs { objects: &objects },
        options,
    )?;
    let renames: Vec<_> = reversed
        .renames
        .iter()
        .map(|rename| Rename {
            source: sources.len() - 1 - rename.source,
            destination: destinations.len() - 1 - rename.destination,
            score: rename.score,
        })
        .collect();
    assert_eq!(
        (renames, reversed.needed_limit),
        (outcome.renames, outcome.needed_limit),
        "{case}: the order in which the files are passed makes no difference"
    );
    Ok(git)
}

/// What `git diff -M<percentage>%` reported for the trees in `dir` with `diff.renameLimit=<limit>`.
fn git_report(dir: &Path, percentage: u32, limit: usize) -> TestResult<Git> {
    let report = std::fs::read(dir.join(format!("{percentage}-{limit}.expected")))?;
    let mut fields = report
        .split(|&byte| byte == 0)
        .filter(|field| !field.is_empty())
        .map(std::str::from_utf8);
    let mut lines = Vec::new();
    while let Some(status) = fields.next() {
        let status = status?;
        let mut line = status.to_owned();
        for _ in 0..if status.starts_with('R') { 2 } else { 1 } {
            let path = fields
                .next()
                .ok_or("git names the paths of each change after its status")?;
            line.push(' ');
            line.push_str(path?);
        }
        lines.push(line);
    }
    lines.sort_by(|a, b| key_path(a).cmp(key_path(b)));

    let warnings = std::fs::read_to_string(dir.join(format!("{percentage}-{limit}.warnings")))?;
    let needed_limit = match warnings.split_once("to at least ") {
        Some((_, advice)) => Some(advice.split(' ').next().unwrap_or_default().parse()?),
        None => {
            assert_eq!(
                warnings, "",
                "git warns only when the limit prevents the similarity estimates"
            );
            None
        }
    };
    Ok(Git { lines, needed_limit })
}

/// The path of the destination of a change, by which git orders its report.
fn key_path(line: &str) -> &str {
    line.rsplit(' ').next().unwrap_or(line)
}

/// The ids of the old and the new tree of the case in `dir`.
fn tree_ids(dir: &Path) -> TestResult<[ObjectId; 2]> {
    let ids = std::fs::read_to_string(dir.join("trees"))?
        .split_whitespace()
        .map(|hex| ObjectId::from_hex(hex.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let [old, new] = ids.as_slice() else {
        return Err(format!("trees lists {} trees instead of 2", ids.len()).into());
    };
    Ok([*old, *new])
}

/// `entries` in reverse order.
fn reversed<'a>(entries: &[Entry<'a>]) -> Vec<Entry<'a>> {
    entries.iter().rev().copied().collect()
}

/// The blobs of the files of a case, from its object database.
struct ObjectBlobs<'a> {
    objects: &'a gix_odb::Handle,
}

impl Blobs for ObjectBlobs<'_> {
    fn size(&mut self, entry: &Entry<'_>) -> gix_error::Result<u64> {
        Ok(self.objects.header(entry.id)?.size())
    }

    fn content<'buf>(&mut self, entry: &Entry<'_>, buf: &'buf mut Vec<u8>) -> gix_error::Result<(&'buf [u8], Content)> {
        let data = self.objects.find_blob(entry.id, buf)?.data;
        Ok((data, Content::from_data(data)))
    }
}

/// Assert that git reported `line`, which shows the `behavior` that a test is about.
#[track_caller]
fn assert_has(git: &Git, line: &str, behavior: &str) {
    assert!(
        git.lines.iter().any(|reported| reported == line),
        "{behavior}: git reports {line:?} among {:?}",
        git.lines
    );
}

#[test]
fn identical_files_prefer_sources_with_the_file_name_of_the_destination() -> TestResult {
    let git = assert_matches_git("identical", 50, 1000)?;
    assert_has(
        &git,
        "R100 b/one.txt d/one.txt",
        "the source with the file name of the destination is taken although another one comes first",
    );
    assert_has(
        &git,
        "R100 a/other.txt e/else.txt",
        "otherwise the first source that is still unpaired is taken",
    );
    assert_has(&git, "R100 dir/file.txt dir", "a file can take the place of a tree");
    assert_has(
        &git,
        "R100 run.sh run-it.sh",
        "regular files are paired whether or not they are executable",
    );
    Ok(())
}

#[test]
fn links_and_submodules_pair_only_with_their_own_kind() -> TestResult {
    let git = assert_matches_git("links-and-submodules", 50, 1000)?;
    assert_has(
        &git,
        "R100 link-a link-c",
        "a symbolic link pairs with a symbolic link to the same target",
    );
    assert_has(
        &git,
        "A link-d",
        "a regular file with the blob of a symbolic link doesn't pair with it",
    );
    assert_has(&git, "D file-b", "and isn't similar to it either");
    assert_has(
        &git,
        "R100 sub-a sub-b",
        "a submodule pairs with a submodule of the same commit",
    );
    assert_has(&git, "T t", "a file whose type changes isn't renamed");
    Ok(())
}

#[test]
fn empty_files_pair_like_other_identical_files() -> TestResult {
    let git = assert_matches_git("empty", 50, 1000)?;
    assert_has(
        &git,
        "R100 a/empty c/empty",
        "the empty source with the file name of the destination is taken",
    );
    assert_has(&git, "R100 b/zero d/nothing", "otherwise the first empty source");
    assert_has(&git, "D f/empty-too", "an empty file isn't similar to any other file");
    Ok(())
}

#[test]
fn similar_files_are_paired_if_their_similarity_reaches_the_minimum() -> TestResult {
    for (percentage, line, behavior) in [
        (50, "R090 alpha.txt alpha2.txt", "a file that keeps 90% is paired"),
        (50, "R070 beta.txt beta2.txt", "a file that keeps 70% is paired"),
        (50, "D gamma.txt", "a file that keeps nothing isn't"),
        (0, "R070 beta.txt beta2.txt", "a minimum of 0% means 50%"),
        (
            70,
            "R070 beta.txt beta2.txt",
            "a similarity equal to the minimum is enough",
        ),
        (71, "D beta.txt", "a similarity below the minimum isn't"),
        (
            90,
            "R090 alpha.txt alpha2.txt",
            "a similarity equal to the minimum is enough",
        ),
        (91, "D alpha.txt", "a similarity below the minimum isn't"),
        (100, "D alpha.txt", "a minimum of 100% pairs identical files only"),
    ] {
        assert_has(&assert_matches_git("similar", percentage, 1000)?, line, behavior);
    }
    Ok(())
}

#[test]
fn a_unique_file_name_pairs_files_before_more_similar_ones() -> TestResult {
    let git = assert_matches_git("unique-file-name", 50, 1000)?;
    assert_has(
        &git,
        "R080 old/data.txt new/data.txt",
        "the destination with the file name of the source takes it",
    );
    assert_has(
        &git,
        "A new/copy.txt",
        "although another destination is more similar to the source",
    );
    Ok(())
}

#[test]
fn a_unique_file_name_needs_more_similarity_than_the_minimum() -> TestResult {
    let git = assert_matches_git("unique-file-name-too-different", 50, 1000)?;
    assert_has(
        &git,
        "R095 old/info.txt new/other.txt",
        "the most similar destination takes the source",
    );
    assert_has(
        &git,
        "A new/info.txt",
        "as 60% is less than the 75% that the same file name needs at a minimum of 50%",
    );
    Ok(())
}

#[test]
fn file_names_are_unique_among_the_files_left_by_identical_pairs() -> TestResult {
    let git = assert_matches_git("unique-after-exact", 50, 1000)?;
    assert_has(
        &git,
        "R080 b/name.txt d/name.txt",
        "a source that shares its file name only with a source of an identical file has a unique file name",
    );
    assert_has(
        &git,
        "R080 k/list.txt n/list.txt",
        "a destination that shares its file name only with a destination of an identical file has a unique file name",
    );
    Ok(())
}

#[test]
fn shared_file_names_are_left_to_the_estimates() -> TestResult {
    let git = assert_matches_git("shared-file-names", 50, 1000)?;
    for line in ["R090 b/Makefile c/Makefile", "R090 a/Makefile d/Makefile"] {
        assert_has(
            &git,
            line,
            "files whose file name others share are paired by their similarity",
        );
    }
    Ok(())
}

#[test]
fn each_destination_keeps_its_four_most_similar_sources() -> TestResult {
    let git = assert_matches_git("four-candidates", 50, 1000)?;
    assert_has(&git, "R097 s1.txt d2.txt", "the most similar pairs are taken first");
    assert_has(
        &git,
        "A d1.txt",
        "a destination whose four candidates were taken remains added",
    );
    assert_has(&git, "D s5.txt", "although a fifth source is similar enough to it");
    Ok(())
}

#[test]
fn equally_similar_sources_prefer_the_file_name_then_the_first_location() -> TestResult {
    let git = assert_matches_git("ties", 50, 1000)?;
    assert_has(
        &git,
        "R060 b/file.txt d/file.txt",
        "of equally similar sources, the one with the file name of the destination is taken",
    );
    assert_has(&git, "R060 e/one.txt g/three.txt", "otherwise the first of them");
    Ok(())
}

#[test]
fn a_candidate_replaces_the_first_of_the_worst_ones() -> TestResult {
    let git = assert_matches_git("slot-order", 50, 1000)?;
    assert_has(
        &git,
        "R060 e.txt dst.txt",
        "the last source takes the place of the least similar one, which comes first",
    );
    Ok(())
}

#[test]
fn carriage_returns_count_only_in_binary_data() -> TestResult {
    let git = assert_matches_git("binary", 50, 1000)?;
    assert_has(
        &git,
        "R095 text.txt text-crlf.txt",
        "carriage returns before line feeds don't count in text",
    );
    assert_has(&git, "D data.bin", "but they do in binary data");
    assert_has(
        &git,
        "R088 image.bin image-edited.bin",
        "data without line feeds before its end is compared in spans of 64 bytes",
    );
    Ok(())
}

#[test]
fn the_last_line_counts_without_a_line_feed() -> TestResult {
    // Git ignores such a line before version 2.44, so then only an archive that a later version made provides the
    // fixture.
    let Some(fixture) =
        gix_testtools::scripted_fixture_read_only_with_git_version(LAST_LINE_FIXTURE, |version| version >= (2, 44, 0))?
    else {
        return Ok(());
    };
    let git = assert_matches_git_at(&fixture, "unterminated-last-line", 50, 1000)?;
    assert_has(
        &git,
        "R066 old.txt new.txt",
        "the last 40 bytes, which no line feed ends, are a span that both files share",
    );
    Ok(())
}

#[test]
fn estimates_are_skipped_beyond_the_limit() -> TestResult {
    let git = assert_matches_git("limit", 50, 1)?;
    assert_eq!(
        git.needed_limit,
        Some(2),
        "two sources and destinations remain after identical files and a unique file name"
    );
    assert_has(
        &git,
        "R100 exact-a exact-b",
        "identical files are paired regardless of the limit",
    );
    assert_has(
        &git,
        "R095 same/name.txt other/name.txt",
        "as are files with a unique file name",
    );
    assert_has(&git, "D p.txt", "but the similarity of the others isn't estimated");

    let git = assert_matches_git("limit", 50, 2)?;
    assert_eq!(git.needed_limit, None, "a limit of 2 is enough for them");
    assert_has(&git, "R090 p.txt p2.txt", "so their similarity is estimated");
    Ok(())
}

#[test]
fn the_limit_bounds_the_product_of_sources_and_destinations() -> TestResult {
    let git = assert_matches_git("limit-product", 50, 1)?;
    assert_eq!(
        git.needed_limit,
        Some(4),
        "git suggests the larger of the numbers of sources and destinations"
    );
    assert_has(
        &git,
        "D r.txt",
        "1 source and 4 destinations are more than a limit of 1 allows",
    );
    for limit in [2, 0] {
        let git = assert_matches_git("limit-product", 50, limit)?;
        assert_has(
            &git,
            "R090 r.txt r1.txt",
            "1 * 4 isn't more than 2 * 2, and a limit of 0 means no limit",
        );
    }
    Ok(())
}

#[test]
fn only_the_first_hundred_identical_sources_are_compared() -> TestResult {
    let git = assert_matches_git("many-identical", 50, 1000)?;
    assert_has(
        &git,
        "R100 old/f001 new/f101",
        "the first of the compared sources is taken",
    );
    assert_has(
        &git,
        "D old/f101",
        "as the 101st, which has the file name of the destination, isn't compared",
    );
    Ok(())
}

#[test]
fn identical_sources_are_compared_in_reverse_when_gits_table_grows() -> TestResult {
    let git = assert_matches_git("table-growth-205", 50, 1000)?;
    assert_has(
        &git,
        "R100 d2/same.txt d3/same.txt",
        "a table that grows as the last of 205 sources is added returns them in reverse",
    );
    let git = assert_matches_git("table-growth-204", 50, 1000)?;
    assert_has(
        &git,
        "R100 d1/same.txt d3/same.txt",
        "a table of 204 sources returns them in order",
    );
    Ok(())
}

#[test]
fn modified_files_are_no_sources() -> TestResult {
    let git = assert_matches_git("modifications", 50, 1000)?;
    assert_has(
        &git,
        "A copy.txt",
        "a file with the old blob of a modified file is added",
    );
    assert_has(&git, "M kept.txt", "and the modified file remains modified");
    Ok(())
}
