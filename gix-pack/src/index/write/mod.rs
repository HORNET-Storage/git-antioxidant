use gix_error::Result;

pub(crate) struct TreeEntry {
    pub id: gix_hash::ObjectId,
    pub crc32: u32,
}

/// Information gathered while executing [`write_data_iter_to_stream()`][crate::index::write_data_iter_to_stream]
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Outcome {
    /// The version of the verified index
    pub index_version: crate::index::Version,
    /// The verified checksum of the verified index
    pub index_hash: gix_hash::ObjectId,

    /// The hash of the '.pack' file, also found in its trailing bytes
    pub data_hash: gix_hash::ObjectId,
    /// The amount of objects that were verified, always the amount of objects in the pack.
    pub num_objects: u32,
}

/// The progress ids used in [`write_data_iter_to_stream()`][crate::index::write_data_iter_to_stream()].
///
/// Use this information to selectively extract the progress of interest in case the parent application has custom visualization.
#[derive(Debug, Copy, Clone)]
pub enum ProgressId {
    /// Counts the amount of objects that were index thus far.
    IndexObjects,
    /// The amount of bytes that were decompressed while decoding pack entries.
    ///
    /// This is done to determine entry boundaries.
    DecompressedBytes,
    /// The amount of objects whose hashes were computed.
    ///
    /// This is done by decoding them, which typically involves decoding delta objects.
    ResolveObjects,
    /// The amount of bytes that were decoded in total, as the sum of all bytes to represent all resolved objects.
    DecodedBytes,
    /// The amount of bytes written to the index file.
    IndexBytesWritten,
}

impl From<ProgressId> for gix_utils::progress::Id {
    fn from(v: ProgressId) -> Self {
        match v {
            ProgressId::IndexObjects => *b"IWIO",
            ProgressId::DecompressedBytes => *b"IWDB",
            ProgressId::ResolveObjects => *b"IWRO",
            ProgressId::DecodedBytes => *b"IWDB",
            ProgressId::IndexBytesWritten => *b"IWBW",
        }
    }
}

pub(super) mod function {
    use gix_error::{Result, message, validation};
    use std::{io, sync::atomic::AtomicBool};

    use gix_error::{OptionExt, ResultExt, bail};
    use gix_utils::progress::{self, Count, Progress, prodash::DynNestedProgress};

    use crate::cache::delta::{Tree, traverse};

    use super::{Outcome, ProgressId, TreeEntry, modify_base};

    /// Write information about `entries` as obtained from a pack data file into a pack index file via the `out` stream.
    /// The resolver produced by `make_resolver` must resolve pack entries from the same pack data file that produced the
    /// `entries` iterator.
    ///
    /// # Ref-delta bases
    ///
    /// Bases available through an ODB lookup are handled by wrapping `entries` in
    /// [`crate::data::input::LookupRefDeltaObjectsIter`]. As entries are consumed, it inserts each full base immediately
    /// before the first delta that needs it, then rewrites that and later references to the same base as `OFS_DELTA`s.
    ///
    /// Remaining `REF_DELTA`s are resolved in-pack here. They are recorded by base object ID; while traversing the delta
    /// tree, each fully resolved object is hashed and any deltas waiting for that ID are attached as its children. Thus an
    /// in-pack base may occur before or after its delta, and forward-reference chains are supported. Resolution fails if a
    /// referenced base was neither inserted by the wrapper nor found among the pack entries.
    ///
    /// * `kind` is the version of pack index to produce, use [`crate::index::Version::default()`] if in doubt.
    /// * `tread_limit` is used for a parallel tree traversal for obtaining object hashes with optimal performance.
    /// * `root_progress` is the top-level progress to stay informed about the progress of this potentially long-running
    ///   computation.
    /// * `object_hash` defines what kind of object hash we write into the index file.
    /// * `alloc_limit_bytes` limits the maximum size of individual allocations for the delta tree and while resolving pack
    ///   entries to compute object ids. `None` means no limit is applied.
    /// * `big_file_threshold` is the size in bytes above which a blob is treated like `git index-pack` treats a blob above
    ///   `core.bigFileThreshold`, whose default is 512 MiB: it isn't decompressed into a buffer of its size, see
    ///   *Large blobs*.
    /// * `pack_version` is the version of the underlying pack for which `entries` are read. It's used in case none of these objects are provided
    ///   to compute a pack-hash.
    ///
    /// # Large blobs
    ///
    /// A blob above `big_file_threshold` is hashed while it is inflated in chunks, as `git index-pack` does, unless a delta
    /// needs it as its base: then it is decompressed as a whole to resolve the delta, as Git does as well. If its entry
    /// has an [`id`](crate::data::input::Entry::id), as
    /// [`BytesToEntriesIter::with_big_file_threshold()`](crate::data::input::BytesToEntriesIter::with_big_file_threshold())
    /// computes while the entry is read, it isn't inflated here at all, and otherwise it is inflated 8 KiB at a time from
    /// the bytes that the resolver provides. Deltas, the bases of deltas and objects other than blobs are decompressed
    /// as a whole, as in Git.
    ///
    /// # Remarks
    ///
    /// * `make_resolver()` will only be called after the iterator stopped returning elements and produces a function that
    ///   provides all bytes belonging to a pack entry writing them to the given mutable output `Vec`.
    ///   It should return `None` if the entry cannot be resolved from the pack that produced the `entries` iterator, causing
    ///   the write operation to fail.
    #[expect(clippy::too_many_arguments)]
    pub fn write_data_iter_to_stream<F, F2, R>(
        version: crate::index::Version,
        make_resolver: F,
        entries: &mut dyn Iterator<Item = Result<crate::data::input::Entry>>,
        thread_limit: Option<usize>,
        root_progress: &mut dyn DynNestedProgress,
        out: &mut dyn io::Write,
        should_interrupt: &AtomicBool,
        object_hash: gix_hash::Kind,
        alloc_limit_bytes: Option<usize>,
        big_file_threshold: u64,
        pack_version: crate::data::Version,
    ) -> Result<Outcome>
    where
        F: FnOnce() -> io::Result<(F2, R)>,
        R: Send + Sync,
        F2: for<'r> Fn(crate::data::EntryRange, &'r R) -> Option<&'r [u8]> + Send + Clone,
    {
        if version != crate::index::Version::default() {
            bail!(validation(format!(
                "Indices of type {} cannot be written, only {} are supported",
                version as usize,
                crate::index::Version::default() as usize
            )));
        }
        let mut num_objects: usize = 0;
        let mut last_seen_trailer = None;
        let (anticipated_num_objects, upper_bound) = entries.size_hint();
        let worst_case_num_objects_after_thin_pack_resolution = upper_bound.unwrap_or(anticipated_num_objects);
        let mut tree = Tree::with_capacity(worst_case_num_objects_after_thin_pack_resolution, alloc_limit_bytes)?;
        let indexing_start = std::time::Instant::now();

        root_progress.init(Some(4), progress::steps());
        let mut objects_progress = root_progress.add_child_with_id("indexing".into(), ProgressId::IndexObjects.into());
        objects_progress.init(Some(anticipated_num_objects), progress::count("objects"));
        let mut decompressed_progress =
            root_progress.add_child_with_id("decompressing".into(), ProgressId::DecompressedBytes.into());
        decompressed_progress.init(None, progress::bytes());
        let mut pack_entries_end: u64 = 0;
        // The blobs above `big_file_threshold` whose entries come without id, by pack offset, entry size, header size and
        // object size.
        let mut unhashed_large_blobs = Vec::new();

        for entry in entries {
            let crate::data::input::Entry {
                header,
                pack_offset,
                crc32,
                header_size,
                compressed: _,
                compressed_size,
                decompressed_size,
                trailer,
                id,
            } = entry?;

            decompressed_progress.inc_by(decompressed_size as usize);

            let entry_len = u64::from(header_size) + compressed_size;
            pack_entries_end = pack_offset + entry_len;

            let crc32 = crc32.expect("crc32 to be computed by the iterator. Caller assures correct configuration.");

            use crate::data::entry::Header::*;
            match header {
                Tree | Blob | Commit | Tag => match id {
                    Some(id) => tree.add_resolved_root(pack_offset, TreeEntry { id, crc32 }, id)?,
                    None if matches!(header, Blob) && decompressed_size > big_file_threshold => {
                        let id = object_hash.null();
                        tree.add_resolved_root(pack_offset, TreeEntry { id, crc32 }, id)?;
                        unhashed_large_blobs.push((pack_offset, entry_len, header_size, decompressed_size));
                    }
                    None => tree.add_root(
                        pack_offset,
                        TreeEntry {
                            id: object_hash.null(),
                            crc32,
                        },
                    )?,
                },
                RefDelta { base_id } => {
                    tree.add_child_by_id(
                        base_id,
                        pack_offset,
                        TreeEntry {
                            id: object_hash.null(),
                            crc32,
                        },
                    )?;
                }
                OfsDelta { base_distance } => {
                    let base_pack_offset = crate::data::entry::Header::verified_base_pack_offset(
                        pack_offset,
                        base_distance,
                    )
                    .ok_or_raise(|| {
                        message!("{pack_offset} is not a valid offset for pack offset {base_distance}").validation()
                    })?;
                    tree.add_child(
                        base_pack_offset,
                        pack_offset,
                        TreeEntry {
                            id: object_hash.null(),
                            crc32,
                        },
                    )?;
                }
            }
            last_seen_trailer = trailer;
            num_objects += 1;
            objects_progress.inc();
        }
        let num_objects = u32::try_from(num_objects)
            .or_raise(|| message!("Only u32::MAX objects can be stored in a pack, found {num_objects}").validation())?;

        objects_progress.show_throughput(indexing_start);
        decompressed_progress.show_throughput(indexing_start);
        drop(objects_progress);
        drop(decompressed_progress);

        root_progress.inc();

        let (resolver, pack) = make_resolver().map_err(gix_hash::io::from_std_io)?;
        for (pack_offset, entry_len, header_size, decompressed_size) in unhashed_large_blobs {
            let compressed = resolver(pack_offset..pack_offset + entry_len, &pack)
                .and_then(|entry| entry.get(usize::from(header_size)..))
                .ok_or_raise(|| {
                    message!("The resolver failed to obtain the pack entry bytes for the entry at {pack_offset}")
                })?;
            let id = hash_blob_in_chunks(compressed, decompressed_size, object_hash)?;
            let (data, resolved_id) = tree
                .resolved_root_mut(pack_offset)
                .expect("each large blob was added as resolved root");
            data.id = id;
            *resolved_id = id;
        }
        let sorted_pack_offsets_by_oid = {
            let traverse::Outcome { roots, children } = tree.traverse(
                resolver,
                &pack,
                pack_entries_end,
                |data,
                 _progress,
                 traverse::Context {
                     entry,
                     decompressed: bytes,
                     ..
                 }| {
                    // A root whose id is known already is only resolved as the base of its deltas.
                    if data.id.is_null() {
                        modify_base(data, entry, bytes, object_hash)
                    } else {
                        Ok(())
                    }
                },
                traverse::Options {
                    object_progress: Box::new(
                        root_progress.add_child_with_id("Resolving".into(), ProgressId::ResolveObjects.into()),
                    ),
                    size_progress: &mut root_progress
                        .add_child_with_id("Decoding".into(), ProgressId::DecodedBytes.into()),
                    thread_limit,
                    should_interrupt,
                    object_hash,
                    alloc_limit_bytes,
                },
            )?;
            root_progress.inc();

            let mut items = roots;
            items.extend(children);
            {
                let _progress = root_progress.add_child_with_id("sorting by id".into(), gix_utils::progress::UNKNOWN);
                items.sort_by_key(|e| e.data.id);
            }

            root_progress.inc();
            items
        };

        let pack_hash = match last_seen_trailer {
            Some(ph) => ph,
            None if num_objects == 0 => {
                let header = crate::data::header::encode(pack_version, 0);
                let mut hasher = gix_hash::hasher(object_hash);
                hasher.update(&header);
                hasher.try_finalize()?
            }
            None => {
                bail!(validation(
                    "The iterator failed to set a trailing hash over all prior pack entries in the last provided entry"
                ));
            }
        };
        let index_hash = crate::index::encode::write_to(
            out,
            sorted_pack_offsets_by_oid,
            &pack_hash,
            version,
            object_hash,
            &mut root_progress.add_child_with_id("writing index file".into(), ProgressId::IndexBytesWritten.into()),
        )?;
        root_progress.show_throughput_with(
            indexing_start,
            num_objects as usize,
            progress::count("objects").expect("unit always set"),
            progress::MessageLevel::Success,
        );
        Ok(Outcome {
            index_version: version,
            index_hash,
            data_hash: pack_hash,
            num_objects,
        })
    }

    /// Compute the id of the blob of `size` bytes whose compressed bytes are `compressed`, inflating it 8 KiB at a time
    /// so that it is never held in memory as a whole.
    fn hash_blob_in_chunks(
        mut compressed: &[u8],
        size: u64,
        object_hash: gix_hash::Kind,
    ) -> Result<gix_hash::ObjectId> {
        let mut hasher = gix_hash::hasher(object_hash);
        hasher.update(&gix_object::encode::loose_header(gix_object::Kind::Blob, size));
        let mut inflate = gix_zlib::Decompress::new();
        let mut buf = [0u8; 8192];
        let mut inflated = 0u64;
        loop {
            let len = gix_zlib::stream::inflate::read(&mut compressed, &mut inflate, &mut buf)
                .or_raise(|| message("Failed to decompress entry"))?;
            if len == 0 {
                break;
            }
            inflated += len as u64;
            if inflated > size {
                bail!(gix_error::corruption(
                    "Pack entry decompressed to more bytes than declared in the entry header"
                ));
            }
            hasher.update(&buf[..len]);
        }
        if inflated != size {
            bail!(gix_error::corruption(
                "Pack entry is truncated: pack entry decompressed to fewer bytes than declared in the entry header"
            ));
        }
        hasher.try_finalize()
    }
}

fn modify_base(
    entry: &mut TreeEntry,
    pack_entry: &crate::data::Entry,
    decompressed: &[u8],
    hash: gix_hash::Kind,
) -> Result {
    let object_kind = pack_entry.header.as_kind().expect("base object as source of iteration");
    let id = gix_object::compute_hash(hash, object_kind, decompressed)?;
    entry.id = id;
    Ok(())
}
