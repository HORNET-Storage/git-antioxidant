//! Patch IDs, as `commit_patch_id()` computes them for `git format-patch --base`, and as `git patch-id --stable`
//! computes them from the diff that `git show` prints.

use bstr::ByteSlice;
use gix_hash::ObjectId;
use gix_rebase::patch_id;
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
        let id = patch_id::of_commit(&mut commits, &commit)?.ok_or("a commit with one parent or none")?;
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
    assert_eq!(patch_id::of_commit(&mut commits, &merge)?, None);
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
        patch_id::of_commit(&mut commits, &on_topic)?,
        patch_id::of_commit(&mut commits, &on_main)?
    );
    assert!(
        file(&root, "reapply", "todo")?.contains_str("# t2") && !file(&root, "linear", "todo")?.contains_str("# t2"),
        "Git leaves out `t2` unless it reapplies cherry-picks"
    );
    Ok(())
}
