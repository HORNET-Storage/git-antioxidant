use std::{ffi::OsStr, io::ErrorKind};

use gix_fs::{walkdir_new, walkdir_sorted_new, walkdir_sorted_new_follow_links};

#[test]
fn hidden_entries_and_depth_limits() -> gix_testtools::TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("walkdir.sh")?.join("hidden");

    let mut names = walkdir_new(&dir, false)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .map(|entry| entry.map(|entry| entry.file_name().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    assert_eq!(
        names,
        [".hidden", "directory"],
        "hidden entries are included, while the root and descendants beyond max_depth are omitted"
    );
    Ok(())
}

#[test]
fn sorted_walk_uses_git_directory_order() -> gix_testtools::TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("walkdir.sh")?.join("sorted");

    let names = walkdir_sorted_new(&dir, 1, false)
        .min_depth(1)
        .into_iter()
        .map(|entry| entry.map(|entry| entry.file_name().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        names,
        ["common.", "common", "common0"],
        "directories sort as if their names ended in a slash, and the depth limit excludes their children"
    );
    Ok(())
}

#[test]
#[cfg(any(unix, windows))]
fn sorted_walk_following_links_orders_linked_directories_as_directories() -> gix_testtools::TestResult {
    let root = gix_testtools::scripted_fixture_read_only("walkdir_symlinks.sh")?;
    if !gix_testtools::fixture_has_symlinks(&root)? {
        return Ok(());
    }
    let dir = root.join("links");
    let name = |path: &std::path::Path| {
        let relative = path.strip_prefix(&dir).expect("entries are below the walked directory");
        let relative = gix_path::into_bstr(relative).expect("fixture paths are valid UTF-8");
        gix_path::to_unix_separators_on_windows(relative).to_string()
    };
    let walk = |walk: gix_fs::WalkDir| -> Vec<String> {
        walk.min_depth(1)
            .into_iter()
            .map(|entry| match entry {
                Ok(entry) if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) => {
                    format!("{}/", name(&entry.path()))
                }
                Ok(entry) => name(&entry.path()),
                Err(err) => format!(
                    "{} {}",
                    if err.loop_ancestor().is_some() { "loop" } else { "error" },
                    name(err.path().expect("errors of entries keep their path"))
                ),
            })
            .collect()
    };

    assert_eq!(
        walk(walkdir_sorted_new_follow_links(&dir, usize::MAX, false)),
        [
            "common-2",
            "common/",
            "common/sub/",
            "common/sub/file",
            "common0",
            "error dangling",
            "loop loop",
            "target/",
            "target/sub/",
            "target/sub/file"
        ],
        "a link to a directory is walked and ordered as a directory, and links that can't be followed are errors in their place"
    );
    assert_eq!(
        walk(walkdir_sorted_new(&dir, usize::MAX, false)),
        [
            "common",
            "common-2",
            "common0",
            "dangling",
            "loop",
            "target/",
            "target/sub/",
            "target/sub/file"
        ],
        "without following, links are entries ordered like files"
    );
    Ok(())
}

#[test]
fn unicode_precomposition_applies_to_names_and_paths() -> gix_testtools::TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("walkdir.sh")?.join("unicode");
    let decomposed = "a\u{308}";
    let root = dir.join(decomposed);

    for (precompose_unicode, expected) in [(false, decomposed), (true, "ä")] {
        for walk in [
            walkdir_new(&root, precompose_unicode),
            walkdir_sorted_new(&root, 1, precompose_unicode),
        ] {
            let mut entries = walk.min_depth(1).max_depth(1).into_iter();
            let entry = entries.next().expect("the directory contains one file")?;
            assert_eq!(
                entry.file_name(),
                OsStr::new(expected),
                "file names follow the requested Unicode precomposition policy"
            );
            assert_eq!(
                entry.path(),
                dir.join(expected).join(expected),
                "the same policy applies to every component of the full path"
            );
            assert!(entry.file_type()?.is_file(), "entry metadata remains accessible");
            assert!(entries.next().is_none(), "only the direct child is returned");
        }
    }
    Ok(())
}

#[test]
fn missing_roots_yield_io_errors() -> gix_testtools::TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("walkdir.sh")?;
    let mut entries = walkdir_new(&dir.join("missing"), false).into_iter();
    let err = entries
        .next()
        .expect("an inaccessible root produces an error item")
        .expect_err("missing directories cannot be traversed");
    assert_eq!(
        err.io_error().map(std::io::Error::kind),
        Some(ErrorKind::NotFound),
        "the underlying IO error is preserved"
    );
    assert!(entries.next().is_none(), "there are no entries below a missing root");
    Ok(())
}
