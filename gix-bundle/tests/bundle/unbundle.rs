use std::{
    io::BufRead,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_bundle::{Header, Reference, Version};
use gix_object::Exists;
use gix_pack::{
    bundle::write::Outcome,
    data::input::{BytesToEntriesIter, EntryDataMode, Mode},
};
use gix_testtools::{TestResult, scripted_fixture_writable};
use gix_utils::progress;

use crate::{FIXTURE, fixture, git_results, ids, objects, open_bundle, revs, tips};

fn pack_directory(dir: &Path, receiver: &str) -> PathBuf {
    dir.join(receiver).join(".git").join("objects").join("pack")
}

/// The sorted names of what `directory` contains.
fn entries(directory: &Path) -> TestResult<Vec<String>> {
    let mut names = std::fs::read_dir(directory)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<TestResult<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Unbundle the bundle `name` of `dir` into `receiver`, whose references point to complete history.
fn unbundle(dir: &Path, receiver: &str, name: &str) -> TestResult<gix_error::Result<Outcome>> {
    let (header, mut pack) = open_bundle(dir, name)?;
    unbundle_into(
        dir,
        receiver,
        &header,
        &mut pack,
        gix_testtools::object_hash(),
        &AtomicBool::new(false),
    )
}

/// Unbundle `header` with the rest of its bundle, `pack`, into `receiver` in `dir`, whose references point to complete
/// history.
pub(crate) fn unbundle_into(
    dir: &Path,
    receiver: &str,
    header: &Header,
    pack: &mut dyn BufRead,
    object_hash: gix_hash::Kind,
    should_interrupt: &AtomicBool,
) -> TestResult<gix_error::Result<Outcome>> {
    Ok(header.unbundle(
        pack,
        &pack_directory(dir, receiver),
        objects(dir, receiver)?,
        object_hash,
        &tips(dir, receiver)?,
        &mut progress::Discard,
        should_interrupt,
        Default::default(),
    ))
}

/// A pack without objects.
fn empty_pack(object_hash: gix_hash::Kind) -> TestResult<Vec<u8>> {
    let mut pack = gix_pack::data::header::encode(gix_pack::data::Version::V2, 0).to_vec();
    let mut hasher = gix_hash::hasher(object_hash);
    hasher.update(&pack);
    pack.extend_from_slice(hasher.try_finalize()?.as_slice());
    Ok(pack)
}

/// Whether an error has the class that a test expects.
type IsExpected = fn(&gix_error::Error) -> bool;

#[test]
fn agrees_with_git_fetch() -> TestResult {
    for git in git_results(&fixture()?)? {
        let dir = scripted_fixture_writable(FIXTURE)?;
        let pack_directory = pack_directory(dir.path(), &git.receiver);
        let before = entries(&pack_directory)?;
        match unbundle(dir.path(), &git.receiver, &git.bundle)? {
            Ok(outcome) => {
                assert!(
                    git.fetch,
                    "{} in {}: git refused the bundle, which was unbundled",
                    git.bundle, git.receiver
                );
                let added: Vec<_> = entries(&pack_directory)?
                    .into_iter()
                    .filter(|name| !before.contains(name))
                    .collect();
                assert_eq!(
                    added.len(),
                    3,
                    "a pack, its index and its keep file were added: {added:?}"
                );
                for path in [&outcome.data_path, &outcome.index_path, &outcome.keep_path] {
                    let path = path.as_deref().expect("a new pack sets all paths");
                    assert_eq!(
                        path.parent(),
                        Some(pack_directory.as_path()),
                        "{} is in the pack directory",
                        path.display()
                    );
                    assert!(path.is_file(), "{} exists", path.display());
                }
                let objects = objects(dir.path(), &git.receiver)?;
                for id in ids(&dir.path().join(format!("{}.objects", git.bundle)))? {
                    assert!(
                        objects.exists(&id),
                        "{} in {}: {id} is present",
                        git.bundle,
                        git.receiver
                    );
                }
            }
            Err(err) => {
                assert!(
                    !git.fetch,
                    "{} in {}: git fetched the bundle, which was refused: {err:?}",
                    git.bundle, git.receiver
                );
                assert_eq!(
                    entries(&pack_directory)?,
                    before,
                    "{} in {}: a refused bundle leaves the pack directory as it was",
                    git.bundle,
                    git.receiver
                );
            }
        }
    }
    Ok(())
}

#[test]
fn tells_why_bundles_are_refused() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let cases: [(&str, &str, IsExpected); 5] = [
        ("empty", "incremental", gix_error::Error::is_not_found),
        ("unconnected", "incremental", gix_error::Error::is_not_found),
        ("has-base", "junk", gix_error::Error::is_corrupted),
        ("has-base", "missing-tree", gix_error::Error::is_corrupted),
        ("has-base", "filtered", gix_error::Error::is_unsupported),
    ];
    for (receiver, bundle, is_expected) in cases {
        let err = unbundle(dir.path(), receiver, bundle)?.expect_err("the bundle is refused");
        assert!(
            is_expected(&err),
            "{bundle} in {receiver}: the refusal has the expected class: {err:?}"
        );
    }
    Ok(())
}

#[test]
fn completes_thin_packs() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let revs = revs(dir.path())?;
    let object_hash = gix_testtools::object_hash();

    let (_header, pack) = open_bundle(dir.path(), "incremental")?;
    let mut external_bases = Vec::new();
    for entry in BytesToEntriesIter::new_from_header(pack, Mode::Verify, EntryDataMode::Ignore, object_hash)? {
        if let gix_pack::data::entry::Header::RefDelta { base_id } = entry?.header {
            external_bases.push(base_id);
        }
    }
    assert!(
        external_bases.contains(&revs.base_large_blob_id),
        "the bundle's pack stores `large` as a delta of the blob of `base`, which it lacks: {external_bases:?}"
    );

    let outcome = unbundle(dir.path(), "has-base", "incremental")??;
    let new_pack = gix_pack::Bundle::at(outcome.index_path.as_deref().expect("a pack was written"), object_hash)?;
    for base_id in &external_bases {
        assert!(
            new_pack.index.lookup(base_id).is_some(),
            "the base {base_id} of a delta was added to the pack"
        );
    }
    let should_interrupt = AtomicBool::new(false);
    new_pack
        .pack
        .verify_checksum(&mut progress::Discard, &should_interrupt)?;
    new_pack
        .index
        .verify_checksum(&mut progress::Discard, &should_interrupt)?;
    let (mut buf, mut inflate) = (Vec::new(), gix_zlib::Inflate::default());
    for entry in new_pack.index.iter() {
        let (object, _location) = new_pack
            .find(&entry.oid, &mut buf, &mut inflate, &mut gix_pack::cache::Never)?
            .expect("indexed objects are present");
        object.verify_checksum(&entry.oid)?;
    }
    Ok(())
}

#[test]
fn keeps_a_pack_that_is_present_already() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let first = unbundle(dir.path(), "has-base", "incremental")??;
    assert!(
        first.keep_path.is_some(),
        "the new pack is protected until references point to it"
    );

    let pack_directory = pack_directory(dir.path(), "has-base");
    let before = entries(&pack_directory)?;
    let second = unbundle(dir.path(), "has-base", "incremental")??;
    assert_eq!(
        second.data_path, first.data_path,
        "the pack that is present already is reported"
    );
    assert_eq!(
        second.index_path, first.index_path,
        "the index of the pack that is present already is reported"
    );
    assert_eq!(second.keep_path, None, "the pack was present already");
    assert_eq!(entries(&pack_directory)?, before, "nothing was added");
    Ok(())
}

#[test]
fn adds_nothing_for_a_bundle_without_objects() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let revs = revs(dir.path())?;
    let object_hash = gix_testtools::object_hash();
    let header = Header {
        version: if object_hash == gix_hash::Kind::Sha1 {
            Version::V2
        } else {
            Version::V3
        },
        object_hash,
        filter: None,
        prerequisites: Vec::new(),
        references: vec![Reference {
            id: revs.base_commit_id,
            name: "refs/tags/base".into(),
        }],
    };
    let pack = empty_pack(object_hash)?;
    let pack_directory = pack_directory(dir.path(), "has-base");
    let before = entries(&pack_directory)?;

    let outcome = unbundle_into(
        dir.path(),
        "has-base",
        &header,
        &mut pack.as_slice(),
        object_hash,
        &AtomicBool::new(false),
    )??;
    assert_eq!(outcome.index.num_objects, 0, "the index lists no objects");
    assert_eq!(
        (outcome.data_path, outcome.index_path, outcome.keep_path),
        (None, None, None),
        "no pack is written"
    );
    assert_eq!(entries(&pack_directory)?, before, "the pack directory is as it was");
    Ok(())
}

#[test]
fn refuses_a_bundle_of_another_object_hash() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let object_hash = gix_testtools::object_hash();
    let other = gix_hash::Kind::all()
        .iter()
        .copied()
        .find(|kind| *kind != object_hash)
        .expect("both object hashes are compiled in");
    let pack_directory = pack_directory(dir.path(), "empty");
    let before = entries(&pack_directory)?;

    let (header, mut pack) = open_bundle(dir.path(), "full")?;
    let err = unbundle_into(dir.path(), "empty", &header, &mut pack, other, &AtomicBool::new(false))?
        .expect_err("the repository uses another object hash");
    assert!(
        err.is_unsupported(),
        "another object hash is refused as unsupported: {err:?}"
    );
    assert_eq!(entries(&pack_directory)?, before, "the pack directory is as it was");
    Ok(())
}

#[test]
fn stops_when_interrupted() -> TestResult {
    let dir = scripted_fixture_writable(FIXTURE)?;
    let pack_directory = pack_directory(dir.path(), "empty");
    let before = entries(&pack_directory)?;

    let (header, mut pack) = open_bundle(dir.path(), "full")?;
    let err = unbundle_into(
        dir.path(),
        "empty",
        &header,
        &mut pack,
        gix_testtools::object_hash(),
        &AtomicBool::new(true),
    )?
    .expect_err("the interrupt is noticed while the pack is read");
    assert!(err.is_cancelled(), "the interrupt is reported as cancellation: {err:?}");
    assert_eq!(entries(&pack_directory)?, before, "the pack directory is as it was");
    Ok(())
}
