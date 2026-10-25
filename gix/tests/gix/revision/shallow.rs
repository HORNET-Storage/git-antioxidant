use gix_testtools::TestResult;

/// `base` has the history A-B-C-D on `main` and B-E-F on `side`, and the other repositories are shallow copies of it
/// whose objects or commit-graph may still record the parents of their shallow commits.
const FIXTURE: &str = "make_shallow_merge_base.sh";

fn ids(path: std::path::PathBuf) -> gix_testtools::Result<Vec<gix::ObjectId>> {
    std::fs::read_to_string(path)?
        .lines()
        .map(|line| Ok(gix::ObjectId::from_hex(line.as_bytes())?))
        .collect()
}

#[test]
fn shallow_commits_have_no_parents_as_in_git() -> TestResult {
    for name in ["base", "kept", "graph-only", "depth-3"] {
        let repo = crate::util::named_subrepo_opts(FIXTURE, name, gix::open::Options::isolated())?;
        let workdir = repo.workdir().expect("the repositories have a worktree").to_owned();
        let merge_bases = ids(workdir.join("merge-base.baseline"))?;
        let walk = ids(workdir.join("rev-list.baseline"))?;
        let main = repo.rev_parse_single("main")?.detach();
        let side = repo.rev_parse_single("side")?.detach();

        let found: Vec<_> = repo
            .merge_bases_many(main, &[side])?
            .into_iter()
            .map(gix::Id::detach)
            .collect();
        assert_eq!(
            found, merge_bases,
            "{name}: as `git merge-base --all main side` finds them"
        );
        assert_eq!(
            repo.merge_base(main, side)?.map(gix::Id::detach),
            merge_bases.first().copied(),
            "{name}: the best merge-base"
        );
        assert_eq!(
            repo.merge_base_octopus([main, side])?.map(gix::Id::detach),
            merge_bases.first().copied(),
            "{name}: the octopus merge-base"
        );

        assert_eq!(repo.is_shallow()?, name != "base", "{name}: only `base` is complete");
        assert_eq!(
            repo.commit_graph().is_ok(),
            name != "depth-3",
            "{name}: a commit-graph file exists, which in a shallow repository records the parents of its shallow commits"
        );
        assert_eq!(
            repo.commit_graph_if_enabled()?.is_some(),
            name == "base",
            "{name}: Git reads no commit-graph in a shallow repository"
        );
        if let Ok(cache) = repo.commit_graph() {
            let mut graph = repo.revision_graph(Some(&cache))?;
            let found: Vec<_> = repo
                .merge_bases_many_with_graph(main, &[side], &mut graph)?
                .into_iter()
                .map(gix::Id::detach)
                .collect();
            assert_eq!(found, merge_bases, "{name}: a supplied commit-graph changes nothing");
        }

        let walked = repo
            .rev_walk([main, side])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default()))
            .all()?
            .map(|info| info.map(|info| info.id))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(walked, walk, "{name}: as `git rev-list main side` lists them");

        #[cfg(feature = "merge")]
        {
            let repo = repo.clone().with_object_memory();
            let outcome = repo.merge_commits(
                main,
                side,
                Default::default(),
                gix::merge::commit::Options::from(repo.tree_merge_options()?),
            );
            if merge_bases.is_empty() {
                assert!(
                    outcome.is_err(),
                    "{name}: like Git, the merge refuses histories without a merge-base"
                );
            } else {
                assert_eq!(
                    outcome?.merge_bases.map(|bases| bases.into_iter().collect::<Vec<_>>()),
                    Some(merge_bases),
                    "{name}: the merge uses the merge-base that Git finds"
                );
            }
        }
    }
    Ok(())
}
