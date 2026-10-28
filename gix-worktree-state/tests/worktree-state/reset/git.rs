//! Each case is compared with what `git reset` made of it in the modes that its test names, which also say whether git
//! reset it or refused.

use gix_testtools::TestResult;

use crate::reset::{Reset, assert_matches_git, done, refused};

/// The modes of `git reset` that reset the index to a tree, and the worktree as their names say.
const ALL: [Reset; 4] = [Reset::Hard, Reset::Mixed, Reset::Merge, Reset::Keep];

/// Assert that each of `modes` resets the case `name` as git reset it.
fn reset_as_git(name: &'static str, modes: &[Reset]) -> TestResult {
    for &reset in modes {
        done(assert_matches_git(name, reset)?)?;
    }
    Ok(())
}

/// Assert that each of `modes` refuses to reset the case `name` as git refused it.
fn refused_as_git(name: &'static str, modes: &[Reset]) -> TestResult {
    for &reset in modes {
        refused(assert_matches_git(name, reset)?)?;
    }
    Ok(())
}

#[test]
fn every_kind_of_change_is_made_in_a_clean_worktree() -> TestResult {
    reset_as_git("clean", &ALL)
}

#[test]
fn a_file_with_staged_and_unstaged_changes_that_the_new_tree_changes_is_refused_by_merge_and_keep() -> TestResult {
    reset_as_git("abcd", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("abcd", &[Reset::Merge, Reset::Keep])
}

#[test]
fn a_file_with_staged_and_unstaged_changes_that_the_new_tree_doesnt_change_is_refused_by_merge() -> TestResult {
    reset_as_git("abcc", &[Reset::Hard, Reset::Mixed, Reset::Keep])?;
    refused_as_git("abcc", &[Reset::Merge])
}

#[test]
fn a_staged_change_that_the_new_tree_changes_is_lost_by_merge_and_refused_by_keep() -> TestResult {
    reset_as_git("bbcd", &[Reset::Hard, Reset::Mixed, Reset::Merge])?;
    refused_as_git("bbcd", &[Reset::Keep])
}

#[test]
fn a_staged_change_that_the_new_tree_doesnt_change_is_lost_by_merge_and_kept_in_the_worktree_by_keep() -> TestResult {
    reset_as_git("bbcc", &ALL)
}

#[test]
fn an_unstaged_change_that_the_new_tree_changes_is_refused_by_merge_and_keep() -> TestResult {
    reset_as_git("bccd", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("bccd", &[Reset::Merge, Reset::Keep])
}

#[test]
fn an_unstaged_change_that_the_new_tree_doesnt_change_is_only_lost_by_hard() -> TestResult {
    reset_as_git("bccc", &ALL)
}

#[test]
fn a_conflict_becomes_the_entry_of_the_new_tree_and_is_refused_by_keep() -> TestResult {
    for name in ["xuab", "xuaa"] {
        reset_as_git(name, &[Reset::Hard, Reset::Mixed, Reset::Merge])?;
        refused_as_git(name, &[Reset::Keep])?;
    }
    Ok(())
}

#[test]
fn a_directory_with_untracked_files_at_the_path_of_a_conflict_is_refused_by_merge() -> TestResult {
    reset_as_git("xu-dir", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("xu-dir", &[Reset::Merge, Reset::Keep])
}

#[test]
fn an_untracked_file_at_the_path_of_a_new_file_is_overwritten_by_hard_and_refused_by_merge_and_keep() -> TestResult {
    reset_as_git("untracked", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("untracked", &[Reset::Merge, Reset::Keep])
}

#[test]
fn an_untracked_file_at_the_path_of_a_new_directory_is_replaced_by_hard_and_refused_by_merge_and_keep() -> TestResult {
    reset_as_git("way", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("way", &[Reset::Merge, Reset::Keep])
}

#[test]
fn an_untracked_file_at_the_path_of_a_file_that_only_head_has_is_refused_by_keep() -> TestResult {
    reset_as_git("removed", &[Reset::Hard, Reset::Mixed, Reset::Merge])?;
    refused_as_git("removed", &[Reset::Keep])
}

#[test]
fn a_directory_with_untracked_files_at_the_path_of_a_new_file_is_refused_by_merge_and_keep() -> TestResult {
    reset_as_git("dirs", &[Reset::Hard, Reset::Mixed])?;
    refused_as_git("dirs", &[Reset::Merge, Reset::Keep])
}

#[test]
fn ignored_files_in_the_way_are_overwritten() -> TestResult {
    reset_as_git("ignored", &ALL)
}

#[test]
fn files_that_are_gone_are_not_changes() -> TestResult {
    reset_as_git("gone", &ALL)
}

#[test]
fn local_changes_at_paths_that_the_trees_agree_on_are_only_undone_by_hard() -> TestResult {
    reset_as_git("restore", &ALL)
}

#[test]
fn staged_changes_at_paths_that_the_trees_agree_on_are_lost_by_merge_and_kept_in_the_worktree_by_keep() -> TestResult {
    reset_as_git("staged", &ALL)
}

#[test]
fn the_current_working_directory_is_not_replaced_by_a_file() -> TestResult {
    reset_as_git("cwd", &[Reset::Mixed])?;
    refused_as_git("cwd", &[Reset::Hard, Reset::Merge, Reset::Keep])
}

#[test]
fn the_entries_of_the_paths_of_a_pathspec_are_reset_and_replace_those_in_their_way() -> TestResult {
    reset_as_git("paths", &[Reset::Paths])
}
