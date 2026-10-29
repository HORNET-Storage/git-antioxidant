//! The fixture `make_rebase_todo.sh` records what Git decides for rebases, which `gix-rebase` decides the same way: the
//! todo lists that `git rebase -i` hands its editor, whether `git rebase` finds a branch up to date, the fork point that
//! `git merge-base --fork-point` finds, and the patch IDs that `git patch-id --stable` computes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bstr::{BString, ByteSlice};
use gix_hash::ObjectId;
use gix_testtools::TestResult;

mod patch_id;
mod plan;
mod todo;

/// The script that makes the cases.
const FIXTURE: &str = "make_rebase_todo.sh";

/// The directory in which the fixture ran. Since Git 2.50, the todo lists of `git rebase -i` put `#` before the subjects
/// of the commits, as `gix-rebase` does, so with older Git the fixture comes from its archive.
fn root() -> TestResult<PathBuf> {
    gix_testtools::scripted_fixture_read_only_with_git_version(FIXTURE, |version| version >= (2, 50, 0))?
        .ok_or_else(|| "the archive of the fixture, as Git before 2.50 writes other todo lists".into())
}

/// A repository of the fixture.
struct Repo {
    /// The `.git` directory.
    git_dir: PathBuf,
    /// The objects.
    objects: gix_odb::Handle,
    /// The references.
    refs: gix_ref::file::Store,
}

impl Repo {
    /// Open the repository in the directory `name` of the fixture at `root`.
    fn at(root: &Path, name: &str) -> TestResult<Self> {
        let git_dir = root.join(name).join(".git");
        let object_hash = gix_testtools::object_hash();
        Ok(Repo {
            objects: gix_odb::at(git_dir.join("objects"), object_hash)?,
            refs: gix_ref::file::Store::at(git_dir.clone(), object_hash),
            git_dir,
        })
    }

    /// The commits of the repository.
    fn commits(&self) -> gix_rebase::Commits<'_> {
        gix_rebase::Commits::new(&self.objects, gix_testtools::object_hash())
    }

    /// The references of the repository that point at objects, with these objects.
    fn refs(&self) -> TestResult<Vec<(gix_ref::FullName, ObjectId)>> {
        let mut refs = Vec::new();
        let platform = self.refs.iter()?;
        for reference in platform.all()? {
            let reference = reference?;
            if let gix_ref::Target::Object(id) = reference.target {
                refs.push((reference.name, id));
            }
        }
        Ok(refs)
    }
}

/// The commits that the case `case` names in its `revs`, by their names.
fn revs(root: &Path, case: &str) -> TestResult<HashMap<String, ObjectId>> {
    let mut revs = HashMap::new();
    for line in std::fs::read(root.join(case).join("revs"))?.lines() {
        let (name, id) = line.split_once_str(" ").ok_or("a name and an id")?;
        revs.insert(name.to_str()?.to_owned(), ObjectId::from_hex(id)?);
    }
    Ok(revs)
}

/// The content of the file `name` of the case `case`.
fn file(root: &Path, case: &str, name: &str) -> TestResult<BString> {
    Ok(std::fs::read(root.join(case).join(name))?.into())
}

/// The id in the file `name` of the case `case`, which holds it on a line.
fn id_in(root: &Path, case: &str, name: &str) -> TestResult<ObjectId> {
    Ok(ObjectId::from_hex(file(root, case, name)?.trim())?)
}
