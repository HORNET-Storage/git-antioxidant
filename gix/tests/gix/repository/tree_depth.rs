//! `core.maxTreeDepth` stops the walks through trees where it stops those of Git, which `t6700-tree-depth.sh` shows.

use std::{ops::ControlFlow, path::Path};

use gix::{Repository, Tree, bstr::ByteSlice, sec::Trust, traverse::tree::Recorder};
use gix_testtools::{Result, TestResult};

/// The depth of the deepest tree of the fixture, apart from its tag `big`.
const DEPTH: usize = 5;
/// The limits of the depth to walk the trees of the fixture with, and whether that walk succeeds.
const LIMITS: [(usize, bool); 2] = [(DEPTH, true), (DEPTH - 1, false)];
/// The message of Git and `gix` at a deeper tree.
const TOO_DEEP: &str = "exceeded maximum allowed tree depth";

fn open(dir: &Path, max_depth: usize) -> Result<Repository> {
    let options = gix::open::Options::isolated().config_overrides([
        format!("core.maxTreeDepth={max_depth}"),
        "user.name=gitoxide".into(),
        "user.email=gitoxide@localhost".into(),
    ]);
    Ok(gix::open_opts(dir, options)?)
}

/// Return whether `git` with the arguments `args` succeeds, which only the depth of a tree may keep it from.
fn git_succeeds(dir: &Path, max_depth: usize, args: &str) -> Result<bool> {
    let output = gix_testtools::git_command(dir)
        .arg("-c")
        .arg(format!("core.maxTreeDepth={max_depth}"))
        .args(args.split_whitespace())
        .output()?;
    assert!(
        output.status.success() || output.stderr.contains_str(TOO_DEEP),
        "git {args} fails for the depth of a tree only: {}",
        output.stderr.as_bstr()
    );
    Ok(output.status.success())
}

/// Assert that `walk` succeeded if `expected_success` is set, and failed for the depth of a tree otherwise.
#[track_caller]
fn assert_outcome<T>(outcome: gix::Result<T>, expected_success: bool, walk: &str) {
    match outcome {
        Ok(_) => assert!(expected_success, "{walk} fails at a deeper tree"),
        Err(err) => {
            let message = format!("{err:?}");
            assert!(!expected_success && message.contains(TOO_DEEP), "{walk}: {message}");
        }
    }
}

fn tree<'repo>(repo: &'repo Repository, revision: &str) -> Result<Tree<'repo>> {
    Ok(repo.rev_parse_single(revision)?.object()?.peel_to_tree()?)
}

#[test]
fn reading_trees_stops_where_git_stops() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
    let dir = fixture.path();
    let empty_tree = gix::ObjectId::empty_tree(gix_testtools::object_hash());
    for (max_depth, expected_success) in LIMITS {
        for args in [
            "archive main".to_owned(),
            "ls-tree -r main".to_owned(),
            "rev-list --objects main".to_owned(),
            format!("diff-tree -r {empty_tree} main"),
        ] {
            assert_eq!(
                git_succeeds(dir, max_depth, &args)?,
                expected_success,
                "git {args} with a limit of {max_depth}"
            );
        }

        let repo = open(dir, max_depth)?;
        let tree = tree(&repo, "main")?;
        assert_outcome(
            tree.traverse().breadthfirst(&mut Recorder::default()),
            expected_success,
            "a breadth-first traversal",
        );
        assert_outcome(
            tree.traverse().depthfirst(&mut Recorder::default()),
            expected_success,
            "a depth-first traversal",
        );
        assert_outcome(repo.index_from_tree(&tree.id), expected_success, "an index of a tree");
        let empty = repo.empty_tree();
        assert_outcome(
            repo.diff_tree_to_tree(&empty, &tree, None),
            expected_success,
            "a diff of trees",
        );
        assert_outcome(
            empty
                .changes()?
                .for_each_to_obtain_tree(&tree, |_| Ok(ControlFlow::Continue(()))),
            expected_success,
            "the changes to a tree",
        );
    }
    Ok(())
}

/// Blame compares the trees along the path of the file where Git's blame compares them: a commit with each of its parents
/// up to the first with the same version of the file, entering the directories whose trees differ, and the first commit
/// below contents that aren't committed, entering every directory of the path. A commit without parents isn't compared.
#[test]
#[cfg(feature = "blame")]
fn blame_stops_where_git_stops() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
    let dir = fixture.path();
    let contents = "new\n";
    std::fs::write(dir.join("contents"), contents)?;
    let deep = "a/a/a/a/a/file";
    for (revision, file, with_contents, limits) in [
        ("main", deep, false, LIMITS),
        // `side` has the file of its parent, and another file in the same directory.
        ("side", deep, false, LIMITS),
        ("main", deep, true, LIMITS),
        ("main~1", deep, false, [(DEPTH, true), (DEPTH - 1, true)]),
        // `side` and its parent have the same tree at `b`.
        ("side", "b/file", false, [(1, true), (0, true)]),
        ("main", "b/file", false, [(1, true), (0, false)]),
        ("main", "b/file", true, [(1, true), (0, false)]),
    ] {
        let args = if with_contents {
            format!("blame --contents contents {revision} -- {file}")
        } else {
            format!("blame {revision} -- {file}")
        };
        for (max_depth, expected_success) in limits {
            assert_eq!(
                git_succeeds(dir, max_depth, &args)?,
                expected_success,
                "git {args} with a limit of {max_depth}"
            );
            let repo = open(dir, max_depth)?;
            let suspect = repo.rev_parse_single(revision)?.detach();
            let outcome = if with_contents {
                gix::blame::file(
                    &repo.objects,
                    gix::blame::Start::Contents {
                        first_suspect: suspect,
                        contents: contents.as_bytes().into(),
                    },
                    None,
                    &mut repo.diff_resource_cache_for_tree_diff()?,
                    &mut gix::diff::tree::State::new(repo.max_tree_depth()?),
                    file.into(),
                    Default::default(),
                )
            } else {
                repo.blame_file(file.into(), suspect, Default::default())
            };
            assert_outcome(
                outcome,
                expected_success,
                &format!("{args} with a limit of {max_depth}"),
            );
        }
    }
    Ok(())
}

#[test]
#[cfg(feature = "merge")]
fn merging_stops_where_git_stops() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
    let dir = fixture.path();
    for (max_depth, expected_success) in LIMITS {
        assert_eq!(
            git_succeeds(dir, max_depth, "merge-tree --write-tree main side")?,
            expected_success,
            "git merge-tree with a limit of {max_depth}"
        );
        let repo = open(dir, max_depth)?.with_object_memory();
        let (main, side) = (repo.rev_parse_single("main")?, repo.rev_parse_single("side")?);
        let options = repo.tree_merge_options()?;
        assert_outcome(
            repo.merge_commits(main, side, Default::default(), options.clone().into())
                .map(drop),
            expected_success,
            "a merge of commits",
        );
        assert_outcome(
            repo.merge_trees(
                tree(&repo, "main~1")?.id,
                tree(&repo, "main")?.id,
                tree(&repo, "side")?.id,
                Default::default(),
                options,
            )
            .map(drop),
            expected_success,
            "a merge of trees",
        );
    }
    Ok(())
}

#[test]
#[cfg(feature = "reset")]
fn resetting_stops_where_git_stops() -> TestResult {
    use gix::reset::Mode;

    for (mode, flag) in [(Mode::Mixed, "--mixed"), (Mode::Hard, "--hard"), (Mode::Keep, "--keep")] {
        for (max_depth, expected_success) in LIMITS {
            let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
            // The files of this copy have other metadata than the index knows, which `--keep` refuses until it is refreshed.
            gix_testtools::git(fixture.path(), "update-index -q --refresh")?;
            assert_eq!(
                git_succeeds(fixture.path(), max_depth, &format!("reset {flag} main"))?,
                expected_success,
                "git reset {flag} with a limit of {max_depth}"
            );

            let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
            let repo = open(fixture.path(), max_depth)?;
            let commit = repo.rev_parse_single("main")?.detach();
            assert_outcome(
                repo.reset(Some(commit), mode, Default::default(), &Default::default()),
                expected_success,
                flag,
            );
        }
    }
    Ok(())
}

#[test]
#[cfg(feature = "reset")]
fn restoring_paths_stops_where_git_stops() -> TestResult {
    for (max_depth, expected_success) in LIMITS {
        let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
        assert_eq!(
            git_succeeds(fixture.path(), max_depth, "reset main -- a")?,
            expected_success,
            "git reset <tree-ish> -- <path> with a limit of {max_depth}"
        );

        let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
        let repo = open(fixture.path(), max_depth)?;
        let tree = tree(&repo, "main")?.id;
        assert_outcome(repo.reset_paths(Some(tree), ["a"]), expected_success, "reset of paths");
    }
    Ok(())
}

#[test]
#[cfg(feature = "rebase")]
fn rebasing_stops_where_git_stops() -> TestResult {
    for (max_depth, expected_success) in LIMITS {
        let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
        assert_eq!(
            git_succeeds(fixture.path(), max_depth, "rebase main")?,
            expected_success,
            "git rebase with a limit of {max_depth}"
        );

        let fixture = gix_testtools::scripted_fixture_writable("make_deep_trees.sh")?;
        let repo = open(fixture.path(), max_depth)?;
        let options = gix::rebase::Options {
            upstream: Some("main".into()),
            ..Default::default()
        };
        assert_outcome(repo.rebase(options, &Default::default()), expected_success, "a rebase");
    }
    Ok(())
}

#[test]
fn the_default_limit_stops_at_the_trees_that_git_stops_at() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deep_trees.sh")?;
    let output = gix_testtools::git_command(&dir)
        .args(["ls-tree", "-r", "big"])
        .output()?;
    assert!(
        !output.status.success() && output.stderr.contains_str(TOO_DEEP),
        "{}",
        output.stderr.as_bstr()
    );

    let repo = gix::open_opts(&dir, gix::open::Options::isolated())?;
    assert_eq!(repo.max_tree_depth()?, gix_object::tree::DEFAULT_MAX_DEPTH);
    assert_outcome(
        repo.index_from_tree(&tree(&repo, "big")?.id),
        false,
        "an index of a deep tree",
    );

    let repo = open(&dir, 4100)?;
    let index = repo.index_from_tree(&tree(&repo, "big")?.id)?;
    assert_eq!(index.entries().len(), 1, "a higher limit lets the walk go on");
    Ok(())
}

#[test]
fn a_tree_that_contains_itself_ends_the_walk_at_the_limit() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deep_trees.sh")?;
    let id = std::fs::read_to_string(dir.join("self-containing-tree"))?;
    let id = id.trim();
    assert!(!git_succeeds(&dir, 64, &format!("ls-tree -r {id}"))?);

    let repo = gix::open_opts(&dir, gix::open::Options::isolated())?;
    let tree = repo.find_tree(gix::ObjectId::from_hex(id.as_bytes())?)?;
    assert_outcome(
        tree.traverse().breadthfirst(&mut Recorder::default()),
        false,
        "a breadth-first traversal",
    );
    assert_outcome(
        tree.traverse().depthfirst(&mut Recorder::default()),
        false,
        "a depth-first traversal",
    );
    assert_outcome(repo.index_from_tree(&tree.id), false, "an index of a tree");
    assert_outcome(
        repo.diff_tree_to_tree(&repo.empty_tree(), &tree, None),
        false,
        "a diff of trees",
    );
    Ok(())
}

#[test]
fn only_trusted_configuration_sets_the_limit() -> TestResult {
    let fixture = gix_testtools::scripted_fixture_writable("make_basic_repo.sh")?;
    let config = fixture.path().join(".git").join("config");
    let mut content = std::fs::read(&config)?;
    content.extend_from_slice(b"[core]\n\tmaxTreeDepth = 3\n");
    std::fs::write(&config, content)?;

    let limit = |trust, overrides: &[&str]| -> Result<usize> {
        let options = gix::open::Options::isolated()
            .with(trust)
            .config_overrides(overrides.iter().copied());
        Ok(gix::open_opts(fixture.path(), options)?.max_tree_depth()?)
    };
    assert_eq!(limit(Trust::Full, &[])?, 3, "the repository of the user sets it");
    assert_eq!(
        limit(Trust::Reduced, &[])?,
        gix_object::tree::DEFAULT_MAX_DEPTH,
        "the repository of another owner can't raise it to keep a walk going"
    );
    assert_eq!(
        limit(Trust::Reduced, &["core.maxTreeDepth=4"])?,
        4,
        "the configuration of the caller stays in use"
    );
    assert_eq!(
        limit(Trust::Full, &["core.maxTreeDepth=1k"])?,
        1024,
        "as Git reads integers"
    );
    Ok(())
}

/// Git reads `core.maxTreeDepth` as a signed integer, and a negative one fails every walk at the root tree already, so
/// the limit is an error, with lenient configuration as well.
#[test]
fn a_negative_limit_is_an_error_where_it_stops_every_walk_of_git() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deep_trees.sh")?;
    assert!(
        !git_succeeds(&dir, 0, "ls-tree -r main")?,
        "the first subtree is too deep"
    );
    for args in ["ls-tree main", "archive main"] {
        let output = gix_testtools::git_command(&dir)
            .args(["-c", "core.maxTreeDepth=-1"])
            .args(args.split_whitespace())
            .output()?;
        assert!(
            !output.status.success() && output.stderr.contains_str(TOO_DEEP),
            "git {args}: even the root tree is too deep"
        );
    }

    for strict in [false, true] {
        let options = gix::open::Options::isolated()
            .strict_config(strict)
            .config_overrides(["core.maxTreeDepth=-1"]);
        let repo = gix::open_opts(&dir, options)?;
        assert_outcome(repo.max_tree_depth(), false, "the limit");
        let tree = tree(&repo, "main")?;
        assert_outcome(
            tree.traverse().breadthfirst(&mut Recorder::default()),
            false,
            "a breadth-first traversal",
        );
        assert_outcome(repo.index_from_tree(&tree.id), false, "an index of a tree");
    }
    Ok(())
}
