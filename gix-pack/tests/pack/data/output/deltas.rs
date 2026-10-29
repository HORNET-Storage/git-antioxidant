//! [`Mode::PackCopyAndDeltaSearch`](iter_from_counts::Mode::PackCopyAndDeltaSearch) is compared with the packs that
//! `git pack-objects --revs --thin --stdout --delta-base-offset` writes with one thread for the pushes of
//! `make_deltas.sh`, from the counts that [`revisions::objects()`] finds for the same revisions: the packs hold the same
//! objects in the same order, store the same objects as deltas of the same bases with the same bytes, and are indexed by
//! `gix-pack` and by `git index-pack --fix-thin`. The packs that several threads write hold the same objects and are
//! indexed as well.
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
};

use gix_hash::ObjectId;
use gix_pack::data::{
    decode::entry::ResolvedBase,
    entry::Header,
    output::{self, count::revisions, entry::iter_from_counts},
};
use gix_parallel::{Finalize, InOrderIter};
use gix_testtools::TestResult;
use gix_utils::progress;

/// The packs of `thin/` in the fixture, made from its loose objects and from the pack of `packed.git`.
const PACKS: &[&str] = &[
    "loose-edits",
    "loose-last",
    "loose-all",
    "packed-edits",
    "packed-last",
    "packed-all",
];

/// An entry of a pack.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    id: ObjectId,
    /// The object that the entry stores its object as a delta of and the delta, or `None` if it stores it whole.
    delta: Option<(ObjectId, Vec<u8>)>,
}

/// A pack of `thin/` and the repository that Git wrote it from.
struct Case {
    name: &'static str,
    /// The directory of the fixture.
    dir: PathBuf,
    /// The repository, in which Git runs.
    repo: PathBuf,
    objects: gix_odb::HandleArc,
}

impl Case {
    fn new(dir: &Path, name: &'static str) -> TestResult<Self> {
        let (repo, objects_dir) = if name.starts_with("packed") {
            let repo = dir.join("packed.git");
            let objects_dir = repo.join("objects");
            (repo, objects_dir)
        } else {
            (dir.to_owned(), dir.join(".git").join("objects"))
        };
        let store = gix_odb::Store::at_opts(
            objects_dir,
            gix_testtools::object_hash(),
            &mut None.into_iter(),
            Default::default(),
        )?;
        let mut objects = Arc::new(store).to_cache_arc();
        objects.prevent_pack_unload();
        Ok(Case {
            name,
            dir: dir.to_owned(),
            repo,
            objects,
        })
    }

    /// The objects to send and those that the receiver has, as `<name>.revs` lists them.
    fn revisions(&self) -> TestResult<(Vec<ObjectId>, Vec<ObjectId>)> {
        let (mut tips, mut haves) = (Vec::new(), Vec::new());
        let revisions = std::fs::read_to_string(self.dir.join("thin").join(format!("{}.revs", self.name)))?;
        for line in revisions.lines() {
            match line.strip_prefix('^') {
                Some(have) => haves.push(ObjectId::from_hex(have.as_bytes())?),
                None => tips.push(ObjectId::from_hex(line.as_bytes())?),
            }
        }
        Ok((tips, haves))
    }

    /// The pack that Git wrote.
    fn git_pack(&self) -> TestResult<Vec<u8>> {
        Ok(std::fs::read(
            self.dir.join("thin").join(format!("{}.pack", self.name)),
        )?)
    }

    /// Count the objects of the revisions as a push does, with the preferred bases of a thin pack and the objects of the
    /// tags marked as Git's `mark_tagged()` marks them, and write them into a pack with `options`.
    fn write_pack(&self, options: iter_from_counts::Options) -> TestResult<(Vec<u8>, iter_from_counts::Outcome)> {
        let should_interrupt = AtomicBool::new(false);
        let (tips, haves) = self.revisions()?;
        let mut counted = revisions::objects(
            &self.objects,
            tips,
            haves,
            &progress::Discard,
            &should_interrupt,
            revisions::Options { thin: true, window: 10 },
        )?;
        let mut tagged = HashSet::new();
        for id in std::fs::read_to_string(self.dir.join("tags.list"))?.split_whitespace() {
            tagged.insert(ObjectId::from_hex(id.as_bytes())?);
        }
        for count in &mut counted.counts {
            count.tagged = tagged.contains(&count.id);
        }
        let num_entries = u32::try_from(counted.counts.iter().filter(|count| !count.preferred_base).count())?;
        let mut produced = output::entry::iter_from_counts(
            counted.counts,
            self.objects.clone(),
            Box::new(progress::Discard),
            &should_interrupt,
            options,
        )?;
        let entries: Vec<_> = InOrderIter::from(produced.by_ref())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        let stats = produced.finalize()?;
        let mut pack = Vec::new();
        for written in output::bytes::FromEntriesIter::new(
            std::iter::once(Ok::<_, gix_error::Error>(entries)),
            &mut pack,
            num_entries,
            gix_pack::data::Version::V2,
            gix_testtools::object_hash(),
        ) {
            written?;
        }
        Ok((pack, stats))
    }

    /// Index `pack` with `gix-pack`, which takes the bases of thin deltas from the repository, and with
    /// `git index-pack --fix-thin`, which adds them to the pack, and check that both index the objects of `entries`.
    fn assert_indexed(&self, pack: &[u8], entries: &[Entry]) -> TestResult {
        let object_hash = gix_testtools::object_hash();
        let dir = gix_testtools::tempfile::TempDir::new()?;
        let written = gix_pack::Bundle::write_to_directory(
            &mut &*pack,
            Some(dir.path()),
            &mut progress::Discard,
            &AtomicBool::new(false),
            Some(self.objects.clone()),
            object_hash,
            gix_pack::bundle::write::Options::default(),
        )?;
        let index = gix_pack::index::File::at(written.index_path.ok_or("gix-pack wrote an index")?, object_hash)?;
        for entry in entries {
            assert!(
                index.lookup(entry.id).is_some(),
                "{}: gix-pack indexes {}",
                self.name,
                entry.id
            );
        }

        let fixed = dir.path().join("fixed.pack");
        let mut git = gix_testtools::git_command(&self.repo)
            .args(["index-pack", "--stdin", "--fix-thin"])
            .arg(&fixed)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        git.stdin.take().ok_or("git reads the pack")?.write_all(pack)?;
        let output = git.wait_with_output()?;
        assert!(
            output.status.success(),
            "{}: git index-pack --fix-thin indexes the pack: {}",
            self.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let index = gix_pack::index::File::at(fixed.with_extension("idx"), object_hash)?;
        for entry in entries {
            assert!(
                index.lookup(entry.id).is_some(),
                "{}: git indexes {}",
                self.name,
                entry.id
            );
        }
        Ok(())
    }
}

/// The entries of `pack` in their order, with the bases of the deltas that refer to objects outside of it read from
/// `objects`.
fn entries(pack: &[u8], objects: &gix_odb::HandleArc) -> TestResult<Vec<Entry>> {
    let object_hash = gix_testtools::object_hash();
    let dir = gix_testtools::tempfile::TempDir::new()?;
    let path = dir.path().join("read.pack");
    std::fs::write(&path, pack)?;
    let file = gix_pack::data::File::at(&path, object_hash)?;
    let mut inflate = gix_zlib::Inflate::default();
    let (mut entries, mut ids) = (Vec::new(), HashMap::new());
    for entry in file.streaming_iter()? {
        let entry = entry?;
        let mut object = Vec::new();
        let outcome = file.decode_entry(
            file.entry(entry.pack_offset)?,
            &mut object,
            &mut inflate,
            &|id, out| {
                let mut buf = Vec::new();
                Ok(gix_object::Find::try_find(objects, id, &mut buf)?.map(|base| {
                    out.clear();
                    out.extend_from_slice(base.data);
                    ResolvedBase::OutOfPack {
                        kind: base.kind,
                        end: out.len(),
                    }
                }))
            },
            &mut gix_pack::cache::Never,
        )?;
        let id = gix_object::compute_hash(object_hash, outcome.kind, &object)?;
        let pack_entry = file.entry(entry.pack_offset)?;
        let base = match pack_entry.header {
            Header::OfsDelta { base_distance } => Some(
                ids.get(&pack_entry.base_pack_offset(base_distance))
                    .copied()
                    .ok_or("bases come before their offset deltas")?,
            ),
            Header::RefDelta { base_id } => Some(base_id),
            _ => None,
        };
        let delta = match base {
            Some(base) => {
                let mut delta = vec![0; usize::try_from(pack_entry.decompressed_size)?];
                file.decompress_entry(&pack_entry, &mut inflate, &mut delta)?;
                Some((base, delta))
            }
            None => None,
        };
        ids.insert(entry.pack_offset, id);
        entries.push(Entry { id, delta });
    }
    Ok(entries)
}

/// Check that each delta of `entries` comes after its base, unless its base is outside of the pack.
fn assert_bases_come_first(name: &str, entries: &[Entry]) {
    let held: HashSet<_> = entries.iter().map(|entry| entry.id).collect();
    let mut written = HashSet::new();
    for entry in entries {
        if let Some((base, _)) = &entry.delta
            && held.contains(base)
        {
            assert!(
                written.contains(base),
                "{name}: the base of {} comes before it",
                entry.id
            );
        }
        written.insert(entry.id);
    }
}

fn delta_search(thread_limit: Option<usize>) -> iter_from_counts::Options {
    iter_from_counts::Options {
        thread_limit,
        mode: iter_from_counts::Mode::PackCopyAndDeltaSearch(Default::default()),
        allow_thin_pack: true,
        ..Default::default()
    }
}

/// With one thread, the packs hold the objects of the packs that `git pack-objects` writes with one thread, in their
/// order, and store the same objects as deltas of the same bases, with Git's bytes, whether the deltas are computed,
/// copied from a pack, or refer to the preferred bases of a thin pack.
#[test]
fn packs_are_those_of_git_pack_objects_with_one_thread() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    let (mut computed, mut copied, mut thin) = (0, 0, 0);
    for &name in PACKS {
        let case = Case::new(&dir, name)?;
        let (pack, stats) = case.write_pack(delta_search(Some(1)))?;
        let ours = entries(&pack, &case.objects)?;
        let git = entries(&case.git_pack()?, &case.objects)?;
        assert_eq!(
            ours.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            git.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            "{name}: the pack holds the objects of Git's pack in its order"
        );
        for (entry, git_entry) in ours.iter().zip(&git) {
            assert_eq!(
                entry.delta.as_ref().map(|(base, _)| base),
                git_entry.delta.as_ref().map(|(base, _)| base),
                "{name}: the base of {}",
                entry.id
            );
            assert!(
                entry.delta == git_entry.delta,
                "{name}: the delta of {} has Git's bytes",
                entry.id
            );
        }
        assert_bases_come_first(name, &ours);
        case.assert_indexed(&pack, &ours)?;
        assert_eq!(stats.missing_objects, 0, "{name}: every counted object exists");
        computed += stats.computed_delta_objects;
        copied += stats.objects_copied_from_pack;
        thin += stats.ref_delta_objects;
    }
    assert!(computed > 0, "the search finds deltas");
    assert!(copied > 0, "objects are copied from the pack of the clone");
    assert!(thin > 0, "thin packs have deltas of preferred bases");
    Ok(())
}

/// With several threads, which deltas are found depends on the speed of the threads, but the packs hold the same
/// objects, each base before its deltas, and are indexed like those of one thread.
#[test]
fn several_threads_write_packs_of_the_same_objects() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    for &name in PACKS {
        let case = Case::new(&dir, name)?;
        let (one, _) = case.write_pack(delta_search(Some(1)))?;
        let (several, _) = case.write_pack(delta_search(Some(4)))?;
        let one = entries(&one, &case.objects)?;
        let several_entries = entries(&several, &case.objects)?;
        assert_eq!(
            several_entries.iter().map(|entry| entry.id).collect::<HashSet<_>>(),
            one.iter().map(|entry| entry.id).collect::<HashSet<_>>(),
            "{name}: several threads pack the same objects"
        );
        assert_bases_come_first(name, &several_entries);
        case.assert_indexed(&several, &several_entries)?;
    }
    Ok(())
}

/// Without thin packs, the preferred bases are left out: no delta refers to an object outside of the pack, which holds
/// the objects of the thin pack. The mode that doesn't search deltas doesn't write preferred bases either.
#[test]
fn preferred_bases_are_only_used_by_thin_packs_that_search_deltas() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    for name in ["loose-edits", "packed-edits"] {
        let case = Case::new(&dir, name)?;
        let (thin_pack, _) = case.write_pack(delta_search(Some(1)))?;
        let thin = entries(&thin_pack, &case.objects)?;
        for options in [
            iter_from_counts::Options {
                allow_thin_pack: false,
                ..delta_search(Some(1))
            },
            iter_from_counts::Options {
                thread_limit: Some(1),
                ..Default::default()
            },
        ] {
            let (pack, stats) = case.write_pack(options)?;
            let whole = entries(&pack, &case.objects)?;
            assert_eq!(
                whole.iter().map(|entry| entry.id).collect::<HashSet<_>>(),
                thin.iter().map(|entry| entry.id).collect::<HashSet<_>>(),
                "{name}: the pack holds the objects to send, without preferred bases"
            );
            let held: HashSet<_> = whole.iter().map(|entry| entry.id).collect();
            assert!(
                whole
                    .iter()
                    .filter_map(|entry| entry.delta.as_ref())
                    .all(|(base, _)| held.contains(base)),
                "{name}: every delta refers to an object of the pack"
            );
            assert_eq!(stats.ref_delta_objects, 0, "{name}: no delta refers to its base by id");
            case.assert_indexed(&pack, &whole)?;
        }
    }
    Ok(())
}
