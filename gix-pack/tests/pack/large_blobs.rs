//! Imports of packs whose blobs exceed the big-file threshold. The indexes of complete packs must be those that
//! `git -c core.bigFileThreshold=4k index-pack` writes, and the thin pack of a bundle must be completed and indexed as
//! gix-pack completed and indexed it before it streamed large blobs.
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use gix_odb::pack;
use gix_pack::data::input;
use gix_testtools::tempfile::TempDir;

use crate::Result;

/// The `core.bigFileThreshold` that `make_large_blobs.sh` indexes its packs with: its large blobs exceed it, and all
/// its other objects are smaller.
const THRESHOLD: u64 = 4 * 1024;

/// Git's default of `core.bigFileThreshold`, which no object of the fixture exceeds.
const GIT_DEFAULT_THRESHOLD: u64 = 512 * 1024 * 1024;

/// An allocation limit below the 96 KiB of `whole.bin` and its 80 KiB of compressed bytes, above every other object
/// and entry of `whole.pack`, and below the 66 KiB of the new version of `versioned.bin`, which `ofs.pack` and
/// `ref.pack` store as the base of a delta.
const ALLOC_LIMIT: usize = 16 * 1024;

/// The packs that `make_large_blobs.sh` writes, each beside the index that Git writes for it.
const COMPLETE_PACKS: [&str; 3] = ["whole", "ofs", "ref"];

/// Each complete pack is written as it was read and indexed as Git indexes it, whether its large blobs are streamed or
/// not, also when it goes through the completion of thin packs, which writes its entries again.
#[test]
fn complete_packs_are_indexed_as_git_indexes_them() -> Result {
    let fixture = fixture()?;
    for name in COMPLETE_PACKS {
        let (pack_data, git_index) = complete_pack(&fixture, name)?;
        for big_file_threshold in [THRESHOLD, GIT_DEFAULT_THRESHOLD] {
            for thread_limit in [Some(1), None] {
                for eagerly in [false, true] {
                    for thin in [false, true] {
                        let written = import(
                            &pack_data,
                            thin.then_some(gix_object::find::Never),
                            eagerly,
                            options(big_file_threshold, None, thread_limit),
                        )?;
                        let case = format!(
                            "{name}.pack, threshold {big_file_threshold}, {thread_limit:?} threads, eagerly: {eagerly}, thin: {thin}"
                        );
                        assert_eq!(written.index, git_index, "{case}: the index is Git's");
                        assert_eq!(written.pack, pack_data, "{case}: the pack is written as it was read");
                    }
                }
            }
        }
    }
    Ok(())
}

/// The thin pack of the bundle is completed with the base from the object database and indexed byte for byte as
/// before large blobs were streamed, while the large blob `extra.bin` is written into the pack as it is read, behind
/// the entries before it and the base that is inserted among them.
#[test]
fn thin_pack_is_completed_and_indexed_as_before() -> Result {
    let fixture = fixture()?;
    let pack_data = thin_pack(&fixture)?;
    let object_hash = crate::object_hash();
    // What `Bundle::write_to_directory()` wrote with its default options before large blobs were streamed.
    let pack_hash = crate::hex_to_id_for_hash(
        object_hash,
        "39bde62c18ccb44c74d13b603a17d8f508282643",
        "ac63dcdb489365a5ed6df74cef4ab0a509d8fff0cf3223a9be191bb6af8fb9d6",
    );
    let index_hash = crate::hex_to_id_for_hash(
        object_hash,
        "68b575432d6e8265f7b53947290b3889ad19b487",
        "f9246655e9886db666737f2af9c755a9beeb2599924f609d3b631490753a7091",
    );
    let (pack_len, index_len) = match object_hash {
        gix_hash::Kind::Sha1 => (109_509, 1296),
        gix_hash::Kind::Sha256 => (109_638, 1416),
        _ => unimplemented!(),
    };
    for big_file_threshold in [THRESHOLD, GIT_DEFAULT_THRESHOLD] {
        for thread_limit in [Some(1), None] {
            for eagerly in [false, true] {
                let written = import(
                    &pack_data,
                    Some(objects(&fixture)?),
                    eagerly,
                    options(big_file_threshold, None, thread_limit),
                )?;
                let case = format!("threshold {big_file_threshold}, {thread_limit:?} threads, eagerly: {eagerly}");
                assert_eq!(
                    written.outcome.index.data_hash, pack_hash,
                    "{case}: the pack is the same"
                );
                assert_eq!(written.pack.len(), pack_len, "{case}: the pack has the same size");
                assert!(
                    written.pack.ends_with(pack_hash.as_slice()),
                    "{case}: the pack ends with its hash"
                );
                assert_eq!(
                    written.outcome.index.index_hash, index_hash,
                    "{case}: the index is the same"
                );
                assert_eq!(written.index.len(), index_len, "{case}: the index has the same size");
                assert_eq!(
                    written.outcome.index.num_objects, 8,
                    "{case}: the seven objects of the thin pack and the base that completes it"
                );
            }
        }
    }
    Ok(())
}

/// With an allocation limit below the size of `whole.bin` and of its compressed bytes, the import succeeds and the index
/// is Git's: the indexer never decompresses the blob into a buffer of its size, and its compressed bytes aren't kept
/// either, as the input iteration would keep them in a buffer under the limit when it completes a thin pack, which
/// keeps the compressed bytes of all other entries.
#[test]
fn a_blob_above_the_threshold_and_the_alloc_limit_is_indexed() -> Result {
    let fixture = fixture()?;
    let (pack_data, git_index) = complete_pack(&fixture, "whole")?;
    for thread_limit in [Some(1), None] {
        for eagerly in [false, true] {
            for thin in [false, true] {
                let written = import(
                    &pack_data,
                    thin.then_some(gix_object::find::Never),
                    eagerly,
                    options(THRESHOLD, Some(ALLOC_LIMIT), thread_limit),
                )?;
                let case = format!("{thread_limit:?} threads, eagerly: {eagerly}, thin: {thin}");
                assert_eq!(written.index, git_index, "{case}: the index is Git's");
                assert_eq!(written.pack, pack_data, "{case}: the pack is written as it was read");
            }
        }
    }
    Ok(())
}

/// A large blob that is the base of a delta is decompressed as a whole to resolve the delta, as Git loads it as a whole,
/// so that an allocation limit below its size refuses it.
#[test]
fn a_large_blob_that_is_the_base_of_a_delta_is_refused_above_the_alloc_limit() -> Result {
    let fixture = fixture()?;
    for name in ["ofs", "ref"] {
        let (pack_data, _git_index) = complete_pack(&fixture, name)?;
        for thin in [false, true] {
            let directory = TempDir::new()?;
            let err = write(
                &pack_data,
                directory.path(),
                thin.then_some(gix_object::find::Never),
                false,
                options(THRESHOLD, Some(ALLOC_LIMIT), Some(1)),
            )
            .expect_err("the base of the delta needs a buffer above the limit");
            assert_eq!(
                err.classify().find_map(|classification| match classification.class() {
                    gix_error::Class::ResourceExhaustion(kind) => Some(kind),
                    _ => None,
                }),
                Some(gix_error::ResourceExhaustionKind::AllocationLimit),
                "{name}.pack, thin: {thin}: the limit refuses the buffer"
            );
        }
    }
    Ok(())
}

/// Entries that come without the ids of their large blobs, as an iteration without a big-file threshold yields them, are
/// indexed as Git indexes them: the index writer hashes each such blob while it inflates it in chunks, which the
/// allocation limit below the size of `whole.bin` shows.
#[test]
fn the_index_writer_hashes_large_blobs_without_ids_in_chunks() -> Result {
    let fixture = fixture()?;
    let object_hash = crate::object_hash();
    for (name, alloc_limit_bytes) in [("whole", Some(ALLOC_LIMIT)), ("ofs", None), ("ref", None)] {
        let (pack_data, git_index) = complete_pack(&fixture, name)?;
        for thread_limit in [Some(1), None] {
            let mut entries = input::BytesToEntriesIter::new_from_header(
                &pack_data[..],
                input::Mode::Verify,
                input::EntryDataMode::Crc32,
                object_hash,
            )?;
            let pack_version = entries.version();
            let mut index = Vec::new();
            pack::index::write_data_iter_to_stream(
                pack::index::Version::default(),
                || Ok((resolve_entry, PackData(pack_data.clone()))),
                &mut entries,
                thread_limit,
                &mut gix_utils::progress::Discard,
                &mut index,
                &AtomicBool::new(false),
                object_hash,
                alloc_limit_bytes,
                THRESHOLD,
                pack_version,
            )?;
            assert_eq!(
                index, git_index,
                "{name}.pack, {thread_limit:?} threads: the index is Git's"
            );
        }
    }
    Ok(())
}

/// The input iteration hashes each blob above the threshold while it inflates it, and instead of keeping its compressed
/// bytes it writes its entry to the output of large blobs as it reads it. It keeps the compressed bytes of all other
/// entries. The CRC32 of each entry and the id of each large blob are those of Git's index.
#[test]
fn input_iteration_hashes_large_blobs_and_writes_them_as_it_reads_them() -> Result {
    let fixture = fixture()?;
    let object_hash = crate::object_hash();
    for (name, num_large_blobs) in [("whole", 1), ("ofs", 3), ("ref", 3)] {
        let path = fixture.join("packs").join(format!("{name}.pack"));
        let pack_data = std::fs::read(&path)?;
        let git_entries: BTreeMap<_, _> = pack::index::File::at(path.with_extension("idx"), object_hash)?
            .iter()
            .map(|entry| (entry.pack_offset, entry))
            .collect();
        let large_blobs = SharedBuffer::default();
        let entries = input::BytesToEntriesIter::new_from_header(
            &pack_data[..],
            input::Mode::Verify,
            input::EntryDataMode::KeepAndCrc32,
            object_hash,
        )?
        .with_big_file_threshold(THRESHOLD)
        .with_large_blobs_written_to(Box::new(large_blobs.clone()));
        let mut expected_large_blobs = Vec::new();
        let mut seen_large_blobs = 0;
        for entry in entries {
            let entry = entry?;
            let git_entry = &git_entries[&entry.pack_offset];
            let start = entry.pack_offset as usize;
            let bytes = &pack_data[start..start + entry.bytes_in_pack() as usize];
            assert_eq!(
                entry.crc32, git_entry.crc32,
                "{name}.pack: the CRC32 of each entry is Git's"
            );
            if entry.header == pack::data::entry::Header::Blob && entry.decompressed_size > THRESHOLD {
                seen_large_blobs += 1;
                assert_eq!(entry.id, Some(git_entry.oid), "{name}.pack: a large blob is hashed");
                assert_eq!(entry.compressed, None, "{name}.pack: its compressed bytes aren't kept");
                expected_large_blobs.extend_from_slice(bytes);
            } else {
                assert_eq!(entry.id, None, "{name}.pack: other objects aren't hashed");
                assert_eq!(
                    entry.compressed.as_deref(),
                    Some(&bytes[usize::from(entry.header_size)..]),
                    "{name}.pack: the compressed bytes of other entries are kept"
                );
            }
        }
        assert_eq!(seen_large_blobs, num_large_blobs, "{name}.pack");
        assert_eq!(
            *large_blobs.0.lock().expect("not poisoned"),
            expected_large_blobs,
            "{name}.pack: the entries of the large blobs were written as they were read"
        );
    }
    Ok(())
}

/// The declared size of a large blob is checked by the input iteration that hashes it, and by the index writer that
/// hashes it if its entry comes without its id.
#[test]
fn declared_sizes_of_large_blobs_are_checked() -> Result {
    let object_hash = crate::object_hash();
    let data = vec![b'x'; 2 * THRESHOLD as usize];
    for declared_size in [data.len() as u64 - 1, data.len() as u64 + 1] {
        let (pack_data, entry) = pack_of_one_blob(&data, declared_size)?;
        let err = input::BytesToEntriesIter::new_from_header(
            &pack_data[..],
            input::Mode::Verify,
            input::EntryDataMode::Crc32,
            object_hash,
        )?
        .with_big_file_threshold(THRESHOLD)
        .next()
        .expect("one entry")
        .expect_err("the blob doesn't have its declared size");
        assert!(
            err.is_corrupted(),
            "the input iteration finds the pack corrupt: {err:?}"
        );

        let err = pack::index::write_data_iter_to_stream(
            pack::index::Version::default(),
            || Ok((resolve_entry, PackData(pack_data.clone()))),
            &mut std::iter::once(Ok::<_, gix_error::Error>(entry)),
            Some(1),
            &mut gix_utils::progress::Discard,
            &mut Vec::new(),
            &AtomicBool::new(false),
            object_hash,
            None,
            THRESHOLD,
            pack::data::Version::V2,
        )
        .expect_err("the blob doesn't have its declared size");
        assert!(err.is_corrupted(), "the index writer finds the pack corrupt: {err:?}");
    }
    Ok(())
}

/// An entry without compressed bytes must have been written into the output of the pack while it was read.
#[test]
fn entries_without_compressed_bytes_must_have_been_written_already() -> Result {
    let (_pack_data, entry) = pack_of_one_blob(b"data", 4)?;
    let err = input::EntriesToBytesIter::new(
        std::iter::once(Ok::<_, gix_error::Error>(entry)),
        std::io::Cursor::new(Vec::new()),
        pack::data::Version::V2,
        crate::object_hash(),
    )
    .next()
    .expect("one entry")
    .expect_err("the entry wasn't written");
    let message = format!("{err:?}");
    assert!(message.contains("has no compressed bytes"), "{message}");
    Ok(())
}

fn fixture() -> Result<PathBuf> {
    gix_testtools::scripted_fixture_read_only("make_large_blobs.sh")
}

/// The data of the pack `name` of the fixture, and the index that `git index-pack` wrote for it.
fn complete_pack(fixture: &Path, name: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let path = fixture.join("packs").join(format!("{name}.pack"));
    Ok((std::fs::read(&path)?, std::fs::read(path.with_extension("idx"))?))
}

/// The thin pack of `thin.bundle`, which follows the empty line that ends the header of the bundle.
fn thin_pack(fixture: &Path) -> Result<Vec<u8>> {
    let bundle = std::fs::read(fixture.join("thin.bundle"))?;
    let header_end = bundle
        .windows(2)
        .position(|window| window == b"\n\n")
        .ok_or("a bundle header ends with an empty line")?;
    Ok(bundle[header_end + 2..].to_vec())
}

/// The object database of the fixture, which holds the base that completes the thin pack.
fn objects(fixture: &Path) -> Result<impl gix_object::Find + Send + 'static> {
    Ok(gix_odb::at(fixture.join(".git").join("objects"), crate::object_hash())?.into_arc()?)
}

fn options(
    big_file_threshold: u64,
    alloc_limit_bytes: Option<usize>,
    thread_limit: Option<usize>,
) -> pack::bundle::write::Options {
    pack::bundle::write::Options {
        thread_limit,
        alloc_limit_bytes,
        big_file_threshold,
        ..Default::default()
    }
}

/// What an import wrote.
struct Written {
    outcome: pack::bundle::write::Outcome,
    pack: Vec<u8>,
    index: Vec<u8>,
}

/// Import `pack_data` into a directory of its own, as [`write()`] does, and read what it wrote.
fn import(
    pack_data: &[u8],
    lookup: Option<impl gix_object::Find + Send + 'static>,
    eagerly: bool,
    options: pack::bundle::write::Options,
) -> Result<Written> {
    let directory = TempDir::new()?;
    let outcome = write(pack_data, directory.path(), lookup, eagerly, options)?;
    let pack = std::fs::read(outcome.data_path.as_ref().ok_or("a pack was written")?)?;
    let index = std::fs::read(outcome.index_path.as_ref().ok_or("an index was written")?)?;
    Ok(Written { outcome, pack, index })
}

/// Import `pack_data` into `directory` with [`pack::Bundle::write_to_directory_eagerly()`] if `eagerly` is set, or else
/// with [`pack::Bundle::write_to_directory()`], completing it as a thin pack with `lookup` if that is set.
fn write(
    pack_data: &[u8],
    directory: &Path,
    lookup: Option<impl gix_object::Find + Send + 'static>,
    eagerly: bool,
    options: pack::bundle::write::Options,
) -> gix_error::Result<pack::bundle::write::Outcome> {
    static SHOULD_INTERRUPT: AtomicBool = AtomicBool::new(false);
    if eagerly {
        pack::Bundle::write_to_directory_eagerly(
            Box::new(std::io::Cursor::new(pack_data.to_vec())),
            Some(pack_data.len() as u64),
            Some(directory),
            &mut gix_utils::progress::Discard,
            &SHOULD_INTERRUPT,
            lookup,
            crate::object_hash(),
            options,
        )
    } else {
        pack::Bundle::write_to_directory(
            &mut &pack_data[..],
            Some(directory),
            &mut gix_utils::progress::Discard,
            &SHOULD_INTERRUPT,
            lookup,
            crate::object_hash(),
            options,
        )
    }
}

/// A pack of one blob with `data`, whose entry declares `declared_size` bytes, and the entry that an iteration that
/// doesn't keep compressed bytes or compute ids would yield for it.
fn pack_of_one_blob(data: &[u8], declared_size: u64) -> Result<(Vec<u8>, input::Entry)> {
    let header = pack::data::entry::Header::Blob;
    let mut pack_data = pack::data::header::encode(pack::data::Version::V2, 1).to_vec();
    let header_size = header.write_to(declared_size, &mut pack_data)?;
    let compressed = {
        let mut out = gix_zlib::stream::deflate::Write::new(Vec::new(), gix_zlib::Compression::BEST_SPEED);
        out.write_all(data)?;
        out.flush()?;
        out.into_inner()
    };
    pack_data.extend_from_slice(&compressed);
    let mut hasher = gix_hash::hasher(crate::object_hash());
    hasher.update(&pack_data);
    let trailer = hasher.try_finalize()?;
    pack_data.extend_from_slice(trailer.as_slice());
    let entry = input::Entry {
        header,
        header_size: u16::try_from(header_size)?,
        pack_offset: pack::data::header::SIZE as u64,
        compressed: None,
        compressed_size: compressed.len() as u64,
        crc32: Some(0),
        decompressed_size: declared_size,
        trailer: Some(trailer),
        id: None,
    };
    Ok((pack_data, entry))
}

/// The data of a pack, from which [`resolve_entry()`] provides the bytes of its entries.
struct PackData(Vec<u8>);

fn resolve_entry(range: pack::data::EntryRange, pack: &PackData) -> Option<&[u8]> {
    pack.0.get(range.start as usize..range.end as usize)
}

/// A buffer that the input iteration writes the entries of large blobs to, and that the test reads afterwards.
#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("not poisoned").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
