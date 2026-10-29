//! The fixture `make_state.sh` records the states of rebases, cherry-pick sequences and revert sequences that Git
//! stopped, and the reference logs of rebases that ended, which `gix-sequencer` reads, and writes byte for byte as it
//! takes the same steps.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bstr::{BStr, BString, ByteSlice};
use gix_hash::ObjectId;
use gix_object::Find;
use gix_ref::transaction::{Change, RefEdit};
use gix_sequencer::todo::{ById, List, ParseOptions, default_comment, strip_space};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

mod rebase;
mod sequence;
mod todo;

/// The script that makes the cases.
const FIXTURE: &str = "make_state.sh";

/// The directory in which the fixture ran. The fixture runs `git rebase --trailer`, which Git 2.54 added, so with older
/// Git it comes from its archive.
fn root() -> TestResult<PathBuf> {
    gix_testtools::scripted_fixture_read_only_with_git_version(FIXTURE, |version| version >= (2, 54, 0))?
        .ok_or_else(|| "the archive of the fixture, as Git before 2.54 can't make it".into())
}

/// The kind of the ids of the objects of the fixture.
fn hash() -> gix_hash::Kind {
    gix_testtools::object_hash()
}

/// A case of the fixture: a repository with the state that Git left in it.
struct Case {
    /// The `.git` directory of the repository.
    git_dir: PathBuf,
    /// The objects of the repository.
    objects: gix_odb::Handle,
    /// The commits that the case names, by their names.
    revs: HashMap<String, ObjectId>,
}

impl Case {
    /// Open the case `name`.
    fn new(name: &str) -> TestResult<Self> {
        let git_dir = root()?.join(name).join(".git");
        let mut revs = HashMap::new();
        for (name, id) in pairs(&git_dir.join("revs"))? {
            revs.insert(name, ObjectId::from_hex(&id)?);
        }
        Ok(Case {
            objects: gix_odb::at(git_dir.join("objects"), hash())?,
            git_dir,
            revs,
        })
    }

    /// The commit that the case names `name`.
    fn rev(&self, name: &str) -> ObjectId {
        self.revs[name]
    }

    /// The abbreviations of the commits that the case names, by their names.
    fn shorts(&self) -> TestResult<HashMap<String, BString>> {
        Ok(pairs(&self.git_dir.join("shorts"))?.into_iter().collect())
    }

    /// The value of the `author` header of the commit `name`, and its message from its subject on, as
    /// `find_commit_subject()` finds it.
    fn author_and_message(&self, name: &str) -> TestResult<(BString, BString)> {
        let mut buf = Vec::new();
        let data = self
            .objects
            .try_find(&self.rev(name), &mut buf)?
            .ok_or("the commit exists")?;
        let (headers, message) = data.data.split_once_str("\n\n").ok_or("headers and a message")?;
        let author = headers
            .lines()
            .find_map(|line| line.strip_prefix(b"author "))
            .ok_or("an author")?;
        let start = message.iter().position(|byte| *byte != b'\n').unwrap_or(message.len());
        Ok((author.into(), message[start..].into()))
    }

    /// The subject of the commit `name`, the first line of its message.
    fn subject(&self, name: &str) -> TestResult<BString> {
        let (_, message) = self.author_and_message(name)?;
        Ok(message.lines().next().unwrap_or_default().into())
    }

    /// The todo list that the user made of the list of `git rebase -i`, as Git reads it after the edit.
    fn edited_list(&self) -> TestResult<List> {
        let content = std::fs::read(self.git_dir.join("todo"))?;
        let (list, errors) = List::parse(
            &strip_space(&content, Some(default_comment())),
            ParseOptions::default(),
            &mut ById::new(&self.objects, hash()),
        )?;
        assert!(errors.is_empty(), "{errors:?}");
        Ok(list)
    }

    /// The content of the file `name` of the git directory.
    fn file(&self, name: &str) -> TestResult<BString> {
        Ok(std::fs::read(self.git_dir.join(name))?.into())
    }

    /// The commit that Git wrote into the file of the reference `name`.
    fn ref_target(&self, name: &str) -> TestResult<ObjectId> {
        Ok(ObjectId::from_hex(self.file(name)?.trim())?)
    }

    /// The messages of the entries of the reference log of `name`, oldest first.
    fn log_messages(&self, name: &str) -> TestResult<Vec<BString>> {
        let log = self.file(&format!("logs/{name}"))?;
        Ok(log
            .lines()
            .map(|line| {
                line.split_once_str("\t")
                    .map_or(BString::default(), |(_, message)| message.into())
            })
            .collect())
    }
}

/// The pairs of words of the lines of the file at `path`.
fn pairs(path: &Path) -> TestResult<Vec<(String, BString)>> {
    let mut pairs = Vec::new();
    for line in std::fs::read(path)?.lines() {
        let (name, value) = line.split_once_str(" ").ok_or("two words")?;
        pairs.push((name.to_str()?.to_owned(), value.into()));
    }
    Ok(pairs)
}

/// The commit to which `edit` points its reference, if it updates it.
fn new_target(edit: &RefEdit) -> Option<ObjectId> {
    match &edit.change {
        Change::Update {
            new: gix_ref::Target::Object(id),
            ..
        } => Some(*id),
        _ => None,
    }
}

/// The reference log message of `edit`, if it updates its reference.
fn log_message(edit: &RefEdit) -> Option<&BStr> {
    match &edit.change {
        Change::Update { log, .. } => Some(log.message.as_bstr()),
        Change::Delete { .. } => None,
    }
}

/// Assert that `edit` points the reference `name` at the commit that Git wrote into the reference in the case.
fn assert_points_as_git_wrote(case: &Case, edit: &RefEdit, name: &str) -> TestResult {
    assert_eq!(edit.name.as_bstr(), name.as_bytes().as_bstr());
    assert_eq!(new_target(edit), Some(case.ref_target(name)?), "{name}");
    Ok(())
}

/// Assert that the directory `ours` holds the files of the directory `theirs` but those named in `except`, byte for byte,
/// and no others.
fn assert_same_files(theirs: &Path, ours: &Path, except: &[&str]) -> TestResult {
    let names = |dir: &Path| -> TestResult<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if !except.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    };
    let expected = names(theirs)?;
    assert_eq!(names(ours)?, expected, "the same files");
    for name in expected {
        assert_eq!(
            std::fs::read(ours.join(&name))?.as_bstr(),
            std::fs::read(theirs.join(&name))?.as_bstr(),
            "{name}"
        );
    }
    Ok(())
}

/// Copy the files of the directory `from` into the new directory `to`.
fn copy_dir(from: &Path, to: &Path) -> TestResult {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        std::fs::copy(entry.path(), to.join(entry.file_name()))?;
    }
    Ok(())
}
