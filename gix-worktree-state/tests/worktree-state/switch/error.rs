//! Errors, which leave the index and the worktree as they were as long as the worktree didn't change yet, and those of
//! single files, which the outcome of a switch reports once the worktree changed.

use std::sync::atomic::AtomicBool;

use gix_hash::ObjectId;
use gix_index::entry;
use gix_object::{FindExt, Write, tree::EntryKind};
use gix_testtools::TestResult;
use gix_worktree_state::{switch, switch::Context};
use pretty_assertions::assert_eq;

use crate::switch::{Case, Snapshot, switched};

#[test]
fn indices_with_entries_that_are_excluded_from_a_sparse_checkout_are_refused() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let position = index.entry_index_by_path("b".into()).map_err(|_| "b is tracked")?;
    index.entries_mut()[position].flags.insert(entry::Flags::SKIP_WORKTREE);
    let snapshot = Snapshot::take(&case, &index)?;

    let err = case.switch(&mut index)?.expect_err("sparse checkouts aren't supported");
    assert!(err.is_unsupported(), "{err:?}");
    assert_eq!(
        err.error().to_string(),
        "Indices with entries that are excluded from a sparse checkout can't be switched"
    );
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn a_stack_without_exclude_patterns_is_refused() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let (old_tree, new_tree) = case.trees()?;
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
    let err = switch(
        &mut index,
        &case.dir,
        old_tree.as_deref(),
        &new_tree,
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
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let other_hash = if gix_testtools::object_hash() == gix_hash::Kind::Sha1 {
        gix_hash::Kind::Sha256
    } else {
        gix_hash::Kind::Sha1
    };
    let err = case
        .switch_trees(
            &mut index,
            None,
            &ObjectId::empty_tree(other_hash),
            case.objects()?,
            &AtomicBool::new(false),
        )?
        .expect_err("the trees have to use the hash of the index");
    assert!(err.is_validation(), "{err:?}");
    assert_eq!(err.error().to_string(), "The trees have to use the hash of the index");
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn a_tree_with_a_path_that_cant_be_checked_out_is_refused() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let (old_tree, _) = case.trees()?;
    let objects = gix_odb::memory::Proxy::new(case.objects()?, gix_testtools::object_hash());
    let config = objects.write_buf(gix_object::Kind::Blob, b"[core]\n")?;
    let new_tree = objects.write(&gix_object::Tree {
        entries: vec![gix_object::tree::Entry {
            mode: EntryKind::Blob.into(),
            filename: ".git".into(),
            oid: config,
        }],
    })?;
    let err = case
        .switch_trees(
            &mut index,
            old_tree.as_deref(),
            &new_tree,
            objects,
            &AtomicBool::new(false),
        )?
        .expect_err("a file named .git can't be checked out");
    assert!(err.is_validation(), "{err:?}");
    assert_eq!(err.error().to_string(), "The path '.git' can't be checked out");
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn an_interrupted_switch_changes_nothing() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let mut index = case.index()?;
    let snapshot = Snapshot::take(&case, &index)?;

    let (old_tree, new_tree) = case.trees()?;
    let err = case
        .switch_trees(
            &mut index,
            old_tree.as_deref(),
            &new_tree,
            case.objects()?,
            &AtomicBool::new(true),
        )?
        .expect_err("the switch is interrupted");
    assert!(err.is_cancelled(), "{err:?}");
    assert_eq!(err.error().to_string(), "The switch was interrupted");
    snapshot.assert_unchanged(&case, &index)
}

#[test]
fn a_file_whose_blob_is_missing_is_reported_while_its_entry_is_added() -> TestResult {
    let (case, _tmp) = Case::writable("clean")?;
    let (old_tree, new_tree) = case.trees()?;
    let objects = gix_odb::memory::Proxy::new(case.objects()?, gix_testtools::object_hash());
    let mut tree: gix_object::Tree = objects.find_tree(&new_tree, &mut Vec::new())?.into();
    tree.entries.push(gix_object::tree::Entry {
        mode: EntryKind::Blob.into(),
        filename: "missing".into(),
        oid: gix_object::compute_hash(gix_testtools::object_hash(), gix_object::Kind::Blob, b"missing\n")?,
    });
    tree.entries.sort();
    let new_tree = objects.write(&tree)?;

    let mut index = case.index()?;
    let switched = switched(case.switch_trees(
        &mut index,
        old_tree.as_deref(),
        &new_tree,
        objects,
        &AtomicBool::new(false),
    )??)?;
    let failed: Vec<String> = switched
        .checkout
        .errors
        .iter()
        .map(|record| record.path.to_string())
        .collect();
    assert_eq!(
        failed,
        ["missing"],
        "the file whose blob is missing can't be checked out"
    );
    let entry = index.entry_by_path("missing".into()).ok_or("the entry is added")?;
    assert_eq!(
        entry.stat.size, 0,
        "the entry records no stat information, so that its missing file shows up as a change"
    );
    assert!(!case.dir.join("missing").exists(), "the file isn't written");
    Ok(())
}
