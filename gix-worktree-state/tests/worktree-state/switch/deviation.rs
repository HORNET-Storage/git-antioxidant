//! The cases where this crate deviates from git, most of which it refuses where git loses data.

use bstr::ByteSlice;
use gix_testtools::TestResult;
use gix_worktree_state::switch::Refusals;
use pretty_assertions::assert_eq;

use crate::switch::{Case, Item, Snapshot, records, refused, switched, worktree};

/// Assert that a switch of a copy of `case` is refused with `expected`, which leaves the index and the worktree of the
/// copy as they were.
fn assert_refused(case: &Case, expected: &Refusals) -> TestResult {
    let (copy, _tmp) = case.writable_copy()?;
    assert_copy_refused(&copy, expected)
}

/// Assert that a switch of `copy`, a copy of a case that the test may switch, is refused with `expected`, which leaves
/// its index and its worktree as they were.
fn assert_copy_refused(copy: &Case, expected: &Refusals) -> TestResult {
    let mut index = copy.index()?;
    let snapshot = Snapshot::take(copy, &index)?;
    let refusals = refused(copy.switch(&mut index)??)?;
    let name = copy.name;
    assert_eq!(
        &refusals, expected,
        "{name}: the paths whose data git loses are refused"
    );
    snapshot.assert_unchanged(copy, &index)
}

/// Assert that git switched `case`.
fn assert_git_switched(case: &Case) -> TestResult {
    let name = case.name;
    assert_eq!(case.git_error()?, None, "{name}: git switched");
    Ok(())
}

/// Return `true` if the index that git left in `case` has an entry at `path`.
fn git_has_entry(case: &Case, path: &str) -> TestResult<bool> {
    Ok(case.git_index()?.entry_by_path(path.into()).is_some())
}

/// What git left at `path` in the worktree of `case`.
fn git_item(case: &Case, path: &str) -> TestResult<Option<Item>> {
    Ok(worktree(&case.git)?.remove(path.as_bytes().as_bstr()))
}

#[test]
fn a_staged_change_in_a_directory_that_a_file_replaces_is_refused() -> TestResult {
    let case = Case::new("dev-staged")?;
    assert_refused(
        &case,
        &Refusals {
            staged_changes: vec!["d/x".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert!(!git_has_entry(&case, "d/x")?, "git drops the staged change");
    assert_eq!(git_item(&case, "d")?, Some(Item::File("d\n".into(), false)));
    Ok(())
}

#[test]
fn a_staged_new_file_in_a_directory_that_a_file_replaces_is_refused() -> TestResult {
    let case = Case::new("dev-added")?;
    assert_refused(
        &case,
        &Refusals {
            staged_changes: vec!["d/y".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert!(!git_has_entry(&case, "d/y")?, "git drops the new file");
    assert_eq!(git_item(&case, "d")?, Some(Item::File("d\n".into(), false)));
    Ok(())
}

#[test]
fn a_staged_new_file_whose_file_is_gone_at_the_path_of_a_directory_of_the_new_tree_is_refused() -> TestResult {
    let case = Case::new("dev-gone")?;
    assert_refused(
        &case,
        &Refusals {
            staged_changes: vec!["d".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert!(
        !git_has_entry(&case, "d")?,
        "git drops the entry, whose blob only the index had"
    );
    Ok(())
}

#[test]
fn a_staged_new_file_at_the_path_of_a_directory_of_the_new_tree_is_refused_on_any_file_system() -> TestResult {
    // Git refuses it as an untracked file, unless the file system ignores case, where it drops the entry.
    assert_refused(
        &Case::new("dev-file")?,
        &Refusals {
            staged_changes: vec!["d".into()],
            ..Default::default()
        },
    )
}

#[test]
fn a_repository_in_an_ignored_directory_that_a_file_replaces_is_refused() -> TestResult {
    let case = Case::new("dev-repo")?;
    assert_refused(
        &case,
        &Refusals {
            directories_with_untracked_files: vec!["p".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert_eq!(
        git_item(&case, "p")?,
        Some(Item::File("p\n".into(), false)),
        "git replaces the repository with the file"
    );
    Ok(())
}

#[test]
fn the_worktree_of_a_submodule_that_a_file_replaces_is_refused_if_it_has_files() -> TestResult {
    let case = Case::new("dev-sub")?;
    assert_refused(
        &case,
        &Refusals {
            directories_with_untracked_files: vec!["sub".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert_eq!(
        git_item(&case, "sub")?,
        Some(Item::File("file\n".into(), false)),
        "git replaces the worktree of the submodule, along with its repository, with the file"
    );
    Ok(())
}

#[test]
fn a_file_at_the_path_of_a_submodule_whose_commit_changes_is_refused() -> TestResult {
    let case = Case::new("dev-gitlink")?;
    assert_refused(
        &case,
        &Refusals {
            unstaged_changes: vec!["s".into()],
            ..Default::default()
        },
    )?;
    assert_git_switched(&case)?;
    assert_eq!(
        git_item(&case, "s")?,
        Some(Item::Directory),
        "git replaces the file with the directory of the submodule"
    );
    Ok(())
}

#[test]
fn the_current_working_directory_anywhere_in_a_directory_that_a_file_replaces_is_refused() -> TestResult {
    // Git only protects the directory that is the current working directory, so that it removes it here, unless the host
    // can't remove the current directory of a process.
    assert_refused(
        &Case::new("dev-cwd")?,
        &Refusals {
            current_dir: vec!["d".into()],
            ..Default::default()
        },
    )
}

#[test]
fn entries_in_a_directory_at_the_path_of_a_file_that_only_the_old_tree_has_stay() -> TestResult {
    let case = Case::new("dev-old")?;
    let (copy, _tmp) = case.writable_copy()?;
    let before = copy.index()?;
    let files = worktree(&copy.dir)?;
    let mut index = before.clone();
    let switched = switched(copy.switch(&mut index)??)?;
    assert_eq!(
        (switched.added, switched.modified, switched.removed),
        (0, 0, 0),
        "no entry changes"
    );
    assert_eq!(records(&index), records(&before), "the entries stay");
    assert_eq!(worktree(&copy.dir)?, files, "the files stay");

    assert_git_switched(&case)?;
    assert!(!git_has_entry(&case, "p/x")?, "git removes the entry");
    assert_eq!(git_item(&case, "p/x")?, None, "git removes the file");
    Ok(())
}

#[test]
fn files_are_overwritten_in_place_so_that_their_hard_links_see_the_new_content() -> TestResult {
    let (case, tmp) = Case::writable("clean")?;
    let link = tmp.path().join("link-to-a");
    std::fs::hard_link(case.dir.join("a"), &link)?;
    let mut index = case.index()?;
    switched(case.switch(&mut index)??)?;
    assert_eq!(
        std::fs::read(&link)?,
        b"a2\n",
        "git replaces the file instead, which leaves the old content to the link"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn a_named_pipe_in_a_directory_that_a_file_replaces_is_refused() -> TestResult {
    // Git doesn't see named pipes, and removes this one along with the directory.
    let (copy, _tmp) = Case::writable("to-dir")?;
    let status = std::process::Command::new("mkfifo")
        .arg(copy.dir.join("d").join("pipe"))
        .status()?;
    assert!(status.success(), "mkfifo creates the named pipe");
    assert_copy_refused(
        &copy,
        &Refusals {
            directories_with_untracked_files: vec!["d".into()],
            ..Default::default()
        },
    )
}

/// The cases whose results depend on whether the file system of the host ignores the case of file names, which
/// `git init` recorded in their configuration.
mod host {
    use gix_testtools::TestResult;
    use gix_worktree_state::switch::Refusals;
    use pretty_assertions::assert_eq;

    use super::{assert_git_switched, assert_refused, git_has_entry, git_item};
    use crate::switch::{Case, HOST_FIXTURE, Item, assert_matches_git, switched};

    #[test]
    fn a_new_entry_that_only_differs_in_case_from_one_that_stays_is_refused_where_case_is_ignored() -> TestResult {
        let case = Case::of(HOST_FIXTURE, "icase-kept")?;
        if !case.options()?.checkout.fs.ignore_case {
            // Both files can exist, so the new entry is added as git adds it.
            switched(assert_matches_git(&case, &[])?)?;
            return Ok(());
        }
        assert_refused(
            &case,
            &Refusals {
                untracked_overwritten: vec!["A".into()],
                ..Default::default()
            },
        )?;
        assert_git_switched(&case)?;
        assert!(git_has_entry(&case, "a")?, "git keeps the entry that stays");
        assert_eq!(
            (git_item(&case, "a")?, git_item(&case, "A")?),
            (None, Some(Item::File("A\n".into(), false))),
            "git takes the file of `a` for that of `A`, and replaces it with the new file"
        );
        Ok(())
    }
}

/// The cases with symbolic links.
mod symlinks {
    use gix_testtools::TestResult;
    use gix_worktree_state::switch::Refusals;
    use pretty_assertions::assert_eq;

    use super::{assert_git_switched, assert_refused, git_item};
    use crate::switch::{Case, Item, SYMLINKS_FIXTURE};

    #[test]
    #[cfg_attr(
        windows,
        ignore = "creating symbolic links needs a privilege that Windows doesn't grant by default"
    )]
    fn a_changed_file_behind_a_symbolic_link_that_replaced_its_directory_is_refused() -> TestResult {
        let case = Case::of(SYMLINKS_FIXTURE, "link-in-path")?;
        assert_refused(
            &case,
            &Refusals {
                unstaged_changes: vec!["d/x".into()],
                ..Default::default()
            },
        )?;
        // Git compares the file that the link leads to with the entry, and then replaces the link with a directory.
        assert_git_switched(&case)?;
        assert_eq!(
            (git_item(&case, "d")?, git_item(&case, "d/x")?, git_item(&case, "e/x")?),
            (
                Some(Item::Directory),
                Some(Item::File("x2\n".into(), false)),
                Some(Item::File("x\n".into(), false)),
            ),
            "git replaces the link with a directory for the new file, and the old file stays where the link led"
        );
        Ok(())
    }
}
