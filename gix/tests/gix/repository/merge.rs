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

/// Merging a commit into `HEAD` with `gix`, compared with what `git merge` does, and concluding, aborting and quitting a
/// merge that stopped, compared with `git merge --continue`, `--abort` and `--quit`.
///
/// The hooks that these tests run inherit the environment of the process, which `#[serial]` tests change while they run,
/// so these tests are `#[parallel]` and don't run at the same time as those.
#[cfg(all(feature = "reset", feature = "revision"))]
#[serial_test::parallel]
mod into_head {
    use std::sync::atomic::AtomicBool;

    use gix::merge::into_head::{Error, FastForward, Options, Outcome, Refusals};

    use crate::repository::commit::baseline::Scenario;

    /// Return the files of the Git directory that a merge writes or removes, which `gix` has to leave as Git does, with
    /// the records of the hooks if they run, but without those of `skipped`.
    fn state_without(skipped: &[&str]) -> Vec<&'static str> {
        let mut state = vec![
            "HEAD",
            "ORIG_HEAD",
            "AUTO_MERGE",
            "MERGE_HEAD",
            "MERGE_MSG",
            "MERGE_MODE",
            "MERGE_RR",
            "SQUASH_MSG",
            "COMMIT_EDITMSG",
        ];
        if cfg!(feature = "hooks") {
            state.push("hook-records");
        }
        state.retain(|name| !skipped.contains(name));
        state
    }

    /// Return all files of the Git directory that a merge writes or removes.
    fn state() -> Vec<&'static str> {
        state_without(&[])
    }

    fn scenario(name: &str) -> crate::Result<Scenario> {
        Scenario::new("make_merge_workflow_repos.sh", name)
    }

    /// Merge `merged` into `HEAD` in the scenario `name` with `options`, assert that `gix` left the repository as Git
    /// did, and return what it did.
    fn merges_as_git(name: &str, merged: &str, options: Options) -> crate::Result<Outcome> {
        let scenario = scenario(name)?;
        let outcome = scenario
            .repo
            .merge_into_head(merged, options, &AtomicBool::new(false))?;
        scenario.assert_same_as_git(&state())?;
        Ok(outcome)
    }

    /// Merge as `merges_as_git()` does, assert that `gix` refused with the messages of Git and left the files of `state`
    /// as Git did, and return why it refused.
    fn refuses_as_git(name: &str, merged: &str, options: Options, state: &[&str]) -> crate::Result<Error> {
        let scenario = scenario(name)?;
        let err = scenario
            .repo
            .merge_into_head(merged, options, &AtomicBool::new(false))
            .expect_err("Git refused to merge as well");
        scenario.assert_same_as_git(state)?;
        let refusal = err
            .downcast_any_ref::<Error>()
            .cloned()
            .unwrap_or_else(|| panic!("{name}: a refusal to merge, not {err:?}"));
        let messages = scenario.git_messages()?;
        assert_eq!(
            refusal.to_string().lines().collect::<Vec<_>>(),
            messages.iter().map(String::as_str).collect::<Vec<_>>(),
            "{name}: the messages of Git"
        );
        Ok(refusal)
    }

    /// Return the merge commit of `outcome`.
    fn committed(outcome: &Outcome) -> &gix::commit::index::Outcome {
        match outcome {
            Outcome::Merged {
                commit: Some(commit), ..
            } => commit,
            other => panic!("a merge commit, not {other:?}"),
        }
    }

    #[test]
    fn fast_forward() -> crate::Result {
        let outcome = merges_as_git("fast-forward", "topic", Options::default())?;
        assert!(
            matches!(outcome, Outcome::FastForward { previous: Some(_), .. }),
            "{outcome:?}"
        );
        merges_as_git("ff-detached", "topic", Options::default())?;
        Ok(())
    }

    #[test]
    fn untracked_files_in_the_way_of_a_fast_forward_are_refused() -> crate::Result {
        assert_eq!(
            refuses_as_git("ff-refused", "topic", Options::default(), &state())?,
            Error::FastForwardRefused {
                refusals: Refusals {
                    untracked_overwritten: vec!["topic-file".into()],
                    ..Default::default()
                }
            }
        );
        Ok(())
    }

    #[test]
    fn no_ff_makes_a_merge_commit() -> crate::Result {
        let outcome = merges_as_git(
            "no-ff",
            "topic",
            Options {
                fast_forward: Some(FastForward::Never),
                ..Default::default()
            },
        )?;
        let commit = committed(&outcome);
        assert_eq!(commit.parents.len(), 2, "HEAD stays a parent");
        assert_eq!(commit.message, "Merge branch 'topic'\n");
        let outcome = merges_as_git("ff-config", "topic", Options::default())?;
        assert_eq!(
            committed(&outcome).parents.len(),
            2,
            "`merge.ff=false` doesn't fast-forward either"
        );
        Ok(())
    }

    #[test]
    fn ff_only_refuses_diverged_histories() -> crate::Result {
        assert_eq!(
            refuses_as_git(
                "ff-only",
                "topic",
                Options {
                    fast_forward: Some(FastForward::Only),
                    ..Default::default()
                },
                &state()
            )?,
            Error::NotPossibleToFastForward
        );
        Ok(())
    }

    #[test]
    fn merge_commit_with_the_default_message() -> crate::Result {
        let outcome = merges_as_git("merge", "topic", Options::default())?;
        let commit = committed(&outcome);
        assert_eq!(commit.message, "Merge branch 'topic'\n", "`main` isn't named");
        assert_eq!(commit.parents.len(), 2);
        Ok(())
    }

    #[test]
    fn the_default_message_names_the_branch_merged_into() -> crate::Result {
        let outcome = merges_as_git("into-feature", "topic", Options::default())?;
        assert_eq!(committed(&outcome).message, "Merge branch 'topic' into feature\n");
        let outcome = merges_as_git("merge-detached", "topic", Options::default())?;
        assert_eq!(committed(&outcome).message, "Merge branch 'topic' into HEAD\n");
        let outcome = merges_as_git("suppress-dest", "topic", Options::default())?;
        assert_eq!(
            committed(&outcome).message,
            "Merge branch 'topic' into main\n",
            "an empty `merge.suppressDest` suppresses no branch"
        );
        Ok(())
    }

    #[test]
    fn the_default_message_names_a_commit_by_its_id() -> crate::Result {
        let scenario = scenario("merge-commit-id")?;
        let id = scenario.repo.rev_parse_single("topic")?.to_string();
        let outcome = scenario
            .repo
            .merge_into_head(id.as_str(), Options::default(), &AtomicBool::new(false))?;
        scenario.assert_same_as_git(&state())?;
        assert_eq!(committed(&outcome).message, format!("Merge commit '{id}'\n"));
        Ok(())
    }

    #[test]
    fn the_default_message_of_an_annotated_tag_has_its_message() -> crate::Result {
        let outcome = merges_as_git("merge-tag", "v1", Options::default())?;
        assert_eq!(committed(&outcome).message, "Merge tag 'v1'\n\nversion one\n");
        Ok(())
    }

    #[test]
    fn merge_commit_with_a_message() -> crate::Result {
        let outcome = merges_as_git(
            "message",
            "topic",
            Options {
                message: Some("merged with a message".into()),
                ..Default::default()
            },
        )?;
        assert_eq!(committed(&outcome).message, "merged with a message\n");
        Ok(())
    }

    #[test]
    fn no_commit_stops_before_the_commit() -> crate::Result {
        let outcome = merges_as_git(
            "no-commit",
            "topic",
            Options {
                no_commit: true,
                ..Default::default()
            },
        )?;
        assert!(matches!(outcome, Outcome::Merged { commit: None, .. }), "{outcome:?}");
        merges_as_git(
            "no-commit-no-ff",
            "topic",
            Options {
                no_commit: true,
                fast_forward: Some(FastForward::Never),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    #[test]
    fn squash_prepares_its_message_and_moves_nothing() -> crate::Result {
        let squash = Options {
            squash: true,
            ..Default::default()
        };
        let outcome = merges_as_git("squash", "topic", squash.clone())?;
        assert!(matches!(outcome, Outcome::Merged { commit: None, .. }), "{outcome:?}");
        let outcome = merges_as_git("squash-ff", "topic", squash)?;
        assert!(matches!(outcome, Outcome::FastForward { .. }), "{outcome:?}");
        Ok(())
    }

    #[test]
    fn conflicts_leave_the_merge_in_progress() -> crate::Result {
        let outcome = merges_as_git("conflict", "topic", Options::default())?;
        assert!(
            matches!(&outcome, Outcome::Merged { applied, commit: None } if applied.conflicts),
            "{outcome:?}"
        );
        Ok(())
    }

    #[test]
    fn continue_commits_the_resolved_merge() -> crate::Result {
        let scenario = scenario("continue")?;
        let outcome = scenario.repo.merge_continue()?;
        scenario.assert_same_as_git(&state())?;
        assert_eq!(
            outcome.message, "Merge branch 'topic'\n",
            "the hint about the conflicts goes away with the comments"
        );
        assert_eq!(outcome.parents.len(), 2);
        Ok(())
    }

    #[test]
    fn abort_resets_the_merge() -> crate::Result {
        let scenario = scenario("abort")?;
        scenario.repo.merge_abort(&AtomicBool::new(false))?;
        scenario.assert_same_as_git(&state())?;
        Ok(())
    }

    #[test]
    fn quit_forgets_the_merge() -> crate::Result {
        let scenario = scenario("quit")?;
        scenario.repo.merge_quit()?;
        scenario.assert_same_as_git(&state())?;
        Ok(())
    }

    #[test]
    fn refusals_before_the_merge() -> crate::Result {
        let in_progress = |name: &str, act: &dyn Fn(&gix::Repository) -> gix::Result<()>| -> crate::Result<Error> {
            let scenario = scenario(name)?;
            let err = act(&scenario.repo).expect_err("refused");
            Ok(err
                .downcast_any_ref::<Error>()
                .cloned()
                .unwrap_or_else(|| panic!("{name}: a refusal, not {err:?}")))
        };
        let merge = |repo: &gix::Repository| {
            repo.merge_into_head("topic", Options::default(), &AtomicBool::new(false))
                .map(drop)
        };
        assert_eq!(
            in_progress("abort", &merge)?,
            Error::UnmergedEntries,
            "conflicts come first"
        );
        assert_eq!(in_progress("continue", &merge)?, Error::MergeInProgress);
        assert_eq!(
            in_progress("merge", &|repo| repo.merge_continue().map(drop))?,
            Error::NoMergeInProgress
        );
        assert_eq!(
            in_progress("merge", &|repo| repo.merge_abort(&AtomicBool::new(false)).map(drop))?,
            Error::NoMergeToAbort
        );
        assert_eq!(
            in_progress("merge", &|repo| {
                repo.merge_into_head(
                    "topic",
                    Options {
                        squash: true,
                        fast_forward: Some(FastForward::Never),
                        ..Default::default()
                    },
                    &AtomicBool::new(false),
                )
                .map(drop)
            })?,
            Error::SquashWithNoFastForward
        );
        assert_eq!(
            in_progress("merge", &|repo| {
                repo.merge_into_head("no-such-branch", Options::default(), &AtomicBool::new(false))
                    .map(drop)
            })?,
            Error::NotSomethingWeCanMerge {
                name: "no-such-branch".into()
            }
        );
        Ok(())
    }

    #[test]
    fn local_changes_in_the_way_are_refused() -> crate::Result {
        // After its strategy failed, `git merge` restores the local changes with `git stash apply`, whose merge writes
        // `AUTO_MERGE`, so it isn't compared.
        assert_eq!(
            refuses_as_git(
                "local-changes",
                "topic",
                Options::default(),
                &state_without(&["AUTO_MERGE"])
            )?,
            Error::MergeRefused {
                refusals: Refusals {
                    unstaged_changes: vec!["a".into()],
                    ..Default::default()
                }
            }
        );
        Ok(())
    }

    #[test]
    fn staged_changes_are_refused() -> crate::Result {
        let scenario = scenario("staged-changes")?;
        let err = scenario
            .repo
            .merge_into_head("topic", Options::default(), &AtomicBool::new(false))
            .expect_err("Git refused to merge as well");
        let refusal = err
            .downcast_any_ref::<Error>()
            .cloned()
            .unwrap_or_else(|| panic!("a refusal to merge, not {err:?}"));
        assert_eq!(
            refusal,
            Error::LocalChanges {
                paths: vec!["main-file".into()]
            }
        );
        // `git merge` puts the staged changes back with `git stash apply --index`, which Git for Windows can fail to do
        // in deep directories, so the repository is compared with how it was before, but for `ORIG_HEAD`, which Git sets.
        scenario.assert_same_as(
            &scenario.git.with_file_name("staged-changes"),
            &state_without(&["ORIG_HEAD"]),
        )?;
        assert_eq!(
            std::fs::read(scenario.repo.git_dir().join("ORIG_HEAD"))?,
            std::fs::read(scenario.git.join(".git").join("ORIG_HEAD"))?,
            "ORIG_HEAD"
        );
        let messages = scenario.git_messages()?;
        let lines: Vec<String> = refusal.to_string().lines().map(ToOwned::to_owned).collect();
        assert_eq!(lines[..2], messages[..2], "the refusal of Git");
        assert_eq!(lines.last(), messages.last(), "the last words of Git");
        Ok(())
    }

    #[test]
    fn untracked_files_in_the_way_are_refused() -> crate::Result {
        assert_eq!(
            refuses_as_git("untracked", "topic", Options::default(), &state())?,
            Error::MergeRefused {
                refusals: Refusals {
                    untracked_overwritten: vec!["topic-file".into()],
                    ..Default::default()
                }
            }
        );
        Ok(())
    }

    #[test]
    fn up_to_date() -> crate::Result {
        let outcome = merges_as_git("up-to-date", "topic", Options::default())?;
        assert!(matches!(outcome, Outcome::UpToDate), "{outcome:?}");
        Ok(())
    }

    #[test]
    fn unrelated_histories_are_refused_unless_allowed() -> crate::Result {
        assert_eq!(
            refuses_as_git("unrelated", "other", Options::default(), &state())?,
            Error::UnrelatedHistories
        );
        let outcome = merges_as_git(
            "allow-unrelated",
            "other",
            Options {
                allow_unrelated_histories: true,
                ..Default::default()
            },
        )?;
        assert_eq!(committed(&outcome).message, "Merge branch 'other'\n");
        Ok(())
    }

    #[test]
    #[cfg(feature = "hooks")]
    fn failing_pre_merge_commit_stops_the_commit() -> crate::Result {
        let refusal = refuses_as_git("hook-fails", "topic", Options::default(), &state())?;
        assert_eq!(
            refusal,
            Error::NotCommitted {
                reason: gix::commit::index::Error::HookFailed {
                    event: "pre-merge-commit",
                    code: 1
                }
            }
        );
        Ok(())
    }

    #[test]
    #[cfg(not(feature = "hooks"))]
    fn hooks_do_not_run_without_the_hooks_feature() -> crate::Result {
        let scenario = scenario("hook-fails")?;
        let outcome = scenario
            .repo
            .merge_into_head("topic", Options::default(), &AtomicBool::new(false))?;
        committed(&outcome);
        assert!(
            !scenario.repo.git_dir().join("hook-records").exists(),
            "no hook ran, not even the failing one"
        );
        Ok(())
    }

    #[test]
    fn no_verify_skips_pre_merge_commit_and_commit_msg() -> crate::Result {
        merges_as_git(
            "no-verify",
            "topic",
            Options {
                no_verify: true,
                ..Default::default()
            },
        )?;
        Ok(())
    }

    #[test]
    fn unborn_branch() -> crate::Result {
        let outcome = merges_as_git("unborn", "topic", Options::default())?;
        assert!(
            matches!(outcome, Outcome::FastForward { previous: None, .. }),
            "{outcome:?}"
        );
        Ok(())
    }
}
