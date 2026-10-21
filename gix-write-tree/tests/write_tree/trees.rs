use gix_index::entry::{Flags, Mode, Stage};
use gix_object::Exists;
use gix_testtools::TestResult;
use gix_write_tree::{Options, write_tree};
use pretty_assertions::assert_eq;

use crate::{Case, assert_matches_git, blob, new_index, no_objects, push};

#[test]
fn an_empty_index_is_the_empty_tree() -> TestResult {
    let case = Case::new("empty")?;
    let index = case.index()?;
    assert!(index.entries().is_empty(), "the index has no entries");
    let outcome = assert_matches_git(&case, &index, index.tree(), Options::default())?;
    assert!(outcome.tree_id.is_empty_tree(), "the root tree has no entries either");
    assert_eq!(
        outcome.written_trees, 0,
        "the cache tree names the empty tree, which counts as present, so it isn't written"
    );

    let case = Case::new("emptied")?;
    let index = case.index()?;
    assert_eq!(index.tree(), None, "git computed no cache tree for this index");
    let outcome = assert_matches_git(&case, &index, None, Options::default())?;
    assert_eq!(
        outcome.written_trees, 1,
        "without a cache tree that names it, the empty tree is written like any other tree"
    );
    Ok(())
}

#[test]
fn files_links_and_submodules_are_the_entries_of_the_trees_of_their_directories() -> TestResult {
    let case = Case::new("kinds")?;
    let index = case.index()?;
    let submodule = index
        .entry_by_path("sub".into())
        .ok_or("the index has the submodule `sub`")?;
    assert!(
        !case.objects()?.exists(&submodule.id),
        "the commit of the submodule isn't in the repository, so it isn't looked up"
    );
    let outcome = assert_matches_git(&case, &index, index.tree(), Options::default())?;
    assert_eq!(
        outcome.written_trees, 6,
        "the root and each of its five directories have a tree"
    );
    Ok(())
}

#[test]
fn entries_added_with_intent_to_add_are_left_out_and_invalidate_their_directories() -> TestResult {
    let case = Case::new("intent-to-add")?;
    let index = case.index()?;
    let outcome = assert_matches_git(&case, &index, index.tree(), Options::default())?;
    assert_eq!(
        outcome.cache_tree.num_entries, None,
        "the root tree doesn't list all entries of the index, so the node of the root is invalid"
    );
    assert_eq!(
        outcome.written_trees, 3,
        "the trees of the root, `mixed`, and the empty tree of `only` and `only/deeper`, which their parents leave out"
    );
    Ok(())
}

#[test]
fn entries_without_objects_are_accepted_if_objects_may_be_missing() -> TestResult {
    let case = Case::new("missing-ok")?;
    let index = case.index()?;
    assert_matches_git(&case, &index, index.tree(), Options { missing_ok: true })?;
    Ok(())
}

#[test]
fn a_sparse_directory_entry_is_the_tree_whose_id_it_records() -> TestResult {
    let case = Case::new("sparse")?;
    let index = case.index()?;
    assert!(
        index.entries().iter().any(|entry| entry.mode.is_sparse()),
        "the index records the directory outside of the cone of the sparse checkout as one entry"
    );
    let outcome = assert_matches_git(&case, &index, None, Options::default())?;
    assert_eq!(
        outcome.written_trees, 0,
        "the commit of the case has every tree already"
    );
    assert_matches_git(&case, &index, index.tree(), Options::default())?;
    Ok(())
}

#[test]
fn entries_flagged_for_removal_are_ignored() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut flagged = new_index();
    push(&mut flagged, "a", Mode::FILE, id, Flags::empty());
    push(&mut flagged, "a/b", Mode::FILE, id.kind().null(), Flags::REMOVE);
    push(&mut flagged, "d/x", Mode::FILE, id, Flags::empty());
    push(
        &mut flagged,
        "d/y",
        Mode::FILE,
        id,
        Flags::REMOVE | Flags::from_stage(Stage::Ours),
    );
    push(&mut flagged, "e/z", Mode::FILE, id, Flags::REMOVE);
    let mut removed = new_index();
    push(&mut removed, "a", Mode::FILE, id, Flags::empty());
    push(&mut removed, "d/x", Mode::FILE, id, Flags::empty());

    let with_flagged_entries = write_tree(&flagged, None, &objects, Options::default())?;
    let without_them = write_tree(&removed, None, &objects, Options::default())?;
    assert_eq!(
        (with_flagged_entries.tree_id, with_flagged_entries.cache_tree),
        (without_them.tree_id, without_them.cache_tree),
        "flagged entries are neither checked, written nor counted, as they aren't written with the index"
    );
    Ok(())
}
