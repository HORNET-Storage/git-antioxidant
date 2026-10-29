use crate::util::named_repo;
use gix::merge::tree::DirectoryRenames;
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
    assert_eq!(
        opts.directory_renames,
        DirectoryRenames::Conflict,
        "repository-derived options use Git's conflict default, like the plumbing options"
    );
    Ok(())
}

#[test]
fn tree_merge_options_directory_renames() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_read_only("make_basic_repo.sh")?;
    for renames in ["merge.renames=false", "diff.renames=false"] {
        for directory_renames in ["merge.directoryRenames=bogus", "merge.directoryRenames=true"] {
            let repo = gix::open_opts(
                &fixture,
                crate::util::restricted()
                    .strict_config(true)
                    .config_overrides([renames, directory_renames]),
            )?;
            let opts: gix::merge::plumbing::tree::Options = repo.tree_merge_options()?.into();
            assert_eq!(
                opts.rewrites, None,
                "directory renames do not re-enable rewrite tracking"
            );
            assert_eq!(
                opts.directory_renames,
                DirectoryRenames::Disabled,
                "disabled rewrite tracking ignores even invalid directory rename configuration"
            );
        }
    }
    let repo = gix::open_opts(
        &fixture,
        crate::util::restricted()
            .strict_config(true)
            .config_overrides(["merge.renames=true", "merge.directoryRenames=bogus"]),
    )?;
    assert!(
        repo.tree_merge_options().is_err(),
        "strict configuration still rejects invalid directory rename settings when rewrites are enabled"
    );
    Ok(())
}

#[test]
#[cfg(feature = "revision")]
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

/// Applying the result of merging `topic` into `main` to the index and the worktree, compared with what
/// `git merge --no-commit topic` does.
#[cfg(feature = "reset")]
mod apply {
    use std::sync::atomic::AtomicBool;

    use gix::merge::{
        apply::{Error, Outcome, Refusals},
        blob::builtin_driver::text::Labels,
    };

    use crate::repository::commit::baseline::Scenario;

    /// The files of the Git directory that applying a merge writes, which `gix` has to leave as Git does.
    const STATE: &[&str] = &["HEAD", "AUTO_MERGE"];

    /// Merge `topic` into `HEAD` in the scenario `name` with `gix` and apply the result, and return the scenario with
    /// what applying it did.
    fn merge_topic(name: &str) -> crate::Result<(Scenario, gix::Result<Outcome>)> {
        let scenario = Scenario::new("make_merge_apply_repos.sh", name)?;
        let outcome = {
            let repo = &scenario.repo;
            let mut merge = repo.merge_commits(
                repo.head_id()?.detach(),
                repo.find_reference("topic")?.peel_to_id()?.detach(),
                Labels {
                    ancestor: None,
                    current: Some("HEAD".into()),
                    other: Some("topic".into()),
                },
                repo.tree_merge_options()?.into(),
            )?;
            repo.apply_merge_result(
                repo.head_tree_id()?.detach(),
                &mut merge.tree_merge,
                &AtomicBool::new(false),
            )
        };
        Ok((scenario, outcome))
    }

    /// Merge as `merge_topic()` does, assert that `gix` refused with the messages of Git, left the repository as Git did
    /// and wrote no `AUTO_MERGE`, and return why it refused.
    fn refuses_as_git(name: &str) -> crate::Result<Error> {
        let (scenario, outcome) = merge_topic(name)?;
        let err = outcome.expect_err("Git refused to merge as well");
        // After its strategy failed, `git merge` restores the local changes with `git stash apply`, whose merge writes
        // `AUTO_MERGE`, so only `HEAD` is compared.
        scenario.assert_same_as_git(&["HEAD"])?;
        assert!(
            !scenario.repo.git_dir().join("AUTO_MERGE").exists(),
            "{name}: nothing points to the merge that wasn't applied"
        );
        let refusal = err
            .downcast_any_ref::<Error>()
            .cloned()
            .unwrap_or_else(|| panic!("{name}: a refusal to apply the merge, not {err:?}"));
        let messages = scenario.git_messages()?;
        assert_eq!(
            refusal.to_string().lines().collect::<Vec<_>>(),
            messages
                .iter()
                .map(String::as_str)
                .take_while(|line| !line.starts_with("Merge with strategy"))
                .collect::<Vec<_>>(),
            "{name}: the messages of the checkout of Git"
        );
        Ok(refusal)
    }

    #[test]
    fn clean_merge() -> crate::Result {
        let (scenario, outcome) = merge_topic("clean")?;
        assert!(!outcome?.conflicts);
        scenario.assert_same_as_git(STATE)?;
        Ok(())
    }

    #[test]
    fn conflicts_get_their_stages_and_markers() -> crate::Result {
        let (scenario, outcome) = merge_topic("conflict")?;
        assert!(outcome?.conflicts, "`a` and `c` conflict");
        scenario.assert_same_as_git(STATE)?;
        Ok(())
    }

    #[test]
    fn local_changes_and_untracked_files_out_of_the_way_stay() -> crate::Result {
        let (scenario, outcome) = merge_topic("kept")?;
        outcome?;
        scenario.assert_same_as_git(STATE)?;
        Ok(())
    }

    #[test]
    fn local_changes_in_the_way_are_refused() -> crate::Result {
        assert_eq!(
            refuses_as_git("refused-change")?,
            Error::Refused {
                refusals: Refusals {
                    unstaged_changes: vec!["a".into()],
                    ..Default::default()
                }
            }
        );
        Ok(())
    }

    #[test]
    fn untracked_files_in_the_way_are_refused() -> crate::Result {
        assert_eq!(
            refuses_as_git("refused-untracked")?,
            Error::Refused {
                refusals: Refusals {
                    untracked_overwritten: vec!["t".into()],
                    ..Default::default()
                }
            }
        );
        Ok(())
    }
}
