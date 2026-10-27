use gix_error::{Error, Result};
use std::{fs, io};

use gix_error::{ErrorExt, ResourceExhaustionKind, ResultExt, bail, message};
use gix_hash::{Hasher, ObjectId};
use gix_zlib::Decompress;

use crate::data::input;

/// An iterator over [`Entries`][input::Entry] in a byte stream.
///
/// The iterator used as part of [`Bundle::write_to_directory(…)`][crate::Bundle::write_to_directory()].
///
/// Each entry is inflated to learn where it ends. Its bytes update the hash of the pack and the CRC32 of the entry as the
/// decompressor consumes them, as `git index-pack` updates them, so that compressed bytes are held in memory only if they
/// are kept. Blobs above a [big-file threshold](Self::with_big_file_threshold()) are hashed while they are inflated, too.
pub struct BytesToEntriesIter<BR> {
    read: BR,
    decompressor: Decompress,
    offset: u64,
    had_error: bool,
    version: crate::data::Version,
    objects_left: u32,
    hash: Option<Hasher>,
    mode: input::Mode,
    compressed: input::EntryDataMode,
    hash_len: usize,
    object_hash: gix_hash::Kind,
    /// Blobs larger than this many bytes are hashed while they are inflated.
    big_file_threshold: Option<u64>,
    /// The most compressed bytes of an entry that are kept.
    alloc_limit_bytes: Option<usize>,
    /// Receives the entries of the blobs above `big_file_threshold` as they are read, whose compressed bytes aren't kept.
    large_blob_out: Option<Box<dyn io::Write + Send>>,
}

/// Access
impl<BR> BytesToEntriesIter<BR> {
    /// The pack version currently being iterated
    pub fn version(&self) -> crate::data::Version {
        self.version
    }

    /// The kind of iteration
    pub fn mode(&self) -> input::Mode {
        self.mode
    }
}

/// Configuration
impl<BR> BytesToEntriesIter<BR> {
    /// Treat blobs larger than `big_file_threshold` bytes as `git index-pack` treats blobs above `core.bigFileThreshold`:
    /// hash each one while it is inflated, so that its entry carries its [`id`](input::Entry::id) and an index writer
    /// needn't inflate it again unless a delta needs it as its base.
    pub fn with_big_file_threshold(mut self, big_file_threshold: u64) -> Self {
        self.big_file_threshold = Some(big_file_threshold);
        self
    }

    /// Write each blob above the [big-file threshold](Self::with_big_file_threshold()) to `out` as it is read instead of
    /// keeping its compressed bytes, whatever the [`EntryDataMode`](input::EntryDataMode): first its header, as
    /// [`Header::write_to()`](crate::data::entry::Header::write_to()) writes it, then its compressed bytes.
    /// [`EntriesToBytesIter`](input::EntriesToBytesIter) writing into `out` finds such an entry written when it receives it.
    pub fn with_large_blobs_written_to(mut self, out: Box<dyn io::Write + Send>) -> Self {
        self.large_blob_out = Some(out);
        self
    }

    /// Refuse to keep more than `alloc_limit_bytes` compressed bytes of an entry, which fails the entry instead.
    pub fn with_alloc_limit_bytes(mut self, alloc_limit_bytes: Option<usize>) -> Self {
        self.alloc_limit_bytes = alloc_limit_bytes;
        self
    }
}

/// Initialization
impl<BR> BytesToEntriesIter<BR>
where
    BR: io::BufRead,
{
    /// Obtain an iterator from a `read` stream to a pack data file and configure it using `mode` and `compressed`.
    /// `object_hash` specifies which hash is used for objects in ref-delta entries.
    ///
    /// Note that `read` is expected at the beginning of a valid pack data file with a header, entries and a trailer.
    /// The last entry reads the trailer, so for a pack without entries it is read here, and verified unless `mode` is
    /// [`AsIs`](input::Mode::AsIs). Either way, `read` is left right after the pack.
    pub fn new_from_header(
        mut read: BR,
        mode: input::Mode,
        compressed: input::EntryDataMode,
        object_hash: gix_hash::Kind,
    ) -> Result<BytesToEntriesIter<BR>> {
        let mut header_data = [0u8; crate::data::header::SIZE];
        read.read_exact(&mut header_data).map_err(io_error)?;

        let (version, num_objects) = crate::data::header::decode(&header_data)?;
        let mut iter = BytesToEntriesIter {
            read,
            decompressor: Decompress::new(),
            compressed,
            offset: crate::data::header::SIZE as u64,
            had_error: false,
            version,
            objects_left: num_objects,
            hash: (mode != input::Mode::AsIs).then(|| {
                let mut hash = gix_hash::hasher(object_hash);
                hash.update(&header_data);
                hash
            }),
            mode,
            big_file_threshold: None,
            alloc_limit_bytes: None,
            large_blob_out: None,
            hash_len: object_hash.len_in_bytes(),
            object_hash,
        };
        if num_objects == 0 {
            iter.try_read_trailer()?;
        }
        Ok(iter)
    }

    fn next_inner(&mut self) -> Result<input::Entry> {
        self.objects_left -= 1; // even an error counts as objects

        // Read header
        let entry = match self.hash.as_mut() {
            Some(hash) => {
                let mut read = read_and_pass_to(
                    &mut self.read,
                    HashWrite {
                        inner: io::sink(),
                        hash,
                    },
                );
                crate::data::Entry::from_read(&mut read, self.offset, self.hash_len)
            }
            None => crate::data::Entry::from_read(&mut self.read, self.offset, self.hash_len),
        }
        .map_err(io_error)?;

        let mut header_buf = [0u8; 12 + gix_hash::Kind::longest().len_in_bytes()];
        let header_len = entry
            .header
            .write_to(entry.decompressed_size, &mut header_buf.as_mut())
            .map_err(io_error)?;
        let header = &header_buf[..header_len];
        let is_large_blob = entry.header == crate::data::entry::Header::Blob
            && self
                .big_file_threshold
                .is_some_and(|threshold| entry.decompressed_size > threshold);
        let out = match self.large_blob_out.as_deref_mut() {
            Some(out) if is_large_blob => {
                out.write_all(header).map_err(io_error)?;
                Some(out)
            }
            _ => None,
        };
        let mut crc32 = self.compressed.crc32().then(|| {
            let mut crc32 = crc32fast::Hasher::new();
            crc32.update(header);
            crc32
        });
        // A large blob is never held in memory as a whole, so it is hashed while it is inflated.
        let mut object = is_large_blob.then(|| {
            let mut hash = gix_hash::hasher(self.object_hash);
            hash.update(&gix_object::encode::loose_header(
                gix_object::Kind::Blob,
                entry.decompressed_size,
            ));
            hash
        });

        // Decompress the object to learn its compressed bytes, which are passed on as they are consumed.
        self.decompressor.reset();
        let (inflated, kept, consumed, stop) = {
            let mut compressed = EntryRead {
                kept: (self.compressed.keep() && out.is_none()).then(Vec::new),
                read: &mut self.read,
                pack_hash: self.hash.as_mut(),
                crc32: crc32.as_mut(),
                alloc_limit_bytes: self.alloc_limit_bytes,
                out,
                consumed: 0,
                stop: None,
            };
            let inflated = io::copy(
                &mut DecompressRead {
                    inner: &mut compressed,
                    decompressor: &mut self.decompressor,
                },
                &mut Inflated(object.as_mut()),
            );
            (inflated, compressed.kept, compressed.consumed, compressed.stop)
        };
        match stop {
            Some(Stop::Alloc(kind)) => {
                bail!(gix_error::resource_exhaustion(
                    kind,
                    "Compressed entry too large to keep in memory"
                ));
            }
            Some(Stop::Write(err)) => return Err(io_error(err)),
            None => {}
        }
        let bytes_copied = inflated.map_err(io_error)?;
        if bytes_copied != entry.decompressed_size {
            bail!(gix_error::corruption(format!(
                "pack is incomplete: it was decompressed into {bytes_copied} bytes but {} bytes where expected.",
                entry.decompressed_size
            )));
        }

        let pack_offset = self.offset;
        let compressed_size = self.decompressor.total_in();
        debug_assert_eq!(
            compressed_size, consumed,
            "we must track exactly the same amount of bytes as read by the decompressor"
        );
        self.offset += entry.header_size() as u64 + compressed_size;
        let crc32 = crc32.map(crc32fast::Hasher::finalize);
        let id = object.map(Hasher::try_finalize).transpose()?;

        // Last objects gets trailer (which is potentially verified)
        let trailer = self.try_read_trailer()?;
        Ok(input::Entry {
            header: entry.header,
            header_size: entry.header_size() as u16,
            compressed: kept,
            compressed_size,
            crc32,
            pack_offset,
            decompressed_size: bytes_copied,
            trailer,
            id,
        })
    }

    fn try_read_trailer(&mut self) -> Result<Option<ObjectId>> {
        Ok(if self.objects_left == 0 {
            let mut id = gix_hash::ObjectId::null(self.object_hash);
            if let Err(err) = self.read.read_exact(id.as_mut_slice())
                && self.mode != input::Mode::Restore
            {
                return Err(io_error(err));
            }

            if let Some(hash) = self.hash.take() {
                let actual_id = hash.try_finalize().map_err(hash_io_error)?;
                if self.mode == input::Mode::Restore {
                    id = actual_id;
                } else {
                    actual_id
                        .verify(&id)
                        .or_raise(|| message("Failed to verify pack checksum in trailer"))?;
                }
            }
            Some(id)
        } else if self.mode == input::Mode::Restore {
            let hash = self.hash.clone().expect("in restore mode a hash is set");
            Some(hash.try_finalize().map_err(hash_io_error)?)
        } else {
            None
        })
    }
}

fn read_and_pass_to<R: io::Read, W: io::Write>(read: &mut R, to: W) -> PassThrough<&mut R, W> {
    PassThrough { read, write: to }
}

impl<R> Iterator for BytesToEntriesIter<R>
where
    R: io::BufRead,
{
    type Item = Result<input::Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.had_error || self.objects_left == 0 {
            return None;
        }
        let result = self.next_inner();
        self.had_error = result.is_err();
        if self.had_error {
            self.objects_left = 0;
        }
        if self.mode == input::Mode::Restore && self.had_error {
            None
        } else {
            Some(result)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.objects_left as usize, Some(self.objects_left as usize))
    }
}

impl<R> std::iter::ExactSizeIterator for BytesToEntriesIter<R> where R: io::BufRead {}

struct PassThrough<R, W> {
    read: R,
    write: W,
}

/// Reads the compressed bytes of an entry from `read` and passes each byte that the decompressor consumes on: to the hash
/// of the pack, to the CRC32 of the entry, to the compressed bytes that are kept and to the output of large blobs.
struct EntryRead<'a, R> {
    read: &'a mut R,
    pack_hash: Option<&'a mut Hasher>,
    crc32: Option<&'a mut crc32fast::Hasher>,
    kept: Option<Vec<u8>>,
    alloc_limit_bytes: Option<usize>,
    out: Option<&'a mut (dyn io::Write + Send + 'static)>,
    /// The amount of bytes that were consumed.
    consumed: u64,
    /// Why consumed bytes couldn't be passed on. As [`consume()`](io::BufRead::consume()) can't fail, the next read fails.
    stop: Option<Stop>,
}

/// Why the consumed bytes of an entry couldn't be passed on.
enum Stop {
    /// Keeping them would allocate beyond the limit, or the allocation failed.
    Alloc(ResourceExhaustionKind),
    /// Writing them to the output of large blobs failed.
    Write(io::Error),
}

impl<R: io::BufRead> io::BufRead for EntryRead<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.stop.is_some() {
            return Err(io::Error::other(
                "the compressed bytes of the entry couldn't be passed on",
            ));
        }
        self.read.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        let bytes = &self
            .read
            .fill_buf()
            .expect("never fail as we called fill-buf before and this does nothing")[..amt];
        if let Some(hash) = self.pack_hash.as_mut() {
            hash.update(bytes);
        }
        if let Some(crc32) = self.crc32.as_mut() {
            crc32.update(bytes);
        }
        if self.stop.is_none()
            && let Some(kept) = self.kept.as_mut()
        {
            match reserve_within(kept, amt, self.alloc_limit_bytes) {
                Ok(()) => kept.extend_from_slice(bytes),
                Err(kind) => self.stop = Some(Stop::Alloc(kind)),
            }
        }
        if self.stop.is_none()
            && let Some(out) = self.out.as_mut()
            && let Err(err) = out.write_all(bytes)
        {
            self.stop = Some(Stop::Write(err));
        }
        self.consumed += amt as u64;
        self.read.consume(amt);
    }
}

impl<R: io::BufRead> io::Read for EntryRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = io::BufRead::fill_buf(self)?;
        let amt = available.len().min(buf.len());
        buf[..amt].copy_from_slice(&available[..amt]);
        io::BufRead::consume(self, amt);
        Ok(amt)
    }
}

/// Make room in `kept` for `additional` bytes, growing it as a `Vec` grows, but never beyond `alloc_limit_bytes`.
fn reserve_within(
    kept: &mut Vec<u8>,
    additional: usize,
    alloc_limit_bytes: Option<usize>,
) -> std::result::Result<(), ResourceExhaustionKind> {
    let Some(limit) = alloc_limit_bytes else {
        return kept
            .try_reserve(additional)
            .map_err(|_| ResourceExhaustionKind::AllocationFailure);
    };
    let len = kept.len().saturating_add(additional);
    if len > limit {
        return Err(ResourceExhaustionKind::AllocationLimit);
    }
    if len > kept.capacity() {
        let capacity = kept.capacity().saturating_mul(2).clamp(len, limit);
        kept.try_reserve_exact(capacity - kept.len())
            .map_err(|_| ResourceExhaustionKind::AllocationFailure)?;
    }
    Ok(())
}

/// Discards the inflated bytes of an entry, but hashes them first if the id of its object is computed.
struct Inflated<'a>(Option<&'a mut Hasher>);

impl io::Write for Inflated<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(hash) = self.0.as_mut() {
            hash.update(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<R, W> io::Read for PassThrough<R, W>
where
    W: io::Write,
    R: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_read = self.read.read(buf)?;
        self.write.write_all(&buf[..bytes_read])?;
        Ok(bytes_read)
    }
}

impl<T> crate::data::File<T>
where
    T: crate::FileData,
{
    /// Returns an iterator over [`Entries`][crate::data::input::Entry], without making use of the memory mapping.
    pub fn streaming_iter(&self) -> Result<BytesToEntriesIter<impl io::BufRead>> {
        let reader = io::BufReader::with_capacity(4096 * 8, fs::File::open(&self.path).map_err(io_error)?);
        BytesToEntriesIter::new_from_header(
            reader,
            input::Mode::Verify,
            input::EntryDataMode::KeepAndCrc32,
            self.object_hash,
        )
    }
}

fn io_error(err: io::Error) -> Error {
    err.and_raise(message("An IO operation failed while streaming an entry"))
}

fn hash_io_error(err: Error) -> Error {
    err.and_raise(message("An IO operation failed while streaming an entry"))
}

/// The boxed variant is faster for what we do (moving the decompressor in and out a lot)
pub struct DecompressRead<'a, R> {
    /// The reader from which bytes should be decompressed.
    pub inner: R,
    /// The decompressor doing all the work.
    pub decompressor: &'a mut Decompress,
}

impl<R> io::Read for DecompressRead<'_, R>
where
    R: io::BufRead,
{
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        gix_zlib::stream::inflate::read(&mut self.inner, self.decompressor, into)
    }
}

/// A utility to automatically generate a hash while writing into an inner writer.
pub struct HashWrite<'a, T> {
    /// The hash implementation.
    pub hash: &'a mut Hasher,
    /// The inner writer.
    pub inner: T,
}

impl<T> std::io::Write for HashWrite<'_, T>
where
    T: std::io::Write,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hash.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
