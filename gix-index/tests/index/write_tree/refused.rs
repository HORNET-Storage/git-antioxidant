use gix_index::{
    entry::{Flags, Mode},
    write_tree::Options,
};
use gix_testtools::TestResult;

use super::{Case, blob, messages, new_index, no_objects, push};

/// `message` as git prints an error, after `error: ` and starting in lowercase.
fn as_git_prints_it(message: &str) -> String {
    let (first, rest) = message
        .split_at_checked(1)
        .expect("messages start with an ASCII letter");
    format!("error: {}{rest}", first.to_lowercase())
}

#[test]
fn unmerged_entries_are_refused_listing_up_to_ten_of_them_as_git_does() -> TestResult {
    let case = Case::new("unmerged")?;
    let index = case.index()?;
    let err = index
        .write_tree(index.tree(), &case.objects()?, Options::default())
        .expect_err("an index with unmerged entries has no tree");
    assert!(err.is_conflict(), "the conflicts have to be resolved first");
    let (message, causes) = messages(err);
    assert_eq!(message, "The index has unmerged entries");
    assert_eq!(
        causes,
        case.git_error_lines()?,
        "ten of the twelve unmerged entries are listed as git lists them, then an ellipsis"
    );
    Ok(())
}

#[test]
fn missing_objects_are_refused_in_the_order_in_which_git_looks_at_the_entries() -> TestResult {
    let case = Case::new("missing")?;
    let index = case.index()?;
    let err = index
        .write_tree(index.tree(), &case.objects()?, Options::default())
        .expect_err("the blobs of two entries are missing");
    assert!(err.is_not_found(), "the object of the entry wasn't found");
    let (message, causes) = messages(err);
    assert_eq!(
        vec![as_git_prints_it(&message)],
        case.git_error_lines()?,
        "`dir/file` is refused although `a-file` comes first, as the entries of subdirectories are looked at first"
    );
    assert!(causes.is_empty(), "the first missing object ends the search");
    Ok(())
}

#[test]
fn files_used_as_directories_are_refused_even_if_other_entries_are_between_them() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut index = new_index();
    for path in ["a", "a-b", "a/c"] {
        push(&mut index, path, Mode::FILE, id, Flags::empty());
    }
    let err = index
        .write_tree(None, &objects, Options::default())
        .expect_err("`a` can't be both a file and a directory in the root tree");
    assert!(err.is_corrupted(), "no index can be both");
    assert_eq!(
        messages(err),
        (
            "Index entries are files at paths that other entries use as directories".to_owned(),
            vec!["You have both a and a/c".to_owned()]
        ),
        "git compares each entry only with the next one, and would write a root tree with two entries named `a`"
    );
    Ok(())
}

#[test]
fn more_than_ten_files_used_as_directories_end_their_list_with_an_ellipsis() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut index = new_index();
    for number in 0..11 {
        push(&mut index, &format!("f{number:02}"), Mode::FILE, id, Flags::empty());
        push(&mut index, &format!("f{number:02}/x"), Mode::FILE, id, Flags::empty());
    }
    let err = index
        .write_tree(None, &objects, Options::default())
        .expect_err("eleven files are used as directories");
    let mut expected: Vec<_> = (0..10)
        .map(|number| format!("You have both f{number:02} and f{number:02}/x"))
        .collect();
    expected.push("...".into());
    assert_eq!(messages(err).1, expected, "ten are listed, as git does");
    Ok(())
}

#[test]
fn entries_that_are_not_sorted_or_share_a_path_are_refused_as_corrupt() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    for (paths, expected) in [
        (["b", "a"], "The index entries aren't sorted as 'b' precedes 'a'"),
        (
            ["a/b", "a.b"],
            "The index entries aren't sorted as 'a/b' precedes 'a.b'",
        ),
        (["a", "a"], "The index has more than one entry at 'a'"),
    ] {
        let mut index = new_index();
        for path in paths {
            push(&mut index, path, Mode::FILE, id, Flags::empty());
        }
        let err = index
            .write_tree(None, &objects, Options::default())
            .expect_err("an index is sorted by path and has one entry per path once it is merged");
        assert!(err.is_corrupted(), "{paths:?}: the index is corrupt");
        assert_eq!(
            messages(err).0,
            expected,
            "{paths:?}: the file `a.b` sorts before the entries in `a`, as trees sort their entries"
        );
    }
    Ok(())
}

#[test]
fn paths_that_cannot_be_the_paths_of_tree_entries_are_refused_as_corrupt() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let tree_id = id.kind().empty_tree();
    for (path, mode, id, expected) in [
        ("", Mode::FILE, id, "The path of the entry '' has an empty component"),
        (
            "/a",
            Mode::FILE,
            id,
            "The path of the entry '/a' has an empty component",
        ),
        (
            "a//b",
            Mode::FILE,
            id,
            "The path of the entry 'a//b' has an empty component",
        ),
        (
            "a\0b",
            Mode::FILE,
            id,
            "The path of the entry 'a\0b' contains a NUL byte",
        ),
        (
            "a/",
            Mode::FILE,
            id,
            "The entry 'a/' ends in a slash but isn't a sparse directory",
        ),
        (
            "a",
            Mode::DIR,
            tree_id,
            "The sparse directory entry 'a' doesn't end in a slash",
        ),
        (
            "a//",
            Mode::DIR,
            tree_id,
            "The path of the entry 'a//' has an empty component",
        ),
    ] {
        let mut index = new_index();
        push(&mut index, path, mode, id, Flags::empty());
        let err = index
            .write_tree(None, &objects, Options::default())
            .expect_err("no tree entry can have this path");
        assert!(err.is_corrupted(), "{path:?}: the index is corrupt");
        assert_eq!(messages(err).0, expected, "{path:?}: the path is refused");
    }
    Ok(())
}

#[test]
fn a_sparse_directory_entry_followed_by_entries_inside_of_it_is_refused_as_corrupt() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut index = new_index();
    push(&mut index, "dir/", Mode::DIR, id.kind().empty_tree(), Flags::empty());
    push(&mut index, "dir/file", Mode::FILE, id, Flags::empty());
    let err = index
        .write_tree(None, &objects, Options::default())
        .expect_err("the entry of the directory is its tree, which can't have more entries");
    assert!(err.is_corrupted(), "the index is corrupt");
    assert_eq!(
        messages(err).0,
        "The sparse directory entry 'dir/' is followed by 'dir/file' inside of it"
    );
    Ok(())
}

#[test]
fn a_null_object_id_is_refused_as_corrupt_even_if_objects_may_be_missing() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let null_id = gix_testtools::object_hash().null();
    let mut index = new_index();
    push(&mut index, "file", Mode::FILE, null_id, Flags::empty());
    for missing_ok in [false, true] {
        let err = index
            .write_tree(None, &objects, Options { missing_ok })
            .expect_err("no object has the null id");
        assert!(err.is_corrupted(), "missing_ok: {missing_ok}: the index is corrupt");
        assert_eq!(
            messages(err).0,
            format!("Invalid object 100644 {null_id} for 'file'"),
            "missing_ok: {missing_ok}: the message is git's"
        );
    }
    Ok(())
}

#[test]
fn an_entry_without_the_mode_of_a_tree_entry_is_refused_as_corrupt() -> TestResult {
    let (objects, _dir) = no_objects()?;
    let id = blob(&objects, "content")?;
    let mut index = new_index();
    push(&mut index, "file", Mode::empty(), id, Flags::empty());
    let err = index
        .write_tree(None, &objects, Options::default())
        .expect_err("a tree entry needs a mode");
    assert!(err.is_corrupted(), "the index is corrupt");
    assert_eq!(messages(err).0, "The entry 'file' has the invalid mode 0");
    Ok(())
}
