//! Each case is compared with what `git switch`, or `git read-tree -m -u` on a branch without commits, made of it.

use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, assert_matches_git, refused};

#[test]
fn every_kind_of_change_is_made_in_a_clean_worktree() -> TestResult {
    assert_matches_git(&Case::new("clean")?, &[])?;
    Ok(())
}

#[test]
fn files_and_directories_replace_each_other() -> TestResult {
    assert_matches_git(&Case::new("to-dir")?, &[])?;
    Ok(())
}

#[test]
fn changes_at_paths_that_the_trees_agree_on_stay_and_so_do_staged_changes_that_the_new_tree_makes() -> TestResult {
    assert_matches_git(&Case::new("kept")?, &[])?;
    Ok(())
}

#[test]
fn ignored_files_in_the_way_are_overwritten_or_removed() -> TestResult {
    assert_matches_git(&Case::new("ignored")?, &[])?;
    Ok(())
}

#[test]
fn local_changes_at_paths_that_the_trees_change_are_refused_with_staged_changes_first() -> TestResult {
    assert_matches_git(&Case::new("local")?, &[])?;
    Ok(())
}

#[test]
fn untracked_files_that_the_switch_would_overwrite_or_remove_are_refused() -> TestResult {
    assert_matches_git(&Case::new("untracked")?, &[])?;
    Ok(())
}

#[test]
fn directories_whose_files_would_be_lost_are_refused() -> TestResult {
    assert_matches_git(&Case::new("dirs")?, &[])?;
    Ok(())
}

#[test]
fn files_that_are_gone_are_not_changes() -> TestResult {
    assert_matches_git(&Case::new("gone")?, &[])?;
    Ok(())
}

#[test]
fn directories_that_removed_files_leave_empty_are_removed_unless_they_have_untracked_files() -> TestResult {
    assert_matches_git(&Case::new("empty")?, &[])?;
    Ok(())
}

#[test]
fn the_current_working_directory_stays_although_the_switch_leaves_it_empty() -> TestResult {
    assert_matches_git(&Case::new("cwd")?, &[])?;
    Ok(())
}

#[test]
fn the_current_working_directory_is_not_replaced_by_a_file() -> TestResult {
    assert_matches_git(&Case::new("cwd-file")?, &[])?;
    Ok(())
}

#[test]
fn the_current_working_directory_is_not_removed_at_the_path_of_a_file_that_only_the_old_tree_has() -> TestResult {
    assert_matches_git(&Case::new("cwd-old")?, &[])?;
    Ok(())
}

#[test]
fn submodules_keep_their_worktrees_and_a_removed_one_with_files_stays() -> TestResult {
    assert_matches_git(&Case::new("gitlinks")?, &["removed"])?;
    Ok(())
}

#[test]
fn a_conflict_is_refused_before_anything_else_is_checked() -> TestResult {
    assert_matches_git(&Case::new("unmerged")?, &[])?;
    Ok(())
}

#[test]
fn files_are_checked_out_with_the_attributes_of_the_new_tree() -> TestResult {
    assert_matches_git(&Case::new("eol")?, &[])?;
    Ok(())
}

#[test]
fn filters_apply_when_files_are_compared_and_when_they_are_checked_out() -> TestResult {
    assert_matches_git(&Case::new("ident")?, &[])?;
    Ok(())
}

#[test]
fn on_a_branch_without_commits_the_entries_of_the_new_tree_are_added() -> TestResult {
    assert_matches_git(&Case::new("unborn")?, &[])?;
    Ok(())
}

#[test]
fn on_a_branch_without_commits_an_untracked_file_in_the_way_is_refused() -> TestResult {
    assert_matches_git(&Case::new("unborn-way")?, &[])?;
    Ok(())
}

#[test]
fn on_a_branch_without_commits_staged_files_stay_if_the_new_tree_has_the_same_entries_or_none() -> TestResult {
    assert_matches_git(&Case::new("unborn-kept")?, &[])?;
    Ok(())
}

#[test]
fn on_a_branch_without_commits_a_staged_file_that_differs_from_the_new_tree_is_refused() -> TestResult {
    let refusals = refused(assert_matches_git(&Case::new("unborn-staged")?, &[])?)?;
    assert_eq!(
        refusals.staged_changes,
        ["a"],
        "git refuses it as an entry that the merge would overwrite"
    );
    Ok(())
}

/// The cases whose results depend on whether the file system of the host has executable bits and ignores the case of
/// file names, which `git init` recorded in their configuration.
mod host {
    use bstr::ByteSlice;
    use gix_testtools::TestResult;
    use pretty_assertions::assert_eq;

    use crate::{Case, HOST_FIXTURE, Item, assert_matches_git, refused, switched, worktree};

    #[test]
    fn files_are_executable_as_their_new_entries_are_if_the_file_system_has_executable_bits() -> TestResult {
        let case = Case::of(HOST_FIXTURE, "exec")?;
        switched(assert_matches_git(&case, &[])?)?;
        let executable = case.options()?.checkout.fs.executable_bit;
        let files = worktree(&case.git)?;
        assert_eq!(
            (files.get(b"x".as_bstr()), files.get(b"y".as_bstr())),
            (
                Some(&Item::File("x\n".into(), executable)),
                Some(&Item::File("y\n".into(), false)),
            ),
            "git made the file of the executable entry executable, if the file system has executable bits"
        );
        Ok(())
    }

    #[test]
    fn a_changed_executable_bit_is_a_local_change_if_the_file_system_has_executable_bits() -> TestResult {
        let case = Case::of(HOST_FIXTURE, "exec-local")?;
        let outcome = assert_matches_git(&case, &[])?;
        if case.options()?.checkout.fs.executable_bit {
            assert_eq!(
                refused(outcome)?.unstaged_changes,
                ["z"],
                "the file whose executable bit changed is refused"
            );
        } else {
            switched(outcome)?;
        }
        Ok(())
    }

    #[test]
    fn an_entry_replaces_a_removed_one_whose_path_only_differs_in_case_on_any_file_system() -> TestResult {
        switched(assert_matches_git(&Case::of(HOST_FIXTURE, "case-rename")?, &[])?)?;
        Ok(())
    }
}

/// The cases with symbolic links.
mod symlinks {
    use gix_testtools::TestResult;

    use crate::{Case, SYMLINKS_FIXTURE, assert_matches_git, switched};

    #[test]
    #[cfg_attr(
        windows,
        ignore = "creating symbolic links needs a privilege that Windows doesn't grant by default"
    )]
    fn symbolic_links_are_switched_like_files() -> TestResult {
        switched(assert_matches_git(&Case::of(SYMLINKS_FIXTURE, "symlinks")?, &[])?)?;
        Ok(())
    }
}
