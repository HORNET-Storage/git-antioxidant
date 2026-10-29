//! Errors, which leave the index and the worktree as they were.

use std::sync::atomic::AtomicBool;

use gix_hash::ObjectId;
use gix_index::entry;
use gix_object::{Write, tree::EntryKind};
use gix_testtools::TestResult;
use gix_worktree_state::{
    reset::{self, Mode},
    switch::Context,
};
use pretty_assertions::assert_eq;

use crate::{
    reset::{Reset, case, reset_index, reset_to},
    switch::{STAT, Snapshot},
};

/// The hash that the objects of the fixtures don't use.
fn other_hash() -> gix_hash::Kind {
    if gix_testtools::object_hash() == gix_hash::Kind::Sha1 {
        gix_hash::Kind::Sha256
    } else {
        gix_hash::Kind::Sha1
    }
}

#[test]
fn indices_with_entries_that_are_excluded_from_a_sparse_checkout_are_refused() -> TestResult {
    for reset in [Reset::Hard, Reset::Mixed, Reset::Merge, Reset::Keep] {
        let (case, _tmp) = case("clean", reset)?.writable_copy()?;
        let mut index = case.index()?;
        let position = index.entry_index_by_path("b".into()).map_err(|_| "b is tracked")?;
        index.entries_mut()[position].flags.insert(entry::Flags::SKIP_WORKTREE);
        let snapshot = Snapshot::take(&case, &index)?;

        let err = reset_index(&case, &mut index, reset, &AtomicBool::new(false))?
            .expect_err("sparse checkouts aren't supported");
        assert!(err.is_unsupported(), "{reset:?}: {err:?}");
        assert_eq!(
            err.error().to_string(),
            "Indices with entries that are excluded from a sparse checkout can't be reset",
            "{reset:?}"
        );
        snapshot.assert_unchanged(&case, &index)?;
    }
    Ok(())
}

#[test]
fn a_stack_without_exclude_patterns_is_refused() -> TestResult {
    let (case, _tmp) = case("clean", Reset::Merge)?.writable_copy()?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let (_, new_tree) = case.trees()?;
    let options = case.options()?;
    let git_dir_realpath = gix_path::realpath(case.git_dir())?;
    let should_interrupt = AtomicBool::new(false);
    let ctx = Context {
        stack: gix_worktree::Stack::from_state_and_ignore_case(
            &case.dir,
            options.checkout.fs.ignore_case,
            gix_worktree::stack::State::AttributesStack(Default::default()),
            &index,
            index.path_backing(),
        ),
        git_dir_realpath: &git_dir_realpath,
        current_dir: &case.dir,
        should_interrupt: &should_interrupt,
    };
    let err = gix_worktree_state::reset(
        &mut index,
        &case.dir,
        &new_tree,
        Mode::Merge,
        case.objects()?,
        ctx,
        options,
    )
    .expect_err("the stack has to provide exclude patterns");
    assert!(err.is_validation(), "{err:?}");
    assert_eq!(
        err.error().to_string(),
        "The stack has to provide both attributes and exclude patterns"
    );
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn trees_that_dont_use_the_hash_of_the_index_are_refused() -> TestResult {
    let (case, _tmp) = case("clean", Reset::Keep)?.writable_copy()?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let (old_tree, new_tree) = case.trees()?;
    let old_tree = old_tree.ok_or("the case has a HEAD")?;
    let other_tree = ObjectId::empty_tree(other_hash());
    for (tree, mode) in [
        (other_tree, Mode::Hard),
        (other_tree, Mode::Keep { head_tree: old_tree }),
        (new_tree, Mode::Keep { head_tree: other_tree }),
    ] {
        let err = reset_to(&case, &mut index, &tree, mode, case.objects()?, &AtomicBool::new(false))?
            .expect_err("the trees have to use the hash of the index");
        assert!(err.is_validation(), "{mode:?}: {err:?}");
        assert_eq!(
            err.error().to_string(),
            "The trees have to use the hash of the index",
            "{mode:?}"
        );
    }
    let err = reset::mixed(
        &mut index,
        &other_tree,
        &case.objects()?,
        |_| true,
        Default::default(),
        STAT,
    )
    .expect_err("the tree has to use the hash of the index");
    assert!(err.is_validation(), "{err:?}");
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn a_tree_with_more_than_one_entry_at_a_path_is_refused() -> TestResult {
    let (case, _tmp) = case("clean", Reset::Hard)?.writable_copy()?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let objects = gix_odb::memory::Proxy::new(case.objects()?, gix_testtools::object_hash());
    let blob = objects.write_buf(gix_object::Kind::Blob, b"a\n")?;
    let subtree = objects.write(&gix_object::Tree {
        entries: vec![gix_object::tree::Entry {
            mode: EntryKind::Blob.into(),
            filename: "x".into(),
            oid: blob,
        }],
    })?;
    let mut tree = gix_object::Tree {
        entries: vec![
            gix_object::tree::Entry {
                mode: EntryKind::Tree.into(),
                filename: "a".into(),
                oid: subtree,
            },
            gix_object::tree::Entry {
                mode: EntryKind::Blob.into(),
                filename: "a".into(),
                oid: blob,
            },
        ],
    };
    tree.entries.sort();
    let tree = objects.write(&tree)?;

    let err = reset::mixed(&mut index, &tree, &objects, |_| true, Default::default(), STAT)
        .expect_err("a path can only have one entry");
    assert!(err.is_corrupted(), "{err:?}");
    assert_eq!(err.error().to_string(), "The tree has more than one entry at 'a'");
    let err = reset_to(&case, &mut index, &tree, Mode::Hard, objects, &AtomicBool::new(false))?
        .expect_err("a path can only have one entry");
    assert!(err.is_corrupted(), "{err:?}");
    assert_eq!(err.error().to_string(), "The tree has more than one entry at 'a'");
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn index_paths_that_cant_be_in_a_worktree_are_refused_before_their_files_are_removed() -> TestResult {
    for reset in [Reset::Hard, Reset::Merge] {
        for path in ["../outside", ".git/config"] {
            let (case, tmp) = case("clean", reset)?.writable_copy()?;
            let outside = tmp.path().join("outside");
            std::fs::write(&outside, "outside\n")?;
            let config = case.git_dir().join("config");
            let config_before = std::fs::read(&config)?;
            let mut index = case.index()?;
            let id = index.entries()[0].id;
            index.dangerously_push_entry(
                entry::Stat::default(),
                id,
                entry::Flags::empty(),
                entry::Mode::FILE,
                path.into(),
            );
            index.sort_entries();
            let snapshot = Snapshot::take(&case, &index)?;

            let err = reset_index(&case, &mut index, reset, &AtomicBool::new(false))?
                .expect_err("the reset would remove a file that isn't in the worktree");
            assert!(err.is_corrupted(), "{reset:?} {path}: {err:?}");
            assert_eq!(
                err.error().to_string(),
                format!("The index has the path '{path}', which can't be in a worktree"),
                "{reset:?}"
            );
            snapshot.assert_unchanged(&case, &index)?;
            assert_eq!(
                std::fs::read(&outside)?,
                b"outside\n",
                "{reset:?} {path}: the file outside the worktree stays"
            );
            assert_eq!(
                std::fs::read(&config)?,
                config_before,
                "{reset:?} {path}: the configuration stays"
            );
        }
    }
    Ok(())
}

#[test]
fn an_interrupted_reset_changes_nothing() -> TestResult {
    for reset in [Reset::Hard, Reset::Merge, Reset::Keep] {
        let (case, _tmp) = case("clean", reset)?.writable_copy()?;
        let mut index = case.index()?;
        let snapshot = Snapshot::take(&case, &index)?;

        let err = reset_index(&case, &mut index, reset, &AtomicBool::new(true))?.expect_err("the reset is interrupted");
        assert!(err.is_cancelled(), "{reset:?}: {err:?}");
        assert_eq!(err.error().to_string(), "The reset was interrupted", "{reset:?}");
        snapshot.assert_unchanged(&case, &index)?;
    }
    Ok(())
}
