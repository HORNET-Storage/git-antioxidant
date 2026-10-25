//! Loose references reached through symbolic links, compared with how Git's files backend lists, reads and writes them.
//! The fixtures record Git's results on the test host, as archives can't represent links on every host.

use std::path::Path;

use gix_hash::ObjectId;
use gix_lock::acquire::Fail;
use gix_ref::{
    Reference, Target,
    file::Store,
    store::WriteReflog,
    transaction::{PreviousValue, RefEdit},
};
use gix_testtools::{Creation, TestResult, fixture_has_symlinks};

/// Follow `reference` to the object it leads to as `git rev-parse --verify` does, or return `None` if it leads to a
/// reference that doesn't exist or through more symbolic references than Git follows.
fn resolve(store: &Store, mut reference: Reference) -> crate::Result<Option<ObjectId>> {
    // Like Git's `refs_resolve_ref_unsafe()`, read at most five references (`SYMREF_MAXDEPTH`), this one included.
    for _ in 1..5 {
        let name = match &reference.target {
            Target::Object(_) => break,
            Target::Symbolic(name) => name.clone(),
        };
        match store.try_find(name.as_ref())? {
            Some(next) => reference = next,
            None => return Ok(None),
        }
    }
    Ok(reference.target.try_id().map(ToOwned::to_owned))
}

/// List the references of `store` in the order of its iteration, as
/// `git for-each-ref --format='%(refname) %(objectname) %(symref)'` lists them: symbolic references that don't lead
/// to an object are left out.
fn listing(store: &Store) -> crate::Result<Vec<String>> {
    let mut lines = Vec::new();
    for reference in store.iter()?.all()? {
        let reference = reference?;
        let name = reference.name.to_string();
        let symref = reference.target.try_name().map(ToString::to_string).unwrap_or_default();
        if let Some(id) = resolve(store, reference)? {
            lines.push(format!("{name} {id} {symref}"));
        }
    }
    Ok(lines)
}

/// Read the reference `name` from `store` as `git rev-parse --verify` and `git symbolic-ref` report it, with `-` where
/// they report nothing.
fn read(store: &Store, name: &str) -> crate::Result<String> {
    let (id, symref) = match store.try_find(name)? {
        Some(reference) => {
            let symref = reference
                .target
                .try_name()
                .map_or_else(|| "-".to_owned(), ToString::to_string);
            let id = resolve(store, reference)?.map_or_else(|| "-".to_owned(), |id| id.to_string());
            (id, symref)
        }
        None => ("-".to_owned(), "-".to_owned()),
    };
    Ok(format!("{name} {id} {symref}"))
}

/// Return the lines that the fixture script recorded from Git at `path`.
fn git_baseline(path: impl AsRef<Path>) -> crate::Result<Vec<String>> {
    Ok(std::fs::read_to_string(path)?.lines().map(ToOwned::to_owned).collect())
}

/// Describe each entry below `refs/heads` in `git_dir` in path order: a directory with a trailing `/`, a link with its
/// text, and a file with its contents.
fn heads(git_dir: &Path) -> crate::Result<Vec<String>> {
    let mut lines = Vec::new();
    describe(&git_dir.join("refs").join("heads"), "", &mut lines)?;
    Ok(lines)
}

/// Add the description of each entry in `dir` and its subdirectories to `lines`, each named with `prefix`.
fn describe(dir: &Path, prefix: &str, lines: &mut Vec<String>) -> crate::Result {
    let mut names = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let name = format!("{prefix}{}", name.to_string_lossy());
        let file_type = path.symlink_metadata()?.file_type();
        if file_type.is_symlink() {
            let text = gix_path::into_bstr(std::fs::read_link(&path)?)?.into_owned();
            lines.push(format!("{name} -> {}", gix_path::to_unix_separators_on_windows(text)));
        } else if file_type.is_dir() {
            lines.push(format!("{name}/"));
            describe(&path, &format!("{name}/"), lines)?;
        } else {
            lines.push(format!("{name}: {}", std::fs::read_to_string(&path)?.trim_end()));
        }
    }
    Ok(())
}

#[test]
#[cfg(any(unix, windows))]
fn links_are_listed_and_read_as_git_does() -> TestResult {
    let root = crate::scripted_fixture_read_only("make_symlinked_refs.sh")?;
    if !fixture_has_symlinks(&root)? {
        return Ok(());
    }
    let git_dir = root.join("repo").join(".git");
    for link in [
        "HEAD",
        "refs/heads/text-main",
        "refs/heads/path-main",
        "refs/heads/dangling",
        "refs/heads/dir-link",
    ] {
        assert!(
            git_dir.join(link).symlink_metadata()?.file_type().is_symlink(),
            "{link} is a symbolic link"
        );
    }
    let store = Store::at(git_dir, crate::fixture_hash_kind());
    assert_eq!(
        listing(&store)?,
        git_baseline(root.join("repo.listing"))?,
        "links to files are references, links to directories are walked, and links that lead nowhere are skipped"
    );
    let reads = git_baseline(root.join("repo.reads"))?;
    assert_eq!(
        reads
            .iter()
            .map(|line| read(&store, line.split(' ').next().expect("each line starts with a name")))
            .collect::<crate::Result<Vec<_>>>()?,
        reads,
        "a link is a symbolic reference if its text is a valid reference name in refs/, and is read through otherwise"
    );
    Ok(())
}

#[test]
#[cfg(any(unix, windows))]
fn references_below_a_linked_directory_merge_with_packed_references_in_name_order() -> TestResult {
    let root = crate::scripted_fixture_read_only("make_symlinked_refs.sh")?;
    if !fixture_has_symlinks(&root)? {
        return Ok(());
    }
    let store = Store::at(root.join("packed").join(".git"), crate::fixture_hash_kind());
    assert_eq!(
        listing(&store)?,
        git_baseline(root.join("packed.listing"))?,
        "refs/heads/dir-link/sub/x comes between refs/heads/dir-link-2 and refs/heads/dir-link0, and the loose refs/heads/dir-link-2 replaces the packed one"
    );
    Ok(())
}

#[test]
#[cfg(any(unix, windows))]
fn links_that_lead_back_into_the_walk_are_skipped() -> TestResult {
    let root = crate::scripted_fixture_read_only("make_symlinked_refs.sh")?;
    if !fixture_has_symlinks(&root)? {
        return Ok(());
    }
    let store = Store::at(root.join("loop").join(".git"), crate::fixture_hash_kind());
    let names = store
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    // Git walks such a link again at every level and lists the same references under ever longer names, until the
    // system refuses to resolve the path.
    assert_eq!(names, ["refs/heads/main"], "each directory is walked once");
    Ok(())
}

#[test]
#[cfg(any(unix, windows))]
fn writes_through_links_change_what_git_changes() -> TestResult {
    let fixture = crate::scripted_fixture_writable_with_args(
        "make_symlinked_refs_for_writes.sh",
        None::<String>,
        Creation::Execute,
    )?;
    if !fixture_has_symlinks(fixture.path())? {
        return Ok(());
    }
    let git_dir = fixture.path().join("gix").join(".git");
    let mut store = Store::at(git_dir.clone(), crate::fixture_hash_kind());
    store.write_reflog = WriteReflog::Disable;
    let one = store.find("other")?.target.into_id();
    let two = store.find("main")?.target.into_id();
    for edit in [
        RefEdit::update("refs/heads/text-main".try_into()?, one, PreviousValue::Any, ""),
        RefEdit::update("refs/heads/path-main".try_into()?, two, PreviousValue::Any, ""),
        RefEdit::delete("refs/heads/path-other".try_into()?, PreviousValue::MustExist),
    ] {
        // Like `git update-ref`, apply the edit to the reference that a symbolic reference names.
        store
            .transaction()
            .prepare([RefEdit { deref: true, ..edit }], Fail::Immediately, Fail::Immediately)?
            .commit(None)?;
    }
    assert_eq!(
        heads(&git_dir)?,
        heads(&fixture.path().join("git").join(".git"))?,
        "updating text-main changes main, updating path-main replaces the link with a file, and deleting path-other removes only the link"
    );
    Ok(())
}
