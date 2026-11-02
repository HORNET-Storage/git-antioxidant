//! Patch IDs, as `commit_patch_id()` computes them for `git format-patch --base`, and as `git patch-id --stable`
//! computes them from the diff that `git show` prints.

use std::collections::HashMap;

use bstr::ByteSlice;
use gix_hash::ObjectId;
use gix_rebase::patch_id::{self, Part};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Repo, file, revs, root};

/// The commits whose diff `git patch-id --stable` hashes differently from `diff_get_patch_id()`: after a binary file, it
/// adds the hash of nothing at the end of the diff, and the diff that it reads has no `---` and `+++` lines for an empty
/// new file or for a change of a mode alone.
const TEXT_DIFFERS: [&str; 4] = ["add binary", "change binary", "add empty", "change mode"];

#[test]
fn of_each_commit_as_git_computes_them() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "patches")?;
    let mut commits = repo.commits();
    let mut ids = Vec::new();
    for line in std::fs::read(root.join("patch-ids"))?.lines() {
        let (from_text, commit) = line.split_once_str(" ").ok_or("a patch ID and a commit")?;
        let commit = ObjectId::from_hex(commit)?;
        let subject = commits.get(&commit)?.subject();
        let id = patch_id::of_commit(&mut commits, &commit, Part::Full)?.ok_or("a commit with one parent or none")?;
        if !TEXT_DIFFERS.contains(&subject.to_str()?) {
            assert_eq!(
                id,
                ObjectId::from_hex(from_text)?,
                "{subject}: as `git patch-id --stable` computes it"
            );
        }
        ids.push((subject, id));
    }
    assert_eq!(ids.len(), 19, "each commit of `patches` has its patch ID");

    let prerequisites = std::fs::read(root.join("prerequisite-patch-ids"))?;
    let prerequisites = prerequisites
        .lines()
        .map(ObjectId::from_hex)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(prerequisites.len(), ids.len() - 1, "all commits but the first");
    for ((subject, id), expected) in ids.iter().skip(1).zip(prerequisites) {
        assert_eq!(*id, expected, "{subject}: as `commit_patch_id()` computes it");
    }
    Ok(())
}

#[test]
fn of_a_merge_there_is_none() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "repo")?;
    let mut commits = repo.commits();
    let head = revs(&root, "merges")?["head"];
    let merge = commits.get(&head)?.parents[0];
    assert_eq!(commits.get(&merge)?.parents.len(), 2, "the parent of `r4` is a merge");
    for part in [Part::Headers, Part::Full] {
        assert_eq!(patch_id::of_commit(&mut commits, &merge, part)?, None);
    }
    Ok(())
}

#[test]
fn of_a_cherry_pick_is_that_of_its_original() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "repo")?;
    let mut commits = repo.commits();
    let revs = revs(&root, "linear")?;
    let (on_topic, on_main) = (commits.parents(&revs["head"])?[0], revs["upstream"]);
    let on_topic = commits.parents(&on_topic)?[0];
    assert_eq!(commits.get(&on_topic)?.subject(), "t2");
    assert_eq!(commits.get(&on_main)?.subject(), "u2");
    assert_eq!(
        patch_id::of_commit(&mut commits, &on_topic, Part::Full)?,
        patch_id::of_commit(&mut commits, &on_main, Part::Full)?
    );
    assert!(
        file(&root, "reapply", "todo")?.contains_str("# t2") && !file(&root, "linear", "todo")?.contains_str("# t2"),
        "Git leaves out `t2` unless it reapplies cherry-picks"
    );
    Ok(())
}

/// The patch ID of the headers of the changes of a commit covers the paths and modes of its files alone, as
/// `diff_get_patch_id()` computes it with `diff_header_only`, so commits that change other lines of the same file have
/// equal ones and differ in the patch IDs of their changes. It is computed without reading blobs, unlike the patch ID of
/// the changes.
#[test]
fn of_the_headers_covers_the_paths_and_modes_of_the_files_alone() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "patches")?;
    let mut commits = repo.commits();
    let trees_only = WithoutBlobs(&repo.objects);
    let mut trees_only = gix_rebase::Commits::new(&trees_only, gix_testtools::object_hash());
    let mut by_subject = HashMap::new();
    for line in std::fs::read(root.join("patch-ids"))?.lines() {
        let (_, commit) = line.split_once_str(" ").ok_or("a patch ID and a commit")?;
        let commit = ObjectId::from_hex(commit)?;
        by_subject.insert(commits.get(&commit)?.subject(), commit);
    }
    let mut ids = |subject: &str| -> TestResult<(ObjectId, ObjectId)> {
        let commit = by_subject[subject.as_bytes().as_bstr()];
        let headers = patch_id::of_commit(&mut trees_only, &commit, Part::Headers)?.ok_or("not a merge")?;
        let changes = patch_id::of_commit(&mut commits, &commit, Part::Full)?.ok_or("not a merge")?;
        Ok((headers, changes))
    };

    let lines = [ids("two hunks")?, ids("six lines apart")?, ids("seven lines apart")?];
    assert!(
        lines.iter().all(|(headers, _)| *headers == lines[0].0),
        "the commits change lines of the file `lines` alone"
    );
    assert!(
        lines[0].1 != lines[1].1 && lines[1].1 != lines[2].1 && lines[0].1 != lines[2].1,
        "they change other lines"
    );
    assert_ne!(
        ids("add binary")?.0,
        ids("add with space")?.0,
        "the headers of new files have their paths"
    );

    let two_hunks = by_subject[b"two hunks".as_bstr()];
    assert!(
        patch_id::of_commit(&mut trees_only, &two_hunks, Part::Full).is_err(),
        "the patch ID of the changes reads blobs"
    );
    Ok(())
}

/// The objects of a repository without its blobs.
struct WithoutBlobs<'a>(&'a gix_odb::Handle);

impl gix_object::Find for WithoutBlobs<'_> {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix_error::Result<Option<gix_object::Data<'a>>> {
        Ok(self
            .0
            .try_find(id, buffer)?
            .filter(|data| data.kind != gix_object::Kind::Blob))
    }
}
