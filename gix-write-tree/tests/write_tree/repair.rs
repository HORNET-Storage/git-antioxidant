use gix_hash::ObjectId;
use gix_index::{
    State,
    entry::{Flags, Mode},
    extension::Tree,
};
use gix_testtools::TestResult;
use gix_write_tree::{Options, invalidate_path, repair_cache_tree, write_tree};
use pretty_assertions::assert_eq;

use crate::{Case, Objects, blob, new_index, no_objects, push, take_written};

/// The nodes of a cache tree by the paths of their directories, with the tree and the number of entries of the valid
/// ones.
type Nodes = Vec<(String, Option<(ObjectId, u32)>)>;

/// The nodes of `cache_tree`, its root first and each node before those of its subdirectories.
fn nodes(cache_tree: &Tree) -> Nodes {
    fn collect(node: &Tree, path: String, out: &mut Nodes) {
        out.push((path.clone(), node.num_entries.map(|num_entries| (node.id, num_entries))));
        for child in &node.children {
            let name = String::from_utf8_lossy(&child.name);
            let child_path = if path.is_empty() {
                name.into_owned()
            } else {
                format!("{path}/{name}")
            };
            collect(child, child_path, out);
        }
    }
    let mut out = Vec::new();
    collect(cache_tree, String::new(), &mut out);
    out
}

/// An index with a file at each of `paths`, whose blob is written to `objects`.
fn index_with(objects: &Objects, paths: &[&str]) -> TestResult<State> {
    let id = blob(objects, "content")?;
    let mut index = new_index();
    for path in paths {
        push(&mut index, path, Mode::FILE, id, Flags::empty());
    }
    index.sort_entries();
    Ok(index)
}

#[test]
fn without_a_cache_tree_the_node_of_every_directory_whose_tree_exists_is_valid() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let index = index_with(&objects, &["a/b/x", "a/y", "c/z", "top"])?;
    let written = write_tree(&index, None, &objects, Options::default())?;
    assert_eq!(
        repair_cache_tree(&index, None, &objects)?,
        written.cache_tree,
        "the trees of all directories were written, so all of their nodes are valid"
    );
    assert_eq!(
        take_written(&objects).len(),
        written.written_trees + 1,
        "only the blob and the trees of `write_tree()` were written, as repairing writes nothing"
    );
    Ok(())
}

#[test]
fn the_nodes_of_directories_whose_trees_dont_exist_are_invalid_and_so_are_those_that_lead_to_them() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let mut index = index_with(&objects, &["a/b/x", "a/y", "c/z"])?;
    let before = write_tree(&index, None, &objects, Options::default())?;
    push(
        &mut index,
        "a/b/new",
        Mode::FILE,
        blob(&objects, "new")?,
        Flags::empty(),
    );
    index.sort_entries();
    let mut cache_tree = before.cache_tree;
    invalidate_path(&mut cache_tree, "a/b/new".into());

    let (elsewhere, _elsewhere_dir) = no_objects()?;
    let mut expected = nodes(&write_tree(&index, None, &elsewhere, Options { missing_ok: true })?.cache_tree);
    for (path, node) in &mut expected {
        if ["", "a", "a/b"].contains(&path.as_str()) {
            *node = None;
        }
    }
    assert_eq!(
        nodes(&repair_cache_tree(&index, Some(&cache_tree), &objects)?),
        expected,
        "the tree of `a/b` with `a/b/new` doesn't exist, so neither do those of `a` and the root, while `c` keeps its tree"
    );
    assert_eq!(
        nodes(&repair_cache_tree(&index, None, &objects)?),
        expected,
        "without a cache tree, every directory is hashed, and the same trees exist"
    );
    Ok(())
}

#[test]
fn valid_nodes_are_kept_without_their_entries_being_hashed() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let index = index_with(&objects, &["c/z", "d/w"])?;
    let written = write_tree(&index, None, &objects, Options::default())?;
    let mut stale = written.cache_tree.clone();
    stale.children[0].id = stale.children[1].id;
    assert_eq!(
        repair_cache_tree(&index, Some(&stale), &objects)?,
        stale,
        "the node of `c` names the tree of `d`, which exists and counts one entry as `c` does, so it is trusted"
    );

    invalidate_path(&mut stale, "c/z".into());
    assert_eq!(
        repair_cache_tree(&index, Some(&stale), &objects)?,
        written.cache_tree,
        "once `c/z` is invalidated, the trees of `c` and the root are hashed again"
    );
    Ok(())
}

#[test]
fn entries_added_with_intent_to_add_or_with_null_ids_make_the_nodes_of_their_directories_invalid() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let mut index = index_with(&objects, &["a/x", "b/x", "c/y"])?;
    write_tree(&index, None, &objects, Options::default())?;
    let object_hash = gix_testtools::object_hash();
    push(
        &mut index,
        "a/new",
        Mode::FILE,
        ObjectId::empty_blob(object_hash),
        Flags::INTENT_TO_ADD,
    );
    push(&mut index, "b/null", Mode::FILE, object_hash.null(), Flags::empty());
    index.sort_entries();
    let validity: Vec<_> = nodes(&repair_cache_tree(&index, None, &objects)?)
        .into_iter()
        .map(|(path, node)| (path, node.is_some()))
        .collect();
    assert_eq!(
        validity,
        [
            (String::new(), false),
            ("a".into(), false),
            ("b".into(), false),
            ("c".into(), true),
        ],
        "no tree lists `a/new`, and `b/null` names no object, while `write_tree()` refuses the null id as corrupt"
    );
    Ok(())
}

#[test]
fn the_cache_tree_of_an_empty_index_is_the_valid_empty_tree() -> TestResult {
    let (objects, _dir) = no_objects()?;
    assert_eq!(
        nodes(&repair_cache_tree(&new_index(), None, &objects)?),
        [(
            String::new(),
            Some((ObjectId::empty_tree(gix_testtools::object_hash()), 0))
        )],
        "git always has the empty tree, even if the object database doesn't store it"
    );
    Ok(())
}

#[test]
fn entries_that_write_tree_refuses_are_refused() -> TestResult {
    let case = Case::new("unmerged")?;
    let index = case.index()?;
    let err = repair_cache_tree(&index, index.tree(), &case.objects()?).expect_err("unmerged entries have no tree");
    assert!(err.is_conflict(), "the conflicts have to be resolved first");

    let (objects, _dir) = no_objects()?;
    let index = index_with(&objects, &["a", "a/x"])?;
    let err = repair_cache_tree(&index, None, &objects).expect_err("`a` can't be a file and a directory");
    assert!(err.is_corrupted(), "the index is corrupt");
    Ok(())
}
