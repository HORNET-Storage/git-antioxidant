use crate::util::named_repo;
use gix_testtools::TestResult;

#[test]
fn tree_merge_options() -> TestResult {
    let repo = named_repo("make_basic_repo.sh")?;
    let opts: gix::merge::plumbing::tree::Options = repo.tree_merge_options()?.into();
    assert_eq!(
        opts.rewrites,
        Some(gix::diff::Rewrites::default()),
        "If merge options aren't set, it defaults to diff options, and these default to doing rename tracking"
    );
    Ok(())
}

#[test]
fn merges_ignore_big_file_threshold_as_git_does() -> TestResult {
    let repo = named_repo("make_merge_big_file_threshold.sh")?.with_object_memory();
    assert_eq!(
        repo.big_file_threshold()?,
        100,
        "every version of the merged files is larger than this"
    );
    let baseline = std::fs::read_to_string(
        repo.workdir()
            .expect("the fixture has a worktree")
            .join("merge-tree.baseline"),
    )?;
    let mut lines = baseline.lines();
    let tree_id = gix::ObjectId::from_hex(lines.next().expect("the merged tree comes first").as_bytes())?;
    let conflicted: Vec<_> = lines.take_while(|line| !line.is_empty()).collect();

    let mut outcome = repo.merge_commits(
        repo.rev_parse_single("A")?.detach(),
        repo.rev_parse_single("B")?.detach(),
        gix::merge::blob::builtin_driver::text::Labels {
            ancestor: None,
            current: Some("A".into()),
            other: Some("B".into()),
        },
        repo.tree_merge_options()?.into(),
    )?;
    let unresolved: Vec<_> = outcome
        .tree_merge
        .conflicts
        .iter()
        .filter(|conflict| conflict.is_unresolved(gix::merge::tree::TreatAsUnresolved::git()))
        .map(|conflict| conflict.ours.location().to_owned())
        .collect();
    assert_eq!(unresolved, conflicted, "as in Git, only `conflict` conflicts");
    assert_eq!(
        outcome.tree_merge.tree.write()?.detach(),
        tree_id,
        "both files are merged as text, `conflict` with conflict markers, which gives Git's tree"
    );
    Ok(())
}
