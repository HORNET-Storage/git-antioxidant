//! The fixture records what `git write-tree` makes of the index of each of its cases: the root tree that it printed or
//! the error with which it refused the index, the trees that it wrote, and the index that it left with the cache tree
//! that it computed. The trees, cache trees and errors of `State::write_tree()` are compared with these. Indexes that
//! git refuses to read, or that it would turn into broken trees, are built by the tests themselves, and so are the
//! indexes whose cache trees are repaired, which are compared with those that `State::write_tree()` computes.

mod cache_tree;
mod refused;
mod repair;
mod trees;

use std::{collections::BTreeSet, path::PathBuf};

use gix_hash::ObjectId;
use gix_index::{
    State, entry,
    extension::Tree,
    write_tree::{Options, Outcome},
};
use gix_testtools::{TestResult, tempfile::TempDir};

/// The script that makes a repository for each case and records what `git write-tree` makes of its index.
const FIXTURE: &str = "make_write_tree.sh";

/// An object database that reads the objects of a repository and keeps the objects that are written in memory.
type Objects = gix_odb::memory::Proxy<gix_odb::Handle>;

/// A case of the fixture, in whose repository `git write-tree` ran.
struct Case {
    /// The name of the case, which is also the name of its directory.
    name: &'static str,
    /// The directory of the case's repository.
    dir: PathBuf,
}

impl Case {
    /// The case `name` of the fixture.
    fn new(name: &'static str) -> TestResult<Self> {
        Ok(Case {
            name,
            dir: gix_testtools::scripted_fixture_read_only(FIXTURE)?.join(name),
        })
    }

    /// The index of the repository, which `git write-tree` read.
    fn index(&self) -> TestResult<State> {
        self.read_index(".git/index")
    }

    /// The index that `git write-tree` left, with the cache tree that it computed.
    fn index_after(&self) -> TestResult<State> {
        self.read_index("index.after")
    }

    /// The index as it was when git computed its cache tree, before the entries of the case changed.
    fn index_cached(&self) -> TestResult<State> {
        self.read_index("index.cached")
    }

    /// The index at `path` in the directory of the case.
    fn read_index(&self, path: &str) -> TestResult<State> {
        let file = gix_index::File::at(
            self.dir.join(path),
            gix_testtools::object_hash(),
            false,
            Default::default(),
        )?;
        Ok(file.into())
    }

    /// The objects of the repository, keeping the objects that are written in memory.
    fn objects(&self) -> TestResult<Objects> {
        objects_at(self.dir.join(".git").join("objects"))
    }

    /// The id of the root tree that `git write-tree` printed.
    fn git_tree_id(&self) -> TestResult<ObjectId> {
        let hex = std::fs::read_to_string(self.dir.join("tree"))?;
        Ok(ObjectId::from_hex(hex.trim_end().as_bytes())?)
    }

    /// The ids of the trees that `git write-tree` wrote, which are the loose objects in `git-trees/`.
    fn git_written_trees(&self) -> TestResult<BTreeSet<ObjectId>> {
        let mut ids = BTreeSet::new();
        for fan_out in std::fs::read_dir(self.dir.join("git-trees"))? {
            let fan_out = fan_out?;
            let prefix = fan_out.file_name();
            let prefix = prefix
                .to_str()
                .ok_or("the directories of loose objects have hex names")?;
            for object in std::fs::read_dir(fan_out.path())? {
                let rest = object?.file_name();
                let rest = rest.to_str().ok_or("loose objects have hex names")?;
                ids.insert(ObjectId::from_hex(format!("{prefix}{rest}").as_bytes())?);
            }
        }
        Ok(ids)
    }

    /// The lines with which `git write-tree` refused the index, without the last one, which only says that the trees
    /// couldn't be built.
    fn git_error_lines(&self) -> TestResult<Vec<String>> {
        let error = std::fs::read_to_string(self.dir.join("error"))?;
        let mut lines: Vec<_> = error.lines().map(ToOwned::to_owned).collect();
        assert_eq!(
            lines.pop().as_deref(),
            Some("fatal: git-write-tree: error building trees"),
            "{}: git ends its error with the command that failed",
            self.name
        );
        Ok(lines)
    }
}

/// The objects of the object database at `dir`, keeping the objects that are written in memory.
fn objects_at(dir: impl Into<PathBuf>) -> TestResult<Objects> {
    let object_hash = gix_testtools::object_hash();
    Ok(Objects::new(gix_odb::at(dir, object_hash)?, object_hash))
}

/// An object database without objects, keeping the objects that are written in memory, along with the empty directory
/// that it reads, which has to be kept while the object database is used.
fn no_objects() -> TestResult<(Objects, TempDir)> {
    let dir = gix_testtools::tempfile::tempdir()?;
    Ok((objects_at(dir.path())?, dir))
}

/// The ids of the objects that were written to `objects`, which forgets them.
fn take_written(objects: &Objects) -> BTreeSet<ObjectId> {
    objects
        .reset_object_memory()
        .map(|storage| storage.keys().copied().collect())
        .unwrap_or_default()
}

/// Write the trees of `index`, the index of `case`, with `cache_tree` and `options`, assert that the root tree, the cache
/// tree and the written trees are those of `git write-tree`, and return the outcome.
fn assert_matches_git(case: &Case, index: &State, cache_tree: Option<&Tree>, options: Options) -> TestResult<Outcome> {
    let objects = case.objects()?;
    let outcome = index.write_tree(cache_tree, &objects, options)?;
    let name = case.name;
    assert_eq!(
        outcome.tree_id,
        case.git_tree_id()?,
        "{name}: the root tree is the one that git printed"
    );
    assert_eq!(
        Some(&outcome.cache_tree),
        case.index_after()?.tree(),
        "{name}: the cache tree is the one that git stored in the index"
    );
    let written = take_written(&objects);
    assert_eq!(
        written,
        case.git_written_trees()?,
        "{name}: the trees that the repository didn't have are written, as git wrote them"
    );
    assert_eq!(
        outcome.written_trees,
        written.len(),
        "{name}: the outcome counts the trees that were written"
    );
    Ok(outcome)
}

/// A new index without entries.
fn new_index() -> State {
    State::new(gix_testtools::object_hash())
}

/// Append an entry at `path` with `mode`, `id` and `flags` to `index`, without any checks.
fn push(index: &mut State, path: &str, mode: entry::Mode, id: ObjectId, flags: entry::Flags) {
    index.dangerously_push_entry(entry::Stat::default(), id, flags, mode, path.into());
}

/// Write `content` to `objects` as a blob and return its id.
fn blob(objects: &Objects, content: &str) -> TestResult<ObjectId> {
    Ok(gix_object::Write::write_buf(
        objects,
        gix_object::Kind::Blob,
        content.as_bytes(),
    )?)
}

/// The message of `err` and those of its causes.
fn messages(err: gix_error::Error) -> (String, Vec<String>) {
    let message = err.error().to_string();
    let causes = err
        .into_exn()
        .frame()
        .children()
        .iter()
        .map(|cause| cause.error().to_string())
        .collect();
    (message, causes)
}
