//! Each case is compared with what `git add --all` made of it.

use gix_testtools::TestResult;

use crate::{Case, assert_entries_match_git, assert_matches_git};

#[test]
fn a_new_repository_gets_an_index_with_its_files_except_the_ignored_ones() -> TestResult {
    assert_matches_git(&Case::new("new-repository")?, &[])?;
    Ok(())
}

#[test]
fn modified_and_new_files_are_added_and_deleted_ones_removed() -> TestResult {
    assert_matches_git(&Case::new("changes")?, &[])?;
    Ok(())
}

#[test]
fn a_pathspec_limits_the_changes_to_its_paths() -> TestResult {
    assert_matches_git(&Case::new("pathspec")?, &["dir"])?;
    Ok(())
}

#[test]
fn the_wildcard_of_a_pathspec_matches_slashes() -> TestResult {
    assert_matches_git(&Case::new("pathspec-wildcard")?, &["*.txt"])?;
    Ok(())
}

#[test]
fn a_pathspec_selects_paths_by_their_attributes() -> TestResult {
    assert_matches_git(&Case::new("pathspec-attributes")?, &[":(attr:track)"])?;
    Ok(())
}

#[test]
fn an_ignored_file_that_a_pathspec_names_is_left_out_without_failing() -> TestResult {
    let case = Case::new("pathspec-ignored-file")?;
    let error = case
        .git_error()?
        .ok_or("git fails if a pathspec names an ignored file")?;
    assert!(
        error.starts_with("The following paths are ignored by one of your .gitignore files:\nnamed.log\n"),
        "git reports the ignored file: {error}"
    );
    assert_entries_match_git(&case, &["a", "named.log"])?;
    Ok(())
}

#[test]
fn a_nested_repository_is_added_as_the_commit_of_its_head() -> TestResult {
    assert_matches_git(&Case::new("nested-repository")?, &[])?;
    Ok(())
}

#[test]
fn repositories_in_ignored_directories_are_left_out() -> TestResult {
    assert_matches_git(&Case::new("ignored-repository")?, &[])?;
    Ok(())
}

#[test]
fn the_files_of_a_bare_repository_are_added_like_any_other() -> TestResult {
    assert_matches_git(&Case::new("bare-repository")?, &[])?;
    Ok(())
}

#[test]
fn a_dot_git_file_that_names_no_repository_does_not_make_its_directory_one() -> TestResult {
    assert_matches_git(&Case::new("invalid-dot-git-file")?, &[])?;
    Ok(())
}

#[test]
fn the_file_of_a_conflict_resolves_it() -> TestResult {
    assert_matches_git(&Case::new("conflict-resolved")?, &[])?;
    Ok(())
}

#[test]
fn the_stages_of_a_conflict_whose_file_was_deleted_are_removed() -> TestResult {
    assert_matches_git(&Case::new("conflict-deleted")?, &[])?;
    Ok(())
}

#[test]
fn a_conflict_with_a_deleted_side_is_resolved_with_the_file_that_the_merge_left() -> TestResult {
    assert_matches_git(&Case::new("conflict-modified-deleted")?, &[])?;
    Ok(())
}

#[test]
fn submodules_are_added_as_the_commits_of_their_heads() -> TestResult {
    assert_matches_git(&Case::new("submodules")?, &[])?;
    Ok(())
}

#[test]
fn entries_added_with_intent_to_add_are_added_or_removed() -> TestResult {
    assert_matches_git(&Case::new("intent-to-add")?, &[])?;
    Ok(())
}

#[test]
fn a_directory_that_replaced_a_file_is_added_in_its_place() -> TestResult {
    assert_matches_git(&Case::new("file-to-directory")?, &[])?;
    Ok(())
}

#[test]
fn a_repository_that_replaced_a_file_is_added_as_its_commit() -> TestResult {
    assert_matches_git(&Case::new("file-to-repository")?, &[])?;
    Ok(())
}

#[test]
fn a_file_that_replaced_a_directory_is_added_in_its_place() -> TestResult {
    assert_matches_git(&Case::new("directory-to-file")?, &[])?;
    Ok(())
}

#[test]
fn tracked_files_are_added_although_they_are_ignored() -> TestResult {
    assert_matches_git(&Case::new("tracked-and-ignored")?, &[])?;
    Ok(())
}

#[test]
fn directories_without_files_are_not_added() -> TestResult {
    assert_matches_git(&Case::new("empty-directories")?, &[])?;
    Ok(())
}

#[test]
fn files_are_ignored_by_exclude_files_and_the_gitignore_files_of_their_directories() -> TestResult {
    assert_matches_git(&Case::new("excludes")?, &[])?;
    Ok(())
}

#[test]
fn files_in_ignored_directories_are_not_added() -> TestResult {
    assert_matches_git(&Case::new("ignored-directory")?, &[])?;
    Ok(())
}

#[test]
fn a_file_whose_modification_time_changed_keeps_its_entry() -> TestResult {
    assert_matches_git(&Case::new("modification-time")?, &[])?;
    Ok(())
}

#[test]
fn a_change_in_one_directory_is_added_like_any_other() -> TestResult {
    assert_matches_git(&Case::new("cache-tree")?, &[])?;
    Ok(())
}

#[test]
fn without_symbolic_links_the_file_of_a_symbolic_link_is_added_as_its_target() -> TestResult {
    assert_matches_git(&Case::new("symlink-entry-without-symlinks")?, &[])?;
    Ok(())
}

#[test]
fn without_file_modes_entries_keep_their_modes_and_new_files_are_regular() -> TestResult {
    assert_matches_git(&Case::new("filemode-false")?, &[])?;
    Ok(())
}

#[test]
fn autocrlf_converts_the_line_endings_of_text_files() -> TestResult {
    assert_matches_git(&Case::new("autocrlf")?, &[])?;
    Ok(())
}

#[test]
fn autocrlf_keeps_the_line_endings_of_files_whose_blobs_have_crlf() -> TestResult {
    assert_matches_git(&Case::new("autocrlf-with-crlf-in-index")?, &[])?;
    Ok(())
}

#[test]
fn the_text_attribute_converts_line_endings_without_autocrlf() -> TestResult {
    assert_matches_git(&Case::new("text-auto")?, &[])?;
    Ok(())
}

/// The cases whose results depend on whether the file system of the host has executable bits and ignores the case of
/// file names, which `git init` recorded in their configuration.
mod host {
    use gix_testtools::TestResult;

    use crate::{Case, assert_matches_git};

    const FIXTURE: &str = "make_add_host.sh";

    #[test]
    fn executable_bits_are_added_if_the_file_system_has_them() -> TestResult {
        assert_matches_git(&Case::of(FIXTURE, "executable-bits")?, &[])?;
        Ok(())
    }

    #[test]
    fn a_file_renamed_to_another_case_of_its_name_keeps_its_entry_if_the_file_system_ignores_case() -> TestResult {
        assert_matches_git(&Case::of(FIXTURE, "case-rename")?, &[])?;
        Ok(())
    }
}

/// The cases with symbolic links.
mod symlinks {
    use gix_testtools::TestResult;

    use crate::{Case, assert_matches_git};

    const FIXTURE: &str = "make_add_symlinks.sh";

    #[test]
    #[cfg_attr(
        windows,
        ignore = "creating symbolic links needs a privilege that Windows doesn't grant by default"
    )]
    fn symbolic_links_are_added_with_their_targets() -> TestResult {
        assert_matches_git(&Case::of(FIXTURE, "symlinks")?, &[])?;
        Ok(())
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "creating symbolic links needs a privilege that Windows doesn't grant by default"
    )]
    fn a_symbolic_link_that_replaced_a_file_is_added_in_its_place() -> TestResult {
        assert_matches_git(&Case::of(FIXTURE, "file-to-symlink")?, &[])?;
        Ok(())
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "creating symbolic links needs a privilege that Windows doesn't grant by default"
    )]
    fn a_symbolic_link_that_replaced_a_directory_is_added_in_its_place() -> TestResult {
        assert_matches_git(&Case::of(FIXTURE, "directory-to-symlink")?, &[])?;
        Ok(())
    }
}
