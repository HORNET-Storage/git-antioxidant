//! The fixtures record what `git add --all` makes of the worktree of each of their cases: the entries that it left in the
//! index, the blobs that it wrote and the errors that it reported. The entries, blobs and counted changes of this crate are
//! compared with these. The cases that depend on the time at which they run, and those that this crate refuses, are set up
//! by the tests themselves.

mod git;
mod refused;
mod stat;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use bstr::BString;
use gix_add::{Context, Options, Outcome, add_all};
use gix_hash::ObjectId;
use gix_index::{State, entry};
use gix_testtools::{TestResult, tempfile::TempDir};
use pretty_assertions::assert_eq;

/// The script that makes the cases whose results are the same on all hosts.
const FIXTURE: &str = "make_add.sh";

/// How the stat information of entries is compared with that of their files: by the seconds of their modification
/// times and by their sizes, as the other information changes when the fixtures are extracted from their archives.
const STAT: entry::stat::Options = entry::stat::Options {
    trust_ctime: false,
    check_stat: false,
    use_nsec: false,
    use_stdev: false,
};

/// What an entry records besides its stat information: its path, stage, mode and object, and the flags that adding may
/// change.
type Record = (BString, entry::Stage, entry::Mode, ObjectId, entry::Flags);

/// A case of a fixture, in whose repository `git add --all` ran.
struct Case {
    /// The name of the case, which is also the name of its directory.
    name: &'static str,
    /// The worktree of the case's repository, as an absolute path.
    dir: PathBuf,
}

impl Case {
    /// The case `name` of the fixture whose results are the same on all hosts.
    fn new(name: &'static str) -> TestResult<Self> {
        Case::of(FIXTURE, name)
    }

    /// The case `name` of the fixture that `script` makes.
    fn of(script: &str, name: &'static str) -> TestResult<Self> {
        let dir = gix_testtools::scripted_fixture_read_only(script)?.join(name);
        Ok(Case {
            name,
            dir: std::path::absolute(dir)?,
        })
    }

    /// A copy of the case `name` of the fixture whose results are the same on all hosts, which the test may change, along
    /// with the directory that contains it and has to be kept while the copy is used.
    fn writable(name: &'static str) -> TestResult<(Self, TempDir)> {
        let tmp = gix_testtools::scripted_fixture_writable(FIXTURE)?;
        let dir = tmp.path().join(name);
        Ok((Case { name, dir }, tmp))
    }

    /// The `.git` directory of the case's repository.
    fn git_dir(&self) -> PathBuf {
        self.dir.join(".git")
    }

    /// The index of the repository before `git add --all` ran, or a new one if the repository had none.
    fn index(&self) -> TestResult<State> {
        let path = self.git_dir().join("index");
        if path.exists() {
            read_index(&path)
        } else {
            Ok(State::new(gix_testtools::object_hash()))
        }
    }

    /// The index that `git add --all` left.
    fn index_after(&self) -> TestResult<State> {
        read_index(&self.git_dir().join("index.after"))
    }

    /// The ids of the blobs that `git add --all` wrote.
    fn git_written_blobs(&self) -> TestResult<BTreeSet<ObjectId>> {
        loose_objects(&self.git_dir().join("git-objects"))
    }

    /// What `git add --all` printed to its standard error if it failed.
    fn git_error(&self) -> TestResult<Option<String>> {
        let path = self.git_dir().join("error");
        Ok(if path.exists() {
            Some(std::fs::read_to_string(path)?)
        } else {
            None
        })
    }

    /// The configuration of the repository.
    fn config(&self) -> TestResult<gix_config::File> {
        Ok(gix_config::File::from_path_no_includes(
            self.git_dir().join("config"),
            gix_config::Source::Local,
        )?)
    }

    /// The options for the repository with at most `thread_limit` threads: the capabilities of its file system as its
    /// configuration records them, with Git's defaults for those it doesn't set, and the stat options of the tests.
    fn options(&self, thread_limit: Option<usize>) -> TestResult<Options> {
        let config = self.config()?;
        let flag = |key: &str, default: bool| -> TestResult<bool> { Ok(config.boolean(key)?.unwrap_or(default)) };
        Ok(Options {
            fs: gix_fs::Capabilities {
                precompose_unicode: flag("core.precomposeUnicode", false)?,
                ignore_case: flag("core.ignoreCase", false)?,
                executable_bit: flag("core.fileMode", true)?,
                symlink: flag("core.symlinks", true)?,
            },
            stat: STAT,
            thread_limit,
            fscache: false,
        })
    }

    /// The stack for `index` with which `git add` reads attributes and exclude patterns in the repository: those of the
    /// `.gitattributes` and `.gitignore` files, of `.git/info/attributes` and `.git/info/exclude`, and of the file that
    /// `core.excludesFile` names, which is relative to the worktree, where git ran.
    fn stack(&self, index: &State, ignore_case: bool) -> TestResult<gix_worktree::Stack> {
        let git_dir = self.git_dir();
        let mut buf = Vec::new();
        let mut collection = Default::default();
        let globals = gix_attributes::Search::new_globals(std::iter::empty::<PathBuf>(), &mut buf, &mut collection)?;
        let attributes = gix_worktree::stack::state::Attributes::new(
            globals,
            Some(git_dir.join("info").join("attributes")),
            gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            collection,
        );
        let excludes_file = match self.config()?.string("core.excludesFile") {
            Some(path) => Some(self.dir.join(gix_path::from_bstring(path)?)),
            None => None,
        };
        let parse = gix_ignore::search::Ignore::default();
        let ignore = gix_worktree::stack::state::Ignore::new(
            Default::default(),
            gix_ignore::Search::from_git_dir(&git_dir, excludes_file, &mut buf, parse)?,
            None,
            gix_worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            parse,
        );
        Ok(gix_worktree::Stack::from_state_and_ignore_case(
            &self.dir,
            ignore_case,
            gix_worktree::stack::State::for_add(attributes, ignore),
            index,
            index.path_backing(),
        ))
    }

    /// The filters that `git add` applies in the repository, with the conversion of line endings that `core.autocrlf` asks
    /// for.
    fn filter(&self) -> TestResult<gix_filter::Pipeline> {
        let config = self.config()?;
        let auto_crlf = match config.string("core.autocrlf") {
            Some(value) if value == "input" => gix_filter::eol::AutoCrlf::Input,
            Some(_) if config.boolean("core.autocrlf")? == Some(true) => gix_filter::eol::AutoCrlf::Enabled,
            _ => gix_filter::eol::AutoCrlf::Disabled,
        };
        Ok(gix_filter::Pipeline::new(
            Default::default(),
            gix_testtools::object_hash(),
            gix_filter::pipeline::Options {
                eol_config: gix_filter::eol::Configuration { auto_crlf, eol: None },
                ..Default::default()
            },
        ))
    }

    /// Run [`add_all()`] on `index` in the repository of the case with `pathspec` and `options`, writing blobs to
    /// `objects`, and return its result. The outer result fails if the call can't be set up.
    fn add(
        &self,
        index: &mut State,
        pathspec: &[&str],
        objects: &Objects,
        options: Options,
    ) -> TestResult<gix_error::Result<Outcome>> {
        self.add_interruptible(index, pathspec, objects, options, &AtomicBool::new(false))
    }

    /// Like [`Case::add()`], with `should_interrupt` as the flag that interrupts the call.
    fn add_interruptible(
        &self,
        index: &mut State,
        pathspec: &[&str],
        objects: &Objects,
        options: Options,
        should_interrupt: &AtomicBool,
    ) -> TestResult<gix_error::Result<Outcome>> {
        let git_dir_realpath = gix_path::realpath(self.git_dir())?;
        let current_dir = gix_fs::current_dir(options.fs.precompose_unicode)?;
        let ctx = Context {
            pathspec: pathspec_search(pathspec)?,
            stack: self.stack(index, options.fs.ignore_case)?,
            filter: self.filter()?,
            git_dir_realpath: &git_dir_realpath,
            current_dir: &current_dir,
            should_interrupt,
        };
        Ok(add_all(
            index,
            &self.dir,
            objects.handle.clone(),
            repository_head,
            ctx,
            options,
        ))
    }
}

/// An object database that writes objects into a directory of its own, and reads those of a repository as its
/// alternate.
struct Objects {
    /// The directory of the objects that are written, which has to be kept while the object database is used.
    dir: TempDir,
    /// The object database.
    handle: gix_odb::Handle,
}

impl Objects {
    /// An object database with the objects of the repository of `case`.
    fn new(case: &Case) -> TestResult<Self> {
        let dir = gix_testtools::tempfile::tempdir()?;
        let alternate = case.git_dir().join("objects");
        let alternate = alternate
            .to_str()
            .ok_or("the fixtures are at paths that are valid UTF-8")?;
        std::fs::create_dir(dir.path().join("info"))?;
        std::fs::write(dir.path().join("info").join("alternates"), format!("{alternate}\n"))?;
        let handle = gix_odb::at(dir.path(), gix_testtools::object_hash())?;
        Ok(Objects { dir, handle })
    }

    /// The ids of the objects that were written.
    fn written(&self) -> TestResult<BTreeSet<ObjectId>> {
        loose_objects(self.dir.path())
    }
}

/// The index at `path`.
fn read_index(path: &Path) -> TestResult<State> {
    let file = gix_index::File::at(path, gix_testtools::object_hash(), false, Default::default())?;
    Ok(file.into())
}

/// The ids of the loose objects in the object directory `dir`, which has none if it doesn't exist.
fn loose_objects(dir: &Path) -> TestResult<BTreeSet<ObjectId>> {
    let mut ids = BTreeSet::new();
    if !dir.is_dir() {
        return Ok(ids);
    }
    for fan_out in std::fs::read_dir(dir)? {
        let fan_out = fan_out?;
        let name = fan_out.file_name();
        let Some(prefix) = name
            .to_str()
            .filter(|name| name.len() == 2 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        else {
            continue;
        };
        for object in std::fs::read_dir(fan_out.path())? {
            let rest = object?.file_name();
            let rest = rest.to_str().ok_or("loose objects have hexadecimal names")?;
            ids.insert(ObjectId::from_hex(format!("{prefix}{rest}").as_bytes())?);
        }
    }
    Ok(ids)
}

/// The commit that the `HEAD` of the repository at `dir` points to, or `None` if `dir` isn't a repository or its `HEAD`
/// doesn't point to a commit, as Git's `resolve_gitlink_ref()` finds it.
fn repository_head(dir: &Path) -> gix_error::Result<Option<ObjectId>> {
    let dot_git = dir.join(".git");
    if gix_discover::is_git(&dot_git).is_err() {
        return Ok(None);
    }
    let git_dir = if dot_git.is_file() {
        gix_discover::path::from_gitdir_file(&dot_git)?
    } else {
        dot_git
    };
    let refs = gix_ref::file::Store::at(git_dir, gix_testtools::object_hash());
    let mut reference = refs.try_find("HEAD")?;
    // Git follows up to five symbolic references.
    for _ in 0..5 {
        match reference {
            Some(gix_ref::Reference {
                target: gix_ref::Target::Symbolic(name),
                ..
            }) => reference = refs.try_find(&name)?,
            Some(gix_ref::Reference {
                target: gix_ref::Target::Object(id),
                ..
            }) => return Ok(Some(id)),
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The search for the pathspec `patterns`, which matches all paths if there are none.
fn pathspec_search(patterns: &[&str]) -> TestResult<gix_pathspec::Search> {
    let patterns = patterns
        .iter()
        .map(|pattern| gix_pathspec::parse(pattern.as_bytes(), Default::default()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(gix_pathspec::Search::from_specs(patterns, None, Path::new(""))?)
}

/// The records of the entries of `index`, in their order.
fn records(index: &State) -> Vec<Record> {
    let flags = entry::Flags::INTENT_TO_ADD | entry::Flags::SKIP_WORKTREE | entry::Flags::ASSUME_VALID;
    index
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.path(index).to_owned(),
                entry.stage(),
                entry.mode,
                entry.id,
                entry.flags & flags,
            )
        })
        .collect()
}

/// The number of paths that were added, modified and removed on the way from `before` to `after`, as the records of their
/// entries tell.
fn changes(before: &State, after: &State) -> (usize, usize, usize) {
    let by_path = |index: &State| {
        let mut paths = BTreeMap::<BString, Vec<Record>>::new();
        for record in records(index) {
            paths.entry(record.0.clone()).or_default().push(record);
        }
        paths
    };
    let (before, after) = (by_path(before), by_path(after));
    let added = after.keys().filter(|path| !before.contains_key(*path)).count();
    let removed = before.keys().filter(|path| !after.contains_key(*path)).count();
    let modified = after
        .iter()
        .filter(|(path, records)| before.get(*path).is_some_and(|old| old != *records))
        .count();
    (added, modified, removed)
}

/// Assert that `git add --all` succeeded in `case`, and that adding its changes with `pathspec` leaves what git left, as
/// [`assert_entries_match_git()`] checks.
fn assert_matches_git(case: &Case, pathspec: &[&str]) -> TestResult<(Outcome, State)> {
    assert_eq!(case.git_error()?, None, "{}: git succeeded", case.name);
    assert_entries_match_git(case, pathspec)
}

/// Add the changes of `case` with `pathspec`, once on one thread and once on as many as there are cores, and assert that
/// both leave the entries that `git add --all` left, write the blobs that it wrote, count the paths that changed and leave
/// a valid cache tree. Return the outcome of the second run and the index that it left.
fn assert_entries_match_git(case: &Case, pathspec: &[&str]) -> TestResult<(Outcome, State)> {
    let name = case.name;
    let before = case.index()?;
    let git = case.index_after()?;
    let mut last = None;
    for thread_limit in [Some(1), None] {
        let mut index = before.clone();
        let objects = Objects::new(case)?;
        let outcome = case.add(&mut index, pathspec, &objects, case.options(thread_limit)?)??;
        assert_eq!(
            records(&index),
            records(&git),
            "{name}: the entries are those that git left"
        );
        let written = objects.written()?;
        assert_eq!(
            written,
            case.git_written_blobs()?,
            "{name}: the blobs that the repository didn't have are written, as git wrote them"
        );
        assert_eq!(
            outcome.written_blobs,
            written.len(),
            "{name}: the outcome counts the blobs that were written"
        );
        assert_eq!(
            (outcome.added, outcome.modified, outcome.removed),
            changes(&before, &index),
            "{name}: the outcome counts the paths that were added, modified and removed"
        );
        assert_cache_tree_is_valid(case, &index, &objects)?;
        last = Some((outcome, index));
    }
    Ok(last.expect("both runs succeeded"))
}

/// Assert that wherever the cache tree of `index`, an index of `case` that has the objects of `objects`, is valid, it names
/// the trees that its entries make.
fn assert_cache_tree_is_valid(case: &Case, index: &State, objects: &Objects) -> TestResult {
    let trees = gix_odb::memory::Proxy::new(objects.handle.clone(), gix_testtools::object_hash());
    let with_cache_tree = index.write_tree(index.tree(), &trees, Default::default())?;
    let from_entries = index.write_tree(None, &trees, Default::default())?;
    assert_eq!(
        (with_cache_tree.tree_id, with_cache_tree.cache_tree),
        (from_entries.tree_id, from_entries.cache_tree),
        "{}: the valid nodes of the cache tree name the trees of their directories",
        case.name
    );
    Ok(())
}
