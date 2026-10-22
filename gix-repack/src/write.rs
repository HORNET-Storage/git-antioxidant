use std::{
    io::{self, BufWriter},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use gix_error::{Error, Result, ResultExt, bail, cancelled, ensure, message};
use gix_hash::ObjectId;
use gix_object::Find;
use gix_pack::data::output;
use gix_utils::progress::{self, BoxedDynNestedProgress, Count, DynNestedProgress, Progress};

use crate::{
    Options, Written,
    plan::{Location, Plan},
};

/// The most entries that are passed to the pack writer at once.
const ENTRIES_PER_CHUNK: usize = 256;
/// The amount of compressed data after which entries are passed to the pack writer.
const BYTES_PER_CHUNK: usize = 1024 * 1024;

/// Write the objects of `plan` into a new pack of the object database in `objects_dir`, and move it into the pack
/// directory once its index lists exactly these objects.
pub(crate) fn write(
    objects_dir: &Path,
    object_hash: gix_hash::Kind,
    plan: &Plan,
    progress: &mut dyn DynNestedProgress,
    should_interrupt: &AtomicBool,
    options: &Options,
) -> Result<Written> {
    let pack_dir = objects_dir.join("pack");
    match std::fs::create_dir(&pack_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => {
            return Err(err).or_raise(|| message!("Could not create the pack directory {}", pack_dir.display()));
        }
    }
    let staging = Staging::new(&pack_dir)?;

    let mut positions = vec![0; plan.objects.len()];
    for (position, &object) in plan.order.iter().enumerate() {
        positions[object as usize] = position;
    }
    let mut writing = progress.add_child("write pack".into());
    writing.init(None, progress::bytes());
    let producer = Producer {
        objects_dir,
        object_hash,
        plan,
        positions: &positions,
        compression: options.compression,
        should_interrupt,
    };
    let (produced, indexed) = std::thread::scope(|scope| {
        let (writer, mut reader) = gix_utils::io::pipe::unidirectional(16);
        let producing = scope.spawn(move || producer.produce(writer, writing));
        let indexed = gix_pack::Bundle::write_to_directory(
            &mut reader,
            Some(staging.path.as_path()),
            progress,
            should_interrupt,
            None::<gix_object::find::Never>,
            object_hash,
            gix_pack::bundle::write::Options {
                thread_limit: options.thread_limit,
                iteration_mode: gix_pack::data::input::Mode::Verify,
                index_version: gix_pack::index::Version::default(),
                alloc_limit_bytes: None,
                compression: options.compression,
            },
        );
        // If the pack couldn't be indexed, the rest of it isn't read, and the producer must stop waiting to pass it on.
        drop(reader);
        let produced = producing
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (produced, indexed)
    });

    if should_interrupt.load(Ordering::Relaxed) {
        bail!(cancelled("Interrupted while writing the new pack"));
    }
    let outcome = match (produced, indexed) {
        (Ok(()), Ok(outcome)) => outcome,
        // Then the pack ended early, which is why it couldn't be indexed.
        (Err(Failure::Entry(err)), _) => return Err(err),
        (_, Err(err)) => return Err(err).or_raise(|| message("Could not index the new pack")),
        (Err(Failure::Write(err)), Ok(_)) => return Err(err),
    };
    let (Some(index_path), Some(data_path)) = (outcome.index_path, outcome.data_path) else {
        bail!(message("The indexer of the new pack wrote no pack"));
    };
    verify(&index_path, object_hash, plan)?;
    let (index_path, data_path) = move_into(&pack_dir, &index_path, &data_path)?;
    Ok(Written {
        index_path,
        data_path,
        num_objects: plan.objects.len() as u32,
    })
}

/// Check that the index of the new pack lists exactly the objects of `plan`, with the ids that the indexer computed from
/// the data in the pack.
fn verify(index_path: &Path, object_hash: gix_hash::Kind, plan: &Plan) -> Result {
    let index = gix_pack::index::File::at(index_path, object_hash)
        .or_raise(|| message!("Could not read the index of the new pack at {}", index_path.display()))?;
    ensure!(
        index.num_objects() as usize == plan.objects.len(),
        "The new pack holds {} objects instead of {}".corrupted(),
        index.num_objects(),
        plan.objects.len()
    );
    if let Some(object) = plan.objects.iter().find(|object| index.lookup(object.id).is_none()) {
        bail!("The new pack lacks the object {}".corrupted(), object.id);
    }
    Ok(())
}

/// Move the pack at `data_path` with its index at `index_path` into `pack_dir`, the index last so that readers see the
/// pack only once it is complete, unless the same pack is present already. Return the new paths of the index and the
/// data.
fn move_into(pack_dir: &Path, index_path: &Path, data_path: &Path) -> Result<(PathBuf, PathBuf)> {
    let target_data = pack_dir.join(data_path.file_name().expect("a written pack has a file name"));
    let target_index = target_data.with_extension("idx");
    if !target_data.is_file() {
        std::fs::rename(data_path, &target_data)
            .or_raise(|| message("Could not move the new pack into the pack directory"))?;
    }
    if !target_index.is_file() {
        std::fs::rename(index_path, &target_index)
            .or_raise(|| message("Could not move the index of the new pack into the pack directory"))?;
    }
    Ok((target_index, target_data))
}

/// Why the producer of the pack stopped early.
enum Failure {
    /// An entry couldn't be produced.
    Entry(Error),
    /// The pack couldn't be passed on, which happens once its indexer stopped reading it.
    Write(Error),
}

/// What the producer of the pack needs to know.
struct Producer<'a> {
    objects_dir: &'a Path,
    object_hash: gix_hash::Kind,
    plan: &'a Plan,
    /// The position of each object of `plan` in the pack.
    positions: &'a [usize],
    compression: gix_zlib::Compression,
    should_interrupt: &'a AtomicBool,
}

impl Producer<'_> {
    /// Write the pack to `writer`, counting the written bytes in `progress`.
    fn produce(
        self,
        writer: gix_utils::io::pipe::Writer,
        progress: BoxedDynNestedProgress,
    ) -> std::result::Result<(), Failure> {
        let objects = gix_odb::at(self.objects_dir, self.object_hash)
            .or_raise(|| message!("Could not open the object database at {}", self.objects_dir.display()))
            .map_err(Failure::Entry)?;
        let num_objects = self.plan.objects.len() as u32;
        let object_hash = self.object_hash;
        let entries = Entries {
            producer: self,
            objects,
            next: 0,
            buf: Vec::new(),
            failed: false,
        };
        let mut pack = output::bytes::FromEntriesIter::new(
            entries,
            BufWriter::with_capacity(64 * 1024, writer),
            num_objects,
            gix_pack::data::Version::V2,
            object_hash,
        );
        while let Some(written) = pack.next() {
            match written {
                Ok(written) => progress.inc_by(written as usize),
                Err(err) if pack.input.failed => return Err(Failure::Entry(err)),
                Err(err) => return Err(Failure::Write(err)),
            }
        }
        Ok(())
    }
}

/// The entries of the pack, in the order they are written in.
struct Entries<'a> {
    producer: Producer<'a>,
    /// The object database to read objects from that aren't copied from a pack.
    objects: gix_odb::Handle,
    /// The position in the pack of the next object to produce.
    next: usize,
    buf: Vec<u8>,
    /// Whether an entry couldn't be produced, after which no more entries are produced.
    failed: bool,
}

impl Iterator for Entries<'_> {
    type Item = Result<Vec<output::Entry>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.next == self.producer.plan.order.len() {
            return None;
        }
        let chunk = self.chunk();
        self.failed = chunk.is_err();
        Some(chunk)
    }
}

impl Entries<'_> {
    fn chunk(&mut self) -> Result<Vec<output::Entry>> {
        if self.producer.should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled("Interrupted while writing the new pack"));
        }
        let plan = self.producer.plan;
        let mut entries = Vec::new();
        let mut bytes = 0;
        while let Some(&object) = plan.order.get(self.next) {
            if entries.len() == ENTRIES_PER_CHUNK || bytes >= BYTES_PER_CHUNK {
                break;
            }
            let entry = self.entry(object as usize)?;
            bytes += entry.compressed_data.len();
            entries.push(entry);
            self.next += 1;
        }
        Ok(entries)
    }

    /// The entry of the object at `index` in the plan, copied from its pack where possible.
    fn entry(&mut self, index: usize) -> Result<output::Entry> {
        let plan = self.producer.plan;
        let object = &plan.objects[index];
        let Location::Packed {
            source,
            offset,
            end,
            position,
        } = object.location
        else {
            return self.whole(object.id);
        };
        let source = &plan.sources[source];
        let pack_path = || source.data.path().display();
        let entry = source
            .data
            .entry(offset)
            .or_raise(|| message!("Could not read the entry at offset {offset} of {}", pack_path()))?;
        // The offsets were checked against the size of the pack when it was opened.
        let size = (end - offset) as usize;
        if let Some(expected) = source.index.crc32_at_index(position) {
            ensure!(
                source.data.entry_crc32(offset, size) == expected,
                "The entry at offset {offset} of {} doesn't match the checksum in its index".corrupted(),
                pack_path()
            );
        }
        let kind = match (plan.bases[index], entry.header.as_kind()) {
            (Some(base), _) => output::entry::Kind::DeltaRef {
                object_index: self.producer.positions[base as usize],
            },
            (None, Some(kind)) => output::entry::Kind::Base(kind),
            // The object that the delta applies to doesn't go into the new pack, or the delta would make a chain too long.
            (None, None) => return self.whole(object.id),
        };
        let Some(compressed_data) = source.data.entry_slice(entry.data_offset..end) else {
            bail!(
                "The header of the entry at offset {offset} of {} extends past the entry".corrupted(),
                pack_path()
            );
        };
        let Ok(decompressed_size) = usize::try_from(entry.decompressed_size) else {
            bail!(
                "The object {} is too large to be held in memory".unsupported(),
                object.id
            );
        };
        Ok(output::Entry {
            id: object.id,
            kind,
            decompressed_size,
            compressed_data: compressed_data.to_vec(),
        })
    }

    /// The entry of the object `id` with its data compressed anew.
    fn whole(&mut self, id: ObjectId) -> Result<output::Entry> {
        let Some(data) = self
            .objects
            .try_find(&id, &mut self.buf)
            .or_raise(|| message!("Could not read the object {id}"))?
        else {
            bail!("The object {id} disappeared before it was written".not_found());
        };
        let count = output::Count {
            id,
            entry_pack_location: output::count::PackLocation::NotLookedUp,
        };
        output::Entry::from_data(&count, &data, self.producer.compression)
    }
}

/// The start of the name of each directory that holds a new pack.
const STAGING_PREFIX: &str = "tmp_repack_";

/// Remove the directories in `pack_dir` that held the new packs of repacks which ended before they could remove them.
///
/// Only one repack runs in an object database at a time, so no other repack uses them. A directory that can't be removed
/// is left, as readers don't look for packs in it.
pub(crate) fn remove_left_over_staging(pack_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(pack_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(STAGING_PREFIX))
            && entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_staging && let Err(_err) = std::fs::remove_dir_all(entry.path()) {
            gix_trace::warn!(
                "Could not remove the directory {} that an earlier repack left: {}",
                entry.path().display(),
                _err
            );
        }
    }
}

/// A directory inside the pack directory that holds the new pack until its index was checked.
///
/// Readers of an object database look for packs only in the pack directory itself. The directory and whatever remains in
/// it are removed when it is dropped, or by the next repack if the process ended before. Like the temporary files of
/// `git index-pack`, its name starts with `tmp_`, so that `git prune` removes it as well.
struct Staging {
    path: PathBuf,
}

impl Staging {
    fn new(pack_dir: &Path) -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let path = pack_dir.join(format!(
                "{STAGING_PREFIX}{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Staging { path }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).or_raise(|| {
                        message!(
                            "Could not create a directory for the new pack in {}",
                            pack_dir.display()
                        )
                    });
                }
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Remaining files belong to a pack that wasn't moved into the pack directory, so readers don't see them even if
        // they can't be removed.
        std::fs::remove_dir_all(&self.path).ok();
    }
}
