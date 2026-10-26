//! The fixtures record what `git switch` made of each of their cases in a second repository like it: the index and the
//! worktree that it left, or the paths that it refused. Each test switches a copy of a case and compares the outcome with
//! that of git. The cases where this crate deviates from git, and those that depend on errors or on the time at which they
//! run, are checked by the tests themselves.

mod deviation;
mod error;
mod git;
mod stat;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use bstr::BString;
use filetime::FileTime;
use gix_hash::{ObjectId, oid};
use gix_index::{State, entry, extension::Tree};
use gix_switch::{Context, Options, Outcome, Refusals, Switched, switch};
use gix_testtools::{TestResult, tempfile::TempDir};
use gix_worktree::stack::state::attributes::Source;
use pretty_assertions::assert_eq;

/// The script that makes the cases whose results are the same on all hosts.
const FIXTURE: &str = "make_switch.sh";

/// The script that makes the cases whose results depend on the file system of the host. They aren't archived, so that
/// each host records its own.
const HOST_FIXTURE: &str = "make_switch_host.sh";

/// The script that makes the cases with symbolic links, on hosts that can create them.
const SYMLINKS_FIXTURE: &str = "make_switch_symlinks.sh";

/// How the stat information of entries is compared with that of their files: by the seconds of their modification
/// times and by their sizes, as the other information changes when the fixtures are extracted from their archives.
const STAT: entry::stat::Options = entry::stat::Options {
    trust_ctime: false,
    check_stat: false,
    use_nsec: false,
    use_stdev: false,
};

/// What an entry records besides its stat information: its path, stage, mode and object, and the flags that a switch
/// may change.
type Record = (BString, entry::Stage, entry::Mode, ObjectId, entry::Flags);

/// What an index holds: its entries with their stat information, its timestamp and its cache tree.
type Contents = (Vec<(Record, entry::Stat)>, FileTime, Option<Tree>);

/// A case of a fixture: a repository, and a second one like it that git switched to the branch `new`.
struct Case {
    /// The name of the case, which is also the name of its directory.
    name: &'static str,
    /// The worktree of the case's repository, as an absolute path.
    dir: PathBuf,
    /// The worktree of the repository that git switched, as an absolute path.
    git: PathBuf,
}

impl Case {
    /// The case `name` of the fixture whose results are the same on all hosts.
    fn new(name: &'static str) -> TestResult<Self> {
        Case::of(FIXTURE, name)
    }

    /// The case `name` of the fixture that `script` makes.
    fn of(script: &str, name: &'static str) -> TestResult<Self> {
        let root = std::path::absolute(gix_testtools::scripted_fixture_read_only(script)?)?;
        Ok(Case {
            name,
            dir: root.join(name),
            git: root.join(format!("{name}-git")),
        })
    }

    /// A copy of the case `name` of the fixture whose results are the same on all hosts, which the test may switch,
    /// along with the directory that contains it and has to be kept while the copy is used.
    fn writable(name: &'static str) -> TestResult<(Self, TempDir)> {
        Case::new(name)?.writable_copy()
    }

    /// A copy of the case, which the test may switch, along with the directory that contains it and has to be kept while
    /// the copy is used. The repository that git switched stays where it is.
    fn writable_copy(&self) -> TestResult<(Self, TempDir)> {
        let tmp = gix_testtools::tempfile::tempdir()?;
        let dir = tmp.path().join(self.name);
        copy_dir(&self.dir, &dir)?;
        let copy = Case {
            name: self.name,
            dir,
            git: self.git.clone(),
        };
        Ok((copy, tmp))
    }

    /// The `.git` directory of the case's repository.
    fn git_dir(&self) -> PathBuf {
        self.dir.join(".git")
    }

    /// The index of the case's repository.
    fn index(&self) -> TestResult<State> {
        read_index(&self.git_dir().join("index"))
    }

    /// The object database of the case's repository.
    fn objects(&self) -> TestResult<gix_odb::Handle> {
        Ok(gix_odb::at(
            self.git_dir().join("objects"),
            gix_testtools::object_hash(),
        )?)
    }

    /// The trees that git switched between: the tree of `HEAD`, which is `None` on a branch without commits, and the tree
    /// of `new`.
    fn trees(&self) -> TestResult<(Option<ObjectId>, ObjectId)> {
        let tree = |name: &str| -> TestResult<Option<ObjectId>> {
            let path = self.git_dir().join(name);
            if !path.exists() {
                return Ok(None);
            }
            Ok(Some(ObjectId::from_hex(
                std::fs::read_to_string(path)?.trim_end().as_bytes(),
            )?))
        };
        let new_tree = tree("new-tree")?.ok_or("every case records the tree that it switches to")?;
        Ok((tree("old-tree")?, new_tree))
    }

    /// The directory in which git ran, as an absolute path.
    fn current_dir(&self) -> TestResult<PathBuf> {
        let path = self.git_dir().join("cwd");
        Ok(if path.exists() {
            self.dir.join(std::fs::read_to_string(path)?.trim_end())
        } else {
            self.dir.clone()
        })
    }

    /// The index that git left.
    fn git_index(&self) -> TestResult<State> {
        read_index(&self.git.join(".git").join("index"))
    }

    /// What git printed if it refused to switch.
    fn git_error(&self) -> TestResult<Option<String>> {
        let path = self.git.join(".git").join("error");
        Ok(if path.exists() {
            Some(std::fs::read_to_string(path)?)
        } else {
            None
        })
    }

    /// The configuration of the case's repository.
    fn config(&self) -> TestResult<gix_config::File> {
        Ok(gix_config::File::from_path_no_includes(
            self.git_dir().join("config"),
            gix_config::Source::Local,
        )?)
    }

    /// The options of a switch in the case's repository: the capabilities of its file system as its configuration
    /// records them, with Git's defaults for those it doesn't set, and the stat options of the tests. The checkout reads
    /// the attributes of the new index before those of the worktree, as git does.
    fn options(&self) -> TestResult<Options> {
        let config = self.config()?;
        let flag = |key: &str, default: bool| -> TestResult<bool> { Ok(config.boolean(key)?.unwrap_or(default)) };
        let mut checkout = gix_worktree_state::checkout::Options::new(filter());
        checkout.fs = gix_fs::Capabilities {
            precompose_unicode: flag("core.precomposeUnicode", false)?,
            ignore_case: flag("core.ignoreCase", false)?,
            executable_bit: flag("core.fileMode", true)?,
            symlink: flag("core.symlinks", true)?,
        };
        checkout.stat_options = STAT;
        checkout.attributes = self.attributes(Source::IdMappingThenWorktree)?;
        Ok(Options {
            checkout,
            fscache: false,
        })
    }

    /// The attributes of the `.gitattributes` files, read from `source`, and those of `.git/info/attributes`.
    fn attributes(&self, source: Source) -> TestResult<gix_worktree::stack::state::Attributes> {
        let mut buf = Vec::new();
        let mut collection = Default::default();
        let globals = gix_attributes::Search::new_globals(std::iter::empty::<PathBuf>(), &mut buf, &mut collection)?;
        Ok(gix_worktree::stack::state::Attributes::new(
            globals,
            Some(self.git_dir().join("info").join("attributes")),
            source,
            collection,
        ))
    }

    /// The stack for `index` with which git reads attributes and exclude patterns while it compares the worktree with the
    /// index: those of the `.gitattributes` and `.gitignore` files, which it reads from the worktree first, and those of
    /// `.git/info/attributes` and `.git/info/exclude`.
    fn stack(&self, index: &State, ignore_case: bool) -> TestResult<gix_worktree::Stack> {
        let mut buf = Vec::new();
        let parse = gix_ignore::search::Ignore::default();
        let ignore = gix_worktree::stack::state::Ignore::new(
            Default::default(),
            gix_ignore::Search::from_git_dir(&self.git_dir(), None, &mut buf, parse)?,
            None,
            gix_worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            parse,
        );
        Ok(gix_worktree::Stack::from_state_and_ignore_case(
            &self.dir,
            ignore_case,
            gix_worktree::stack::State::for_add(self.attributes(Source::WorktreeThenIdMapping)?, ignore),
            index,
            index.path_backing(),
        ))
    }

    /// Switch `index`, the index of the case's repository, from the tree of `HEAD` to that of `new` as git did, and return
    /// the outcome. The outer result fails if the switch can't be set up.
    fn switch(&self, index: &mut State) -> TestResult<gix_error::Result<Outcome>> {
        let (old_tree, new_tree) = self.trees()?;
        self.switch_trees(
            index,
            old_tree.as_deref(),
            &new_tree,
            self.objects()?,
            &AtomicBool::new(false),
        )
    }

    /// Switch `index` from `old_tree` to `new_tree` with the objects of `objects`, from the directory in which git ran, with
    /// `should_interrupt` as the flag that interrupts the switch.
    fn switch_trees<O>(
        &self,
        index: &mut State,
        old_tree: Option<&oid>,
        new_tree: &oid,
        objects: O,
        should_interrupt: &AtomicBool,
    ) -> TestResult<gix_error::Result<Outcome>>
    where
        O: gix_object::Find + gix_object::Exists + Send + Clone,
    {
        let options = self.options()?;
        let git_dir_realpath = gix_path::realpath(self.git_dir())?;
        let current_dir = self.current_dir()?;
        let ctx = Context {
            stack: self.stack(index, options.checkout.fs.ignore_case)?,
            git_dir_realpath: &git_dir_realpath,
            current_dir: &current_dir,
            should_interrupt,
        };
        Ok(switch(index, &self.dir, old_tree, new_tree, objects, ctx, options))
    }
}

/// The filters that git applies in the repositories of the fixtures, whose configuration asks for none, so that only
/// those of their attributes apply.
fn filter() -> gix_filter::Pipeline {
    gix_filter::Pipeline::new(Default::default(), gix_testtools::object_hash(), Default::default())
}

/// Copy the directory `src` to `dst`, along with its empty directories and its symbolic links.
fn copy_dir(src: &Path, dst: &Path) -> TestResult {
    std::fs::create_dir(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            gix_fs::symlink::create(&std::fs::read_link(&from)?, &to)?;
        } else if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)?;
        } else {
            return Err(format!("{} is neither a file, a directory nor a symbolic link", from.display()).into());
        }
    }
    Ok(())
}

/// The index at `path`.
fn read_index(path: &Path) -> TestResult<State> {
    let file = gix_index::File::at(path, gix_testtools::object_hash(), false, Default::default())?;
    Ok(file.into())
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

/// What `index` holds.
fn contents(index: &State) -> Contents {
    let stats = index.entries().iter().map(|entry| entry.stat);
    (
        records(index).into_iter().zip(stats).collect(),
        index.timestamp(),
        index.tree().cloned(),
    )
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

/// What a path of a worktree holds.
#[derive(Debug, PartialEq, Eq)]
enum Item {
    /// A file, with its content and whether it is executable.
    File(BString, bool),
    /// A symbolic link, with its target.
    Link(PathBuf),
    /// A directory.
    Directory,
    /// The `.git` of a repository in the worktree, whose content isn't listed.
    DotGit,
    /// Something that can't be tracked, such as a named pipe, whose content isn't read.
    Untrackable,
}

/// What the worktree at `dir` holds outside of its own `.git` directory, by path.
fn worktree(dir: &Path) -> TestResult<BTreeMap<BString, Item>> {
    let mut items = BTreeMap::new();
    list_directory(dir, Path::new(""), &mut items)?;
    Ok(items)
}

/// Add what the directory `rela_dir` of the worktree at `root` holds to `items`.
fn list_directory(root: &Path, rela_dir: &Path, items: &mut BTreeMap<BString, Item>) -> TestResult {
    for entry in std::fs::read_dir(root.join(rela_dir))? {
        let entry = entry?;
        let rela_path = rela_dir.join(entry.file_name());
        let file_type = entry.file_type()?;
        let item = if entry.file_name() == ".git" {
            if rela_dir.as_os_str().is_empty() {
                continue;
            }
            Item::DotGit
        } else if file_type.is_symlink() {
            Item::Link(std::fs::read_link(entry.path())?)
        } else if file_type.is_dir() {
            list_directory(root, &rela_path, items)?;
            Item::Directory
        } else if file_type.is_file() {
            Item::File(
                std::fs::read(entry.path())?.into(),
                gix_fs::is_executable(&entry.metadata()?),
            )
        } else {
            Item::Untrackable
        };
        let path = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(&rela_path)?).into_owned();
        items.insert(path, item);
    }
    Ok(())
}

/// What the index and the worktree of a case hold, to assert that a call that changes nothing leaves them as they were.
struct Snapshot {
    index: Contents,
    worktree: BTreeMap<BString, Item>,
}

impl Snapshot {
    /// What `index` and the worktree of `case` hold.
    fn take(case: &Case, index: &State) -> TestResult<Self> {
        Ok(Snapshot {
            index: contents(index),
            worktree: worktree(&case.dir)?,
        })
    }

    /// Assert that `index` and the worktree of `case` hold what they held when the snapshot was taken.
    fn assert_unchanged(&self, case: &Case, index: &State) -> TestResult {
        let name = case.name;
        assert_eq!(contents(index), self.index, "{name}: the index is left as it was");
        assert_eq!(
            worktree(&case.dir)?,
            self.worktree,
            "{name}: the worktree is left as it was"
        );
        Ok(())
    }
}

/// What a switch did if it happened, or the test fails.
fn switched(outcome: Outcome) -> TestResult<Switched> {
    match outcome {
        Outcome::Switched(switched) => Ok(switched),
        Outcome::Refused(refusals) => Err(format!("the switch is refused: {refusals:?}").into()),
    }
}

/// The paths that a switch refused, or the test fails if it happened.
fn refused(outcome: Outcome) -> TestResult<Refusals> {
    match outcome {
        Outcome::Refused(refusals) => Ok(refusals),
        Outcome::Switched(switched) => Err(format!("the switch happened: {switched:?}").into()),
    }
}

/// The paths that a switch refused, in the groups of git's messages.
#[derive(Debug, Default, PartialEq, Eq)]
struct Refused {
    /// Paths with conflicts.
    unmerged: Vec<BString>,
    /// The blocks of paths with local changes, which git lists under the same message, staged changes first.
    local_changes: Vec<Vec<BString>>,
    /// Directories whose untracked files would be lost.
    directories_with_untracked_files: Vec<BString>,
    /// Directories that are the current working directory or contain it.
    current_dir: Vec<BString>,
    /// Untracked files that would be overwritten.
    untracked_overwritten: Vec<BString>,
    /// Untracked files that would be removed.
    untracked_removed: Vec<BString>,
}

/// A group of paths that `git switch` lists under a message of its own.
#[derive(Clone, Copy)]
enum Group {
    LocalChanges,
    DirectoriesWithUntrackedFiles,
    CurrentDir,
    UntrackedOverwritten,
    UntrackedRemoved,
}

/// The messages under which `git switch` lists the paths of each group.
const MESSAGES: [(&str, Group); 5] = [
    (
        "error: Your local changes to the following files would be overwritten by checkout:",
        Group::LocalChanges,
    ),
    (
        "error: Updating the following directories would lose untracked files in them:",
        Group::DirectoriesWithUntrackedFiles,
    ),
    (
        "error: Refusing to remove the current working directory:",
        Group::CurrentDir,
    ),
    (
        "error: The following untracked working tree files would be overwritten by checkout:",
        Group::UntrackedOverwritten,
    ),
    (
        "error: The following untracked working tree files would be removed by checkout:",
        Group::UntrackedRemoved,
    ),
];

impl Refused {
    /// The paths of `refusals`, whose staged and unstaged changes are the blocks of local changes, unless they are empty.
    fn of(refusals: &Refusals) -> Self {
        Refused {
            unmerged: refusals.unmerged.clone(),
            local_changes: [&refusals.staged_changes, &refusals.unstaged_changes]
                .into_iter()
                .filter(|paths| !paths.is_empty())
                .cloned()
                .collect(),
            directories_with_untracked_files: refusals.directories_with_untracked_files.clone(),
            current_dir: refusals.current_dir.clone(),
            untracked_overwritten: refusals.untracked_overwritten.clone(),
            untracked_removed: refusals.untracked_removed.clone(),
        }
    }

    /// The paths that git refused, as it printed them in `error`. `git switch` lists the paths of each group under a
    /// message of its own, and those with conflicts as needing a merge, while `git read-tree` names the first path that
    /// it refuses in a sentence. As the order in which git finds the paths of a group isn't that of their names, they
    /// are sorted.
    fn parse(error: &str) -> TestResult<Self> {
        let mut refused = Refused::default();
        let mut group = None;
        for line in error.lines() {
            if let Some(path) = line.strip_prefix('\t') {
                let group = group.ok_or_else(|| format!("a path follows no message in {error:?}"))?;
                refused.paths(group).push(path.into());
            } else if line.is_empty() || line == "Aborting" {
                group = None;
            } else if let Some(&(_, message_group)) = MESSAGES.iter().find(|(message, _)| *message == line) {
                if matches!(message_group, Group::LocalChanges) {
                    refused.local_changes.push(Vec::new());
                }
                group = Some(message_group);
            } else if let Some(path) = line.strip_suffix(": needs merge") {
                refused.unmerged.push(path.into());
            } else if let Some(path) = quoted(line, "error: Entry '", "' would be overwritten by merge. Cannot merge.")
            {
                refused.local_changes.push(vec![path.into()]);
            } else if let Some(path) = quoted(
                line,
                "error: Untracked working tree file '",
                "' would be overwritten by merge.",
            ) {
                refused.untracked_overwritten.push(path.into());
            } else if line != "error: you need to resolve your current index first" {
                return Err(format!("git printed an unexpected line: {line:?}").into());
            }
        }
        let Refused {
            unmerged,
            local_changes,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        } = &mut refused;
        for paths in local_changes.iter_mut().chain([
            unmerged,
            directories_with_untracked_files,
            current_dir,
            untracked_overwritten,
            untracked_removed,
        ]) {
            paths.sort();
        }
        Ok(refused)
    }

    /// The paths of `group`, of which those with local changes are in the last block.
    fn paths(&mut self, group: Group) -> &mut Vec<BString> {
        match group {
            Group::LocalChanges => self
                .local_changes
                .last_mut()
                .expect("a block of local changes begins with its message"),
            Group::DirectoriesWithUntrackedFiles => &mut self.directories_with_untracked_files,
            Group::CurrentDir => &mut self.current_dir,
            Group::UntrackedOverwritten => &mut self.untracked_overwritten,
            Group::UntrackedRemoved => &mut self.untracked_removed,
        }
    }
}

/// The path that `line` quotes between `before` and `after`, if it is that sentence.
fn quoted<'a>(line: &'a str, before: &str, after: &str) -> Option<&'a str> {
    line.strip_prefix(before)?.strip_suffix(after)
}

/// Switch a copy of `case` and assert that this leaves what git left. If git switched, the index holds the entries and
/// the worktree the files that git left, the outcome counts the entries that changed and lists the files of the removed
/// entries at `unremoved` as those that couldn't be removed, and wherever the cache tree of git is valid, so is that of
/// the switch. If git refused to switch, the same paths are refused, and the index and the worktree are left as they
/// were.
fn assert_matches_git(case: &Case, unremoved: &[&str]) -> TestResult<Outcome> {
    let name = case.name;
    let (case, _tmp) = case.writable_copy()?;
    let before = case.index()?;
    let snapshot = Snapshot::take(&case, &before)?;
    let mut index = before.clone();
    let outcome = case.switch(&mut index)??;
    match (&outcome, case.git_error()?) {
        (Outcome::Switched(switched), None) => {
            assert_switched_as_git(&case, &before, &index, switched)?;
            let paths: Vec<String> = switched
                .unremoved
                .iter()
                .map(|unremoved| unremoved.path.to_string())
                .collect();
            assert_eq!(
                paths, unremoved,
                "{name}: only the files of these removed entries couldn't be removed"
            );
        }
        (Outcome::Refused(refusals), Some(error)) => {
            assert_eq!(
                Refused::of(refusals),
                Refused::parse(&error)?,
                "{name}: the paths that git refused are refused"
            );
            snapshot.assert_unchanged(&case, &index)?;
        }
        (outcome, error) => {
            return Err(format!("{name}: git and the switch disagree about switching: {outcome:?}, {error:?}").into());
        }
    }
    Ok(outcome)
}

/// Assert that `index`, which the switch of `case` from `before` left with the outcome `switched`, and the worktree of
/// `case` hold what git left, that the outcome counts the entries that changed and that all files were checked out.
fn assert_switched_as_git(case: &Case, before: &State, index: &State, switched: &Switched) -> TestResult {
    let name = case.name;
    let git = case.git_index()?;
    assert_eq!(
        records(index),
        records(&git),
        "{name}: the entries are those that git left"
    );
    assert_eq!(
        worktree(&case.dir)?,
        worktree(&case.git)?,
        "{name}: the worktree holds what git left in it"
    );
    assert_eq!(
        (switched.added, switched.modified, switched.removed),
        changes(before, index),
        "{name}: the outcome counts the entries that were added, modified and removed"
    );
    let checkout = &switched.checkout;
    assert!(
        checkout.errors.is_empty() && checkout.collisions.is_empty() && switched.checkout_error.is_none(),
        "{name}: all files were checked out: {switched:?}"
    );
    assert_cache_tree_is_valid(case, index)?;
    assert_cache_tree_covers_git(name, index.tree(), git.tree());
    Ok(())
}

/// Assert that wherever the cache tree of `index`, an index of `case`, is valid, it names the trees that its entries make.
fn assert_cache_tree_is_valid(case: &Case, index: &State) -> TestResult {
    let trees = gix_odb::memory::Proxy::new(case.objects()?, gix_testtools::object_hash());
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

/// Assert that the cache tree `ours` is valid wherever the cache tree `git` that git left is valid, with the same tree and
/// the same number of entries.
fn assert_cache_tree_covers_git(name: &str, ours: Option<&Tree>, git: Option<&Tree>) {
    let Some(git) = git else {
        return;
    };
    let mut nodes = vec![(BString::default(), git, ours)];
    while let Some((path, git, ours)) = nodes.pop() {
        if git.num_entries.is_some() {
            assert_eq!(
                ours.map(|ours| (ours.num_entries, ours.id)),
                Some((git.num_entries, git.id)),
                "{name}: the cache tree is valid at '{path}' with the tree that git recorded"
            );
        }
        for child in &git.children {
            let ours = ours.and_then(|ours| ours.children.iter().find(|ours| ours.name == child.name));
            let mut child_path = path.clone();
            if !child_path.is_empty() {
                child_path.push(b'/');
            }
            child_path.extend_from_slice(&child.name);
            nodes.push((child_path, child, ours));
        }
    }
}
