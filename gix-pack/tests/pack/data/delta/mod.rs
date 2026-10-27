//! Every delta that `data::delta` computes is checked by a reader of packs: the tests write packs that store objects as
//! these deltas and have `gix-pack` index them, which applies each delta and computes the id of each object from the
//! result. The deltas that `git pack-objects` writes are compared with the ones computed here from the same objects.

mod encode;
mod git;
mod search;

use std::{io::Write, sync::atomic::AtomicBool};

use gix_pack::data::output;
use gix_testtools::TestResult;

/// An object of a test pack.
struct Packed<'a> {
    kind: gix_object::Kind,
    data: &'a [u8],
    /// The position of the object earlier in the pack that this object is stored as a delta of, and the delta.
    delta: Option<(usize, Vec<u8>)>,
}

impl<'a> Packed<'a> {
    fn whole(kind: gix_object::Kind, data: &'a [u8]) -> Self {
        Packed {
            kind,
            data,
            delta: None,
        }
    }

    fn delta(kind: gix_object::Kind, data: &'a [u8], base: usize, delta: Vec<u8>) -> Self {
        Packed {
            kind,
            data,
            delta: Some((base, delta)),
        }
    }
}

/// Write `objects` into a pack in their order, have gix-pack index it, and check that the index lists the id of each
/// object, which the indexer computed from the data that the deltas build.
fn assert_pack_holds(objects: &[Packed<'_>]) -> TestResult {
    let object_hash = gix_testtools::object_hash();
    let mut ids = Vec::with_capacity(objects.len());
    let mut entries = Vec::with_capacity(objects.len());
    for object in objects {
        let id = gix_object::compute_hash(object_hash, object.kind, object.data)?;
        entries.push(match &object.delta {
            None => output::Entry::from_data(
                &output::Count {
                    id,
                    entry_pack_location: output::count::PackLocation::NotLookedUp,
                },
                &gix_object::Data {
                    kind: object.kind,
                    object_hash,
                    data: object.data,
                },
                gix_zlib::Compression::DEFAULT,
            )?,
            Some((base, delta)) => output::Entry {
                id,
                kind: output::entry::Kind::DeltaRef { object_index: *base },
                decompressed_size: delta.len(),
                compressed_data: deflate(delta),
            },
        });
        ids.push(id);
    }

    let num_entries = u32::try_from(entries.len())?;
    let mut pack = Vec::new();
    for written in output::bytes::FromEntriesIter::new(
        std::iter::once(Ok(entries)),
        &mut pack,
        num_entries,
        gix_pack::data::Version::V2,
        object_hash,
    ) {
        written?;
    }

    let dir = gix_testtools::tempfile::TempDir::new()?;
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut pack.as_slice(),
        Some(dir.path()),
        &mut gix_utils::progress::Discard,
        &AtomicBool::new(false),
        None::<gix_object::find::Never>,
        object_hash,
        gix_pack::bundle::write::Options {
            thread_limit: Some(1),
            iteration_mode: gix_pack::data::input::Mode::Verify,
            index_version: gix_pack::index::Version::default(),
            alloc_limit_bytes: None,
            compression: gix_zlib::Compression::DEFAULT,
            ..Default::default()
        },
    )?;
    let index_path = outcome.index_path.expect("the pack was written into a directory");
    let index = gix_pack::index::File::at(&index_path, object_hash)?;
    assert_eq!(index.num_objects() as usize, objects.len());
    for id in ids {
        assert!(index.lookup(id).is_some(), "the deltas build the object {id}");
    }
    Ok(())
}

/// `data` compressed as pack entries are.
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut out = gix_zlib::stream::deflate::Write::new(Vec::new(), gix_zlib::Compression::DEFAULT);
    out.write_all(data).expect("writing to memory doesn't fail");
    out.flush().expect("writing to memory doesn't fail");
    out.into_inner()
}

/// A deterministic source of pseudo-random numbers (xorshift64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }

    /// `lines` lines of words from a small vocabulary, which repeat as they do in source code.
    fn text(&mut self, lines: usize) -> Vec<u8> {
        const WORDS: &[&str] = &[
            "let", "fn", "match", "object", "delta", "base", "window", "depth", "pack", "index", "=", "{", "}", "(",
            ")", ";", "self", "return", "Some", "None",
        ];
        let mut text = Vec::new();
        for _ in 0..lines {
            for _ in 0..=self.below(10) {
                text.extend_from_slice(WORDS[self.below(WORDS.len())].as_bytes());
                text.push(b' ');
            }
            text.push(b'\n');
        }
        text
    }

    /// `text` with a few lines removed, replaced or added, so that it differs from `text`.
    fn edit_lines(&mut self, text: &[u8]) -> Vec<u8> {
        loop {
            let mut lines: Vec<Vec<u8>> = text
                .split_inclusive(|&byte| byte == b'\n')
                .map(<[u8]>::to_vec)
                .collect();
            for _ in 0..=self.below(4) {
                let at = self.below(lines.len() + 1);
                match self.below(3) {
                    0 if at < lines.len() => {
                        lines.remove(at);
                    }
                    1 if at < lines.len() => lines[at] = self.text(1),
                    _ => lines.insert(at, self.text(1)),
                }
            }
            let edited = lines.concat();
            if edited != text {
                return edited;
            }
        }
    }

    /// `data` with a few ranges removed, replaced with random bytes, or with random bytes inserted, so that it differs
    /// from `data`.
    fn edit_bytes(&mut self, data: &[u8]) -> Vec<u8> {
        loop {
            let mut edited = data.to_vec();
            for _ in 0..=self.below(4) {
                let at = self.below(edited.len() + 1);
                let available = (edited.len() - at).min(200);
                let (removed, inserted) = match self.below(3) {
                    0 if available > 0 => (1 + self.below(available), 0),
                    1 => (0, 1 + self.below(200)),
                    _ => (self.below(available + 1), 1 + self.below(200)),
                };
                let replacement = self.bytes(inserted);
                edited.splice(at..at + removed, replacement);
            }
            if edited != data {
                return edited;
            }
        }
    }
}
