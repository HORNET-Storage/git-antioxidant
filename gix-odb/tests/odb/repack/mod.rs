//! The fixture records what `git repack -d -l --geometric=2`, the repack of the `geometric-repack` task of
//! `git maintenance`, does with a copy of each of its cases: which packs remained, which objects the packs that git wrote
//! hold, and which loose objects and files of multi-pack-indices remained. What [`gix_odb::repack::repack()`] does with
//! another copy of a case is compared with these, and checked with `git fsck` and `git verify-pack`. The fixture also
//! records whether `git maintenance run --auto` repacks some of the cases, which [`gix_odb::repack::needs_repack()`] is
//! compared with.

mod auto;
mod deltas;
mod deviation;
mod git;
mod objects;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_hash::ObjectId;
use gix_odb::repack::{Options, Outcome, Survey};
use gix_testtools::{TestResult, tempfile::TempDir};
use gix_utils::progress;

/// The script that makes the cases and records what git does with them.
const FIXTURE: &str = "make_repack.sh";

/// A copy of a case of the fixture, which the test may change.
struct Case {
    /// The name of the case, which is also the name of the worktree of its repository.
    name: &'static str,
    /// The copy of the fixture, with every case and the records.
    tmp: TempDir,
}

impl Case {
    /// A copy of the case `name`.
    fn writable(name: &'static str) -> TestResult<Self> {
        Ok(Case {
            name,
            tmp: gix_testtools::scripted_fixture_writable(FIXTURE)?,
        })
    }

    /// The worktree of the case's repository.
    fn worktree(&self) -> PathBuf {
        self.tmp.path().join(self.name)
    }

    /// The object database of the case's repository.
    fn objects_dir(&self) -> PathBuf {
        self.worktree().join(".git").join("objects")
    }

    /// The pack directory of the case's repository.
    fn pack_dir(&self) -> PathBuf {
        self.objects_dir().join("pack")
    }

    /// The lines of the file `name` of the fixture.
    fn fixture_file(&self, name: &str) -> TestResult<Vec<String>> {
        Ok(std::fs::read_to_string(self.tmp.path().join(name))?
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }

    /// The lines of the record `kind` of the case, like `packs-after` for `<case>.packs-after`.
    fn record(&self, kind: &str) -> TestResult<Vec<String>> {
        self.fixture_file(&format!("{}.{kind}", self.name))
    }

    /// Repack the object database of the case with `options`.
    fn repack(&self, options: Options) -> TestResult<Outcome> {
        Ok(gix_odb::repack::repack(
            &self.objects_dir(),
            gix_testtools::object_hash(),
            &mut progress::Discard,
            &AtomicBool::new(false),
            options,
        )?)
    }

    /// The names of the pack indices, as `packs <repository>` of the fixture lists them.
    fn packs(&self) -> TestResult<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for entry in std::fs::read_dir(self.pack_dir())? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "idx") {
                names.insert(file_name(&path)?);
            }
        }
        Ok(names)
    }

    /// The paths of the pack indices.
    fn pack_indices(&self) -> TestResult<Vec<PathBuf>> {
        let pack_dir = self.pack_dir();
        Ok(self.packs()?.iter().map(|name| pack_dir.join(name)).collect())
    }

    /// The objects of the packs with the indices `names`, sorted, as the fixture lists the objects of the packs that git
    /// wrote.
    fn objects_of_packs(&self, names: &BTreeSet<String>) -> TestResult<Vec<String>> {
        let mut ids = Vec::new();
        for name in names {
            let index = gix_pack::index::File::at(self.pack_dir().join(name), gix_testtools::object_hash())?;
            ids.extend(index.iter().map(|entry| entry.oid.to_string()));
        }
        ids.sort();
        Ok(ids)
    }

    /// The loose objects, as `loose <repository>` of the fixture lists them: each file in a fanout directory, as the name
    /// of the directory followed by the name of the file.
    fn loose(&self) -> TestResult<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(self.objects_dir())? {
            let path = entry?.path();
            let fanout = file_name(&path)?;
            let is_fanout = fanout.len() == 2 && fanout.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
            if !is_fanout || !path.is_dir() {
                continue;
            }
            for file in std::fs::read_dir(&path)? {
                let file = file?.path();
                if file.is_file() {
                    names.push(format!("{fanout}{}", file_name(&file)?));
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// The path of the loose object `id`.
    fn loose_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_string();
        self.objects_dir().join(&hex[..2]).join(&hex[2..])
    }

    /// Every object of the object database: those of every pack, and the loose objects.
    fn objects(&self) -> TestResult<BTreeSet<ObjectId>> {
        let mut ids = BTreeSet::new();
        for path in self.pack_indices()? {
            ids.extend(
                gix_pack::index::File::at(&path, gix_testtools::object_hash())?
                    .iter()
                    .map(|entry| entry.oid),
            );
        }
        for name in self.loose()? {
            ids.insert(ObjectId::from_hex(name.as_bytes())?);
        }
        Ok(ids)
    }

    /// The files of multi-pack-indices, as `multi_pack_indices <repository>` of the fixture lists them: as paths in the
    /// pack directory.
    fn multi_pack_indices(&self) -> TestResult<Vec<String>> {
        Ok(entries(&self.pack_dir())?
            .into_iter()
            .filter(|(path, size)| {
                size.is_some()
                    && path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| name.starts_with("multi-pack-index"))
            })
            .map(|(path, _size)| path)
            .collect())
    }

    /// Run git with `args` in the worktree of the case, and return its output if it succeeded.
    fn git<I, S>(&self, args: I) -> TestResult<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = gix_testtools::git_command(self.worktree()).args(args).output()?;
        if !output.status.success() {
            return Err(format!(
                "{}: git failed with {}: {}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    /// The depth of every object of the packs with the indices at `paths`, as `git verify-pack -v` reports it: 0 for an
    /// object stored whole, and otherwise the amount of deltas that make it from an object stored whole.
    fn delta_depths(&self, paths: &[PathBuf]) -> TestResult<BTreeMap<ObjectId, u32>> {
        let mut args: Vec<OsString> = vec!["verify-pack".into(), "-v".into()];
        args.extend(paths.iter().map(|path| path.clone().into_os_string()));
        let hex_len = gix_testtools::object_hash().len_in_hex();
        let is_object =
            |id: &str, kind: &str| id.len() == hex_len && matches!(kind, "commit" | "tree" | "blob" | "tag");
        let mut depths = BTreeMap::new();
        for line in self.git(&args)?.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (id, depth) = match fields.as_slice() {
                [id, kind, _size, _packed_size, _offset] if is_object(id, kind) => (id, 0),
                [id, kind, _size, _packed_size, _offset, depth, _base] if is_object(id, kind) => (id, depth.parse()?),
                _ => continue,
            };
            let previous = depths.insert(ObjectId::from_hex(id.as_bytes())?, depth);
            assert_eq!(previous, None, "{}: only one of the packs holds {id}", self.name);
        }
        Ok(depths)
    }

    /// Every file and directory of the object database, as a path in it, with the size of each file.
    fn entries(&self) -> TestResult<BTreeMap<String, Option<u64>>> {
        entries(&self.objects_dir())
    }
}

/// The entries below `dir` as paths in it, with `/` between the names of directories, along with the size of each file
/// and `None` for directories.
fn entries(dir: &Path) -> TestResult<BTreeMap<String, Option<u64>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![(dir.to_owned(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let relative = format!("{prefix}{}", file_name(&path)?);
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                pending.push((path, format!("{relative}/")));
                entries.insert(relative, None);
            } else {
                entries.insert(relative, Some(metadata.len()));
            }
        }
    }
    Ok(entries)
}

/// The name of the file at `path`.
fn file_name(path: &Path) -> TestResult<String> {
    Ok(path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| format!("{} has no name in UTF-8", path.display()))?
        .to_owned())
}

/// The names of the files at `paths`.
fn file_names(paths: &[PathBuf]) -> TestResult<BTreeSet<String>> {
    paths.iter().map(|path| file_name(path)).collect()
}

/// A copy of a case that was repacked with the defaults of `git maintenance`, and what it was like before.
struct Repacked {
    case: Case,
    outcome: Outcome,
    /// The packs before the repack.
    survey_before: Survey,
    /// The names of the pack indices before the repack.
    packs_before: BTreeSet<String>,
    /// The loose objects before the repack.
    loose_before: Vec<String>,
}

/// Repack a copy of the case `name` with the defaults of `git maintenance`, and check what holds after every repack: every
/// object remains, `git fsck` accepts the object database, the directory of the new pack is gone, and the outcome reports
/// the pack that was written and the packs and loose objects that were removed.
fn repack(name: &'static str) -> TestResult<Repacked> {
    let case = Case::writable(name)?;
    let object_hash = gix_testtools::object_hash();
    let survey_before = Survey::at(&case.objects_dir(), object_hash)?;
    let packs_before = case.packs()?;
    assert_eq!(
        packs_before,
        case.record("packs-before")?.into_iter().collect::<BTreeSet<_>>(),
        "{name}: the copy has the packs that git repacked"
    );
    let loose_before = case.loose()?;
    let objects_before = case.objects()?;

    let outcome = case.repack(Options::default())?;

    assert_eq!(case.objects()?, objects_before, "{name}: every object remains");
    case.git(["fsck", "--full", "--no-dangling"])?;
    let packs_after = case.packs()?;
    let removed: BTreeSet<String> = packs_before.difference(&packs_after).cloned().collect();
    assert_eq!(
        file_names(&outcome.removed_packs)?,
        removed,
        "{name}: the removed packs are reported"
    );
    let retained = file_names(&outcome.retained_packs)?;
    assert!(
        retained.is_subset(&packs_before) && retained.is_subset(&packs_after),
        "{name}: the retained packs remain: {retained:?}"
    );
    let added: BTreeSet<String> = packs_after.difference(&packs_before).cloned().collect();
    match &outcome.pack {
        Some(pack) => {
            assert_eq!(
                pack.index_path.parent(),
                Some(case.pack_dir().as_path()),
                "{name}: the new pack is in the pack directory"
            );
            assert_eq!(
                pack.data_path,
                pack.index_path.with_extension("pack"),
                "{name}: the data of the new pack is beside its index"
            );
            assert!(pack.data_path.is_file(), "{name}: the data of the new pack exists");
            assert!(
                !pack.index_path.with_extension("keep").exists(),
                "{name}: the new pack isn't kept"
            );
            assert_eq!(
                gix_pack::index::File::at(&pack.index_path, object_hash)?.num_objects(),
                pack.num_objects,
                "{name}: the new pack holds the reported amount of objects"
            );
            let pack_name = file_name(&pack.index_path)?;
            assert!(packs_after.contains(&pack_name), "{name}: the new pack remains");
            let expected_added = if packs_before.contains(&pack_name) {
                BTreeSet::new()
            } else {
                BTreeSet::from([pack_name])
            };
            assert_eq!(
                added, expected_added,
                "{name}: the new pack was added, unless a pack of its name was there already"
            );
        }
        None => assert_eq!(added, BTreeSet::new(), "{name}: no pack was added"),
    }
    let loose_after = case.loose()?;
    assert_eq!(
        outcome.removed_loose_objects,
        loose_before.len() - loose_after.len(),
        "{name}: the removed loose objects are counted"
    );
    assert_eq!(
        outcome.skipped_loose_objects,
        Vec::<ObjectId>::new(),
        "{name}: every loose object could be read"
    );
    let leftovers: Vec<String> = entries(&case.pack_dir())?
        .into_keys()
        .filter(|path| path.starts_with("tmp_"))
        .collect();
    assert_eq!(
        leftovers,
        Vec::<String>::new(),
        "{name}: the directory of the new pack was removed"
    );
    Ok(Repacked {
        case,
        outcome,
        survey_before,
        packs_before,
        loose_before,
    })
}

impl Repacked {
    /// Check that the same packs and loose objects remain as when git repacked the case, and that the pack that was added
    /// holds the objects of the packs that git added.
    fn assert_packs_and_objects_as_git(&self) -> TestResult {
        let name = self.case.name;
        let packs_after = self.case.packs()?;
        let git_packs_after: BTreeSet<String> = self.case.record("packs-after")?.into_iter().collect();
        assert_eq!(
            packs_after.intersection(&self.packs_before).collect::<Vec<_>>(),
            git_packs_after.intersection(&self.packs_before).collect::<Vec<_>>(),
            "{name}: the packs that git left remain"
        );
        let added: BTreeSet<String> = packs_after.difference(&self.packs_before).cloned().collect();
        assert_eq!(
            added.len(),
            git_packs_after.difference(&self.packs_before).count(),
            "{name}: as many packs were added as git added"
        );
        assert_eq!(
            self.case.objects_of_packs(&added)?,
            self.case.record("new-objects")?,
            "{name}: the added pack holds the objects of the packs that git added"
        );
        assert_eq!(
            self.case.loose()?,
            self.case.record("loose-after")?,
            "{name}: the loose objects that git left remain"
        );
        Ok(())
    }
}
