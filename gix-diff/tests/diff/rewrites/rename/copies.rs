//! The fixture records what `git diff -C` reports for the trees of each of its cases, with and without
//! `--find-copies-harder`, which the copies and renames that the finder finds are compared with.

use std::collections::HashSet;

use bstr::{BStr, ByteSlice};
use gix_diff::rewrites::rename::{
    Entry, MAX_SCORE,
    copies::{Options, Pair, Source, SourceKind, detect},
};
use gix_diff::tree::{Recorder, State, recorder::Change};
use gix_object::{FindExt, TreeRefIter};
use gix_testtools::TestResult;

use super::{Git, ObjectBlobs, assert_has, git_report, key_path, tree_ids};

/// The script that makes a repository with an old and a new tree for each case and records what `git diff -C` reports
/// for them.
const FIXTURE: &str = "make_copies.sh";

/// Find the copies and renames between the trees of `case` that are at least `percentage` similar, with a rename limit
/// of `limit`, taking the files that didn't change as sources as well if `find_copies_harder` is set, assert that they
/// are those that `git diff -C<percentage>%` reported with `diff.renameLimit=<limit>`, and with `--find-copies-harder`
/// if it is set, and return what git reported.
fn assert_matches_git(case: &str, find_copies_harder: bool, percentage: u32, limit: usize) -> TestResult<Git> {
    let dir = gix_testtools::scripted_fixture_read_only(FIXTURE)?.join(case);
    let hash_kind = gix_testtools::object_hash();
    let objects = gix_odb::at(dir.join(".git").join("objects"), hash_kind)?;
    let [old, new] = tree_ids(&dir)?;
    let (mut old_buf, mut new_buf) = (Vec::new(), Vec::new());
    let mut changes = Recorder::default();
    gix_diff::tree(
        objects.find_tree_iter(&old, &mut old_buf)?,
        objects.find_tree_iter(&new, &mut new_buf)?,
        State::default(),
        &objects,
        &mut changes,
    )?;
    // The files of the old tree are those that a diff with an empty tree deletes.
    let mut old_files = Recorder::default();
    gix_diff::tree(
        objects.find_tree_iter(&old, &mut old_buf)?,
        TreeRefIter::from_bytes(&[], hash_kind),
        State::default(),
        &objects,
        &mut old_files,
    )?;

    let (mut sources, mut destinations, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    for change in &changes.records {
        match change {
            Change::Deletion {
                entry_mode, oid, path, ..
            } if !entry_mode.is_tree() => sources.push(Source {
                entry: Entry {
                    location: path.as_ref(),
                    mode: *entry_mode,
                    id: oid,
                },
                kind: SourceKind::Deleted,
            }),
            Change::Addition {
                entry_mode, oid, path, ..
            } if !entry_mode.is_tree() => destinations.push(Entry {
                location: path.as_ref(),
                mode: *entry_mode,
                id: oid,
            }),
            // A modified file is a source as it was before the change, and remains modified.
            Change::Modification {
                previous_entry_mode,
                previous_oid,
                entry_mode,
                path,
                ..
            } if !entry_mode.is_tree() => {
                lines.push(format!("M {path}"));
                sources.push(Source {
                    entry: Entry {
                        location: path.as_ref(),
                        mode: *previous_entry_mode,
                        id: previous_oid,
                    },
                    kind: SourceKind::Modified,
                });
            }
            _ => {}
        }
    }
    if find_copies_harder {
        let changed: HashSet<&BStr> = sources.iter().map(|source| source.entry.location).collect();
        for change in &old_files.records {
            if let Change::Deletion {
                entry_mode, oid, path, ..
            } = change
                && !entry_mode.is_tree()
                && !changed.contains(path.as_bstr())
            {
                sources.push(Source {
                    entry: Entry {
                        location: path.as_ref(),
                        mode: *entry_mode,
                        id: oid,
                    },
                    kind: SourceKind::Unchanged,
                });
            }
        }
    }

    let min_score = MAX_SCORE * percentage / 100;
    let options = Options {
        rename_min_score: min_score,
        copy_min_score: min_score,
        limit,
    };
    let outcome = detect(&sources, &destinations, &mut ObjectBlobs { objects: &objects }, options)?;
    let (mut source_is_renamed, mut destination_is_paired) =
        (vec![false; sources.len()], vec![false; destinations.len()]);
    for pair in &outcome.pairs {
        source_is_renamed[pair.source] |= !pair.copy;
        destination_is_paired[pair.destination] = true;
        lines.push(format!(
            "{}{:03} {} {}",
            if pair.copy { 'C' } else { 'R' },
            pair.score * 100 / MAX_SCORE,
            sources[pair.source].entry.location,
            destinations[pair.destination].location
        ));
    }
    // A deleted file that wasn't renamed remains deleted, and a file that was added remains added unless it was paired.
    lines.extend(
        sources
            .iter()
            .zip(&source_is_renamed)
            .filter(|(source, is_renamed)| source.kind == SourceKind::Deleted && !**is_renamed)
            .map(|(source, _)| format!("D {}", source.entry.location)),
    );
    lines.extend(
        destinations
            .iter()
            .zip(&destination_is_paired)
            .filter(|(_, is_paired)| !**is_paired)
            .map(|(destination, _)| format!("A {}", destination.location)),
    );
    lines.sort_by(|a, b| key_path(a).cmp(key_path(b)));

    let report = format!("{}-{percentage}-{limit}", if find_copies_harder { "CC" } else { "C" });
    let git = git_report(&dir, &report)?;
    pretty_assertions::assert_eq!(
        lines,
        git.lines,
        "{case}: the files are paired as `git diff` pairs them for {report}"
    );
    assert_eq!(
        (outcome.needed_limit, outcome.unchanged_sources_left_out),
        (git.needed_limit, git.copies_from_modified_paths_only),
        "{case}: the limit is reported as needed whenever git suggests one, as is leaving out files that didn't change"
    );

    let reversed = detect(
        &sources.iter().rev().copied().collect::<Vec<_>>(),
        &destinations.iter().rev().copied().collect::<Vec<_>>(),
        &mut ObjectBlobs { objects: &objects },
        options,
    )?;
    let pairs: Vec<_> = reversed
        .pairs
        .iter()
        .map(|pair| Pair {
            source: sources.len() - 1 - pair.source,
            destination: destinations.len() - 1 - pair.destination,
            ..*pair
        })
        .collect();
    assert_eq!(
        (pairs, reversed.needed_limit, reversed.unchanged_sources_left_out),
        (outcome.pairs, outcome.needed_limit, outcome.unchanged_sources_left_out),
        "{case}: the order in which the files are passed makes no difference"
    );
    Ok(git)
}

#[test]
fn modified_files_are_copied_as_they_were_before_the_change() -> TestResult {
    let git = assert_matches_git("modified-source", false, 50, 1000)?;
    assert_has(
        &git,
        "C100 kept.txt copy.txt",
        "a file with the old blob of a modified file is its copy",
    );
    assert_has(&git, "C090 kept.txt similar.txt", "as is a file similar to it");
    assert_has(&git, "M kept.txt", "and the modified file remains modified");
    Ok(())
}

#[test]
fn the_last_destination_of_a_deleted_file_is_its_rename() -> TestResult {
    let git = assert_matches_git("deleted-source", false, 50, 1000)?;
    assert_has(&git, "C100 orig.txt a.txt", "the first identical file is a copy");
    assert_has(&git, "C100 orig.txt b.txt", "as is the second one");
    assert_has(
        &git,
        "R090 orig.txt c.txt",
        "while the last destination is the rename, though it is only similar",
    );
    Ok(())
}

#[test]
fn deleted_files_are_paired_before_files_that_remain() -> TestResult {
    let git = assert_matches_git("renames-first", false, 50, 1000)?;
    assert_has(
        &git,
        "R090 gone.txt dst.txt",
        "the deleted file is paired although the modified one is more similar",
    );
    assert_has(&git, "M kept.txt", "which remains modified");
    Ok(())
}

#[test]
fn unchanged_files_are_sources_when_finding_copies_harder() -> TestResult {
    let git = assert_matches_git("unchanged-source", false, 50, 1000)?;
    assert_has(
        &git,
        "A exact.txt",
        "files that didn't change are no sources by default",
    );
    assert_has(&git, "A copy.txt", "neither of identical files nor of similar ones");

    let git = assert_matches_git("unchanged-source", true, 50, 1000)?;
    assert_has(
        &git,
        "C100 same.txt exact.txt",
        "but they are sources of copies when finding copies harder",
    );
    assert_has(&git, "C090 same.txt copy.txt", "of similar files as well");
    Ok(())
}

#[test]
fn the_limit_leaves_out_unchanged_files_if_that_is_enough() -> TestResult {
    let git = assert_matches_git("harder-limit", true, 50, 3)?;
    assert_eq!(
        git.needed_limit, None,
        "a limit of 3 allows comparing 4 sources with 2 destinations"
    );
    assert_has(&git, "C090 u1.txt dst2.txt", "so a file that didn't change is a source");

    let git = assert_matches_git("harder-limit", true, 50, 2)?;
    assert_eq!(git.needed_limit, Some(4), "a limit of 2 doesn't");
    assert!(
        git.copies_from_modified_paths_only,
        "but it does without the files that didn't change"
    );
    assert_has(&git, "C090 mod.txt dst.txt", "so the modified file is still a source");
    assert_has(&git, "A dst2.txt", "while the files that didn't change are left out");

    let git = assert_matches_git("harder-limit", true, 50, 1)?;
    assert!(
        !git.copies_from_modified_paths_only,
        "a limit of 1 is too low even without them"
    );
    assert_has(&git, "A dst.txt", "so no similarity is estimated");
    Ok(())
}

#[test]
fn identical_sources_prefer_unpaired_deleted_files_and_file_names() -> TestResult {
    for find_copies_harder in [false, true] {
        let git = assert_matches_git("identical-sources", find_copies_harder, 50, 1000)?;
        assert_has(
            &git,
            "C100 a/other.txt x/name.txt",
            "an unpaired deleted file counts as much as the file name, and the first of them is taken",
        );
        assert_has(
            &git,
            "R100 a/other.txt y/zzz.txt",
            "the last destination of a deleted file is its rename",
        );
        assert_has(
            &git,
            "C100 c/thing.txt b/thing.txt",
            "the file name counts as much as an unpaired deleted file that comes later",
        );
        assert_has(&git, "D d/else.txt", "which remains deleted");
        assert_has(
            &git,
            "R100 f/ddd.txt g/eee.txt",
            "but an unpaired deleted file counts more than a modified file that comes first",
        );
    }
    Ok(())
}

#[test]
fn the_limit_applies_after_identical_files_are_paired() -> TestResult {
    let git = assert_matches_git("limit", false, 50, 1)?;
    assert_eq!(
        git.needed_limit,
        Some(2),
        "two sources and destinations remain after the identical files"
    );
    assert_has(
        &git,
        "C100 m1.txt exact.txt",
        "identical files are paired regardless of the limit",
    );
    assert_has(&git, "A sim.txt", "but the similarity of the others isn't estimated");

    let git = assert_matches_git("limit", false, 50, 2)?;
    assert_eq!(git.needed_limit, None, "a limit of 2 is enough for them");
    assert_has(&git, "C090 m2.txt sim.txt", "so their similarity is estimated");
    Ok(())
}
