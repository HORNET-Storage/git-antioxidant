use gix_index::{
    entry::{Flags, Mode},
    write_tree::Options,
};
use gix_testtools::TestResult;

use super::{Case, assert_matches_git, blob, messages, new_index, no_objects, push};

#[test]
fn valid_nodes_keep_their_trees_without_their_entries_being_read() -> TestResult {
    let case = Case::new("cache-tree")?;
    let index = case.index()?;
    let outcome = assert_matches_git(&case, &index, index.tree(), Options::default())?;
    assert_eq!(
        outcome.written_trees, 4,
        "the trees of the directories that lead to the changed entry are written again, along with the root tree"
    );

    let x_blob_id = index.entry_by_path("x/y".into()).ok_or("the index has `x/y`")?.id;
    let err = index
        .write_tree(None, &case.objects()?, Options::default())
        .expect_err("without the cache tree, the entries of `x` are read, and the blob of `x/y` is missing");
    assert!(err.is_not_found(), "the blob of `x/y` isn't in the repository");
    assert_eq!(messages(err).0, format!("Invalid object 100644 {x_blob_id} for 'x/y'"));
    Ok(())
}

#[test]
fn changing_an_entry_invalidates_the_nodes_of_the_directories_that_lead_to_it_as_git_does() -> TestResult {
    let case = Case::new("cache-tree")?;
    let mut cache_tree = case
        .index_cached()?
        .tree()
        .cloned()
        .ok_or("git computed a cache tree")?;
    cache_tree.invalidate_path("a/b/c/file".into());
    assert_eq!(
        Some(&cache_tree),
        case.index()?.tree(),
        "the nodes of the root, `a`, `a/b` and `a/b/c` are invalid, as git left them after changing `a/b/c/file`"
    );
    Ok(())
}

#[test]
fn replacing_a_directory_with_a_file_removes_the_node_of_the_directory_as_git_does() -> TestResult {
    let case = Case::new("replace-directory")?;
    let mut cache_tree = case
        .index_cached()?
        .tree()
        .cloned()
        .ok_or("git computed a cache tree")?;
    cache_tree.invalidate_path("a/sibling/file".into());
    cache_tree.invalidate_path("a/sibling".into());
    let index = case.index()?;
    assert_eq!(
        Some(&cache_tree),
        index.tree(),
        "the node of `a/sibling` is gone, as git left it after removing `a/sibling/file` and adding `a/sibling`"
    );
    assert_matches_git(&case, &index, Some(&cache_tree), Options::default())?;
    Ok(())
}

#[test]
fn nodes_keep_their_trees_only_if_they_count_the_entries_of_their_directory_and_their_tree_exists() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut index = new_index();
    for path in ["d/x", "d/y"] {
        push(&mut index, path, Mode::FILE, id, Flags::empty());
    }
    let before = index.write_tree(None, &objects, Options::default())?;
    let again = index.write_tree(Some(&before.cache_tree), &objects, Options::default())?;
    assert_eq!(
        (again.tree_id, &again.cache_tree, again.written_trees),
        (before.tree_id, &before.cache_tree, 0),
        "the cache tree of an outcome keeps every tree while the entries stay as they are"
    );

    push(&mut index, "d/z", Mode::FILE, id, Flags::empty());
    let expected = index.write_tree(None, &objects, Options::default())?;
    let with_stale_counts = index.write_tree(Some(&before.cache_tree), &objects, Options::default())?;
    assert_eq!(
        (with_stale_counts.tree_id, &with_stale_counts.cache_tree),
        (expected.tree_id, &expected.cache_tree),
        "`d/z` was added without invalidating its path, but the nodes of the root and `d` count two entries, not three"
    );

    let mut cache_tree = expected.cache_tree.clone();
    cache_tree.id = gix_object::compute_hash(id.kind(), gix_object::Kind::Tree, b"not a tree of the repository")?;
    let with_missing_tree = index.write_tree(Some(&cache_tree), &objects, Options::default())?;
    assert_eq!(
        (with_missing_tree.tree_id, &with_missing_tree.cache_tree),
        (expected.tree_id, &expected.cache_tree),
        "the node of the root names a tree that doesn't exist, so the root tree is written again"
    );
    Ok(())
}
