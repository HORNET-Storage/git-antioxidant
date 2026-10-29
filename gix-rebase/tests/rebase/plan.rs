//! What `git rebase` replays onto what: whether it finds a branch up to date, and the fork point of `--fork-point`.

use bstr::ByteSlice;
use gix_rebase::{fork_point, plan};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Repo, file, id_in, revs, root};

#[test]
fn up_to_date_as_git_finds_it() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "ff")?;
    for (case, keep_base, up_to_date) in [
        ("ff-ahead", false, true),
        ("ff-diverged", false, false),
        ("ff-onto", false, false),
        ("ff-keep-base", true, true),
        ("ff-merged", false, false),
        ("ff-behind", false, false),
    ] {
        let output = file(&root, case, "rebase-output")?;
        assert_eq!(
            output.contains_str("is up to date"),
            up_to_date,
            "{case}: Git printed {output:?}"
        );
        let revs = revs(&root, case)?;
        let mut commits = repo.commits();
        let plan = plan::Plan::new(
            &mut commits,
            revs["head"],
            &plan::Options {
                upstream: Some(revs["upstream"]),
                onto: revs.get("onto").copied(),
                keep_base,
                reapply_cherry_picks: keep_base,
                ..Default::default()
            },
        )?;
        assert_eq!(plan.can_fast_forward(&mut commits)?, up_to_date, "{case}");
    }
    Ok(())
}

#[test]
fn fork_point_as_git_finds_it_in_the_reflog() -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "repo")?;
    let revs = revs(&root, "fork-point")?;
    let name = gix_ref::FullName::try_from("refs/heads/upstream")?;
    let entries = fork_point::reflog_entries(&repo.refs, name.as_ref())?;
    assert_eq!(entries.len(), 4, "created, committed to, reset, committed to");
    let mut commits = repo.commits();
    let expected = id_in(&root, "fork-point", "fork-point")?;
    assert_ne!(expected, revs["upstream"], "the fork point isn't the upstream");
    assert_eq!(
        fork_point::fork_point(&mut commits, &entries, revs["upstream"], revs["head"])?,
        Some(expected)
    );
    assert_eq!(
        fork_point::fork_point(&mut commits, &[], revs["upstream"], revs["head"])?,
        None,
        "without the reflog, only the upstream is a candidate, which isn't the merge base"
    );
    Ok(())
}
