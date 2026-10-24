//! Deltas as Git's `diff-delta.c` computes them.

/// The amount of bytes that the hashes of an [`Index`] and of [`Index::encode()`] cover, Git's `RABIN_WINDOW`.
const WINDOW: usize = 16;

/// The amount of bits that [`hash_in()`] shifts a hash right by to look up the value to xor into it, Git's
/// `RABIN_SHIFT`.
const SHIFT: u32 = 23;

/// The most entries that one bucket of an [`Index`] keeps, Git's `HASH_LIMIT`.
const BUCKET_LIMIT: u32 = 64;

/// A match at least this long is copied without looking for a longer one.
const GOOD_ENOUGH: usize = 4096;

/// The shortest match that is copied rather than inserted.
const MIN_COPY: usize = 4;

/// The most bytes that one copy instruction copies, as version 2 of the pack format limits them.
const MAX_COPY: usize = 0x10000;

/// The most bytes that one insert instruction carries.
const MAX_INSERT: i32 = 0x7f;

/// The end of a list of [`Unpacked`] entries.
const NONE: u32 = u32::MAX;

/// Git's table `T`: the value xored into a hash for the eight bits that shifting it left by eight moves past bit 30,
/// which keeps hashes below 2^31.
static T: [u32; 256] = [
    0x00000000, 0xab59b4d1, 0x56b369a2, 0xfdeadd73, 0x063f6795, 0xad66d344, 0x508c0e37, 0xfbd5bae6, 0x0c7ecf2a,
    0xa7277bfb, 0x5acda688, 0xf1941259, 0x0a41a8bf, 0xa1181c6e, 0x5cf2c11d, 0xf7ab75cc, 0x18fd9e54, 0xb3a42a85,
    0x4e4ef7f6, 0xe5174327, 0x1ec2f9c1, 0xb59b4d10, 0x48719063, 0xe32824b2, 0x1483517e, 0xbfdae5af, 0x423038dc,
    0xe9698c0d, 0x12bc36eb, 0xb9e5823a, 0x440f5f49, 0xef56eb98, 0x31fb3ca8, 0x9aa28879, 0x6748550a, 0xcc11e1db,
    0x37c45b3d, 0x9c9defec, 0x6177329f, 0xca2e864e, 0x3d85f382, 0x96dc4753, 0x6b369a20, 0xc06f2ef1, 0x3bba9417,
    0x90e320c6, 0x6d09fdb5, 0xc6504964, 0x2906a2fc, 0x825f162d, 0x7fb5cb5e, 0xd4ec7f8f, 0x2f39c569, 0x846071b8,
    0x798aaccb, 0xd2d3181a, 0x25786dd6, 0x8e21d907, 0x73cb0474, 0xd892b0a5, 0x23470a43, 0x881ebe92, 0x75f463e1,
    0xdeadd730, 0x63f67950, 0xc8afcd81, 0x354510f2, 0x9e1ca423, 0x65c91ec5, 0xce90aa14, 0x337a7767, 0x9823c3b6,
    0x6f88b67a, 0xc4d102ab, 0x393bdfd8, 0x92626b09, 0x69b7d1ef, 0xc2ee653e, 0x3f04b84d, 0x945d0c9c, 0x7b0be704,
    0xd05253d5, 0x2db88ea6, 0x86e13a77, 0x7d348091, 0xd66d3440, 0x2b87e933, 0x80de5de2, 0x7775282e, 0xdc2c9cff,
    0x21c6418c, 0x8a9ff55d, 0x714a4fbb, 0xda13fb6a, 0x27f92619, 0x8ca092c8, 0x520d45f8, 0xf954f129, 0x04be2c5a,
    0xafe7988b, 0x5432226d, 0xff6b96bc, 0x02814bcf, 0xa9d8ff1e, 0x5e738ad2, 0xf52a3e03, 0x08c0e370, 0xa39957a1,
    0x584ced47, 0xf3155996, 0x0eff84e5, 0xa5a63034, 0x4af0dbac, 0xe1a96f7d, 0x1c43b20e, 0xb71a06df, 0x4ccfbc39,
    0xe79608e8, 0x1a7cd59b, 0xb125614a, 0x468e1486, 0xedd7a057, 0x103d7d24, 0xbb64c9f5, 0x40b17313, 0xebe8c7c2,
    0x16021ab1, 0xbd5bae60, 0x6cb54671, 0xc7ecf2a0, 0x3a062fd3, 0x915f9b02, 0x6a8a21e4, 0xc1d39535, 0x3c394846,
    0x9760fc97, 0x60cb895b, 0xcb923d8a, 0x3678e0f9, 0x9d215428, 0x66f4eece, 0xcdad5a1f, 0x3047876c, 0x9b1e33bd,
    0x7448d825, 0xdf116cf4, 0x22fbb187, 0x89a20556, 0x7277bfb0, 0xd92e0b61, 0x24c4d612, 0x8f9d62c3, 0x7836170f,
    0xd36fa3de, 0x2e857ead, 0x85dcca7c, 0x7e09709a, 0xd550c44b, 0x28ba1938, 0x83e3ade9, 0x5d4e7ad9, 0xf617ce08,
    0x0bfd137b, 0xa0a4a7aa, 0x5b711d4c, 0xf028a99d, 0x0dc274ee, 0xa69bc03f, 0x5130b5f3, 0xfa690122, 0x0783dc51,
    0xacda6880, 0x570fd266, 0xfc5666b7, 0x01bcbbc4, 0xaae50f15, 0x45b3e48d, 0xeeea505c, 0x13008d2f, 0xb85939fe,
    0x438c8318, 0xe8d537c9, 0x153feaba, 0xbe665e6b, 0x49cd2ba7, 0xe2949f76, 0x1f7e4205, 0xb427f6d4, 0x4ff24c32,
    0xe4abf8e3, 0x19412590, 0xb2189141, 0x0f433f21, 0xa41a8bf0, 0x59f05683, 0xf2a9e252, 0x097c58b4, 0xa225ec65,
    0x5fcf3116, 0xf49685c7, 0x033df00b, 0xa86444da, 0x558e99a9, 0xfed72d78, 0x0502979e, 0xae5b234f, 0x53b1fe3c,
    0xf8e84aed, 0x17bea175, 0xbce715a4, 0x410dc8d7, 0xea547c06, 0x1181c6e0, 0xbad87231, 0x4732af42, 0xec6b1b93,
    0x1bc06e5f, 0xb099da8e, 0x4d7307fd, 0xe62ab32c, 0x1dff09ca, 0xb6a6bd1b, 0x4b4c6068, 0xe015d4b9, 0x3eb80389,
    0x95e1b758, 0x680b6a2b, 0xc352defa, 0x3887641c, 0x93ded0cd, 0x6e340dbe, 0xc56db96f, 0x32c6cca3, 0x999f7872,
    0x6475a501, 0xcf2c11d0, 0x34f9ab36, 0x9fa01fe7, 0x624ac294, 0xc9137645, 0x26459ddd, 0x8d1c290c, 0x70f6f47f,
    0xdbaf40ae, 0x207afa48, 0x8b234e99, 0x76c993ea, 0xdd90273b, 0x2a3b52f7, 0x8162e626, 0x7c883b55, 0xd7d18f84,
    0x2c043562, 0x875d81b3, 0x7ab75cc0, 0xd1eee811,
];

/// Git's table `U`: what a byte contributes to the hash of the 16 bytes that it is the first of, so that xoring it
/// removes the byte from the hash.
static U: [u32; 256] = [
    0x00000000, 0x7eb5200d, 0x5633f4cb, 0x2886d4c6, 0x073e5d47, 0x798b7d4a, 0x510da98c, 0x2fb88981, 0x0e7cba8e,
    0x70c99a83, 0x584f4e45, 0x26fa6e48, 0x0942e7c9, 0x77f7c7c4, 0x5f711302, 0x21c4330f, 0x1cf9751c, 0x624c5511,
    0x4aca81d7, 0x347fa1da, 0x1bc7285b, 0x65720856, 0x4df4dc90, 0x3341fc9d, 0x1285cf92, 0x6c30ef9f, 0x44b63b59,
    0x3a031b54, 0x15bb92d5, 0x6b0eb2d8, 0x4388661e, 0x3d3d4613, 0x39f2ea38, 0x4747ca35, 0x6fc11ef3, 0x11743efe,
    0x3eccb77f, 0x40799772, 0x68ff43b4, 0x164a63b9, 0x378e50b6, 0x493b70bb, 0x61bda47d, 0x1f088470, 0x30b00df1,
    0x4e052dfc, 0x6683f93a, 0x1836d937, 0x250b9f24, 0x5bbebf29, 0x73386bef, 0x0d8d4be2, 0x2235c263, 0x5c80e26e,
    0x740636a8, 0x0ab316a5, 0x2b7725aa, 0x55c205a7, 0x7d44d161, 0x03f1f16c, 0x2c4978ed, 0x52fc58e0, 0x7a7a8c26,
    0x04cfac2b, 0x73e5d470, 0x0d50f47d, 0x25d620bb, 0x5b6300b6, 0x74db8937, 0x0a6ea93a, 0x22e87dfc, 0x5c5d5df1,
    0x7d996efe, 0x032c4ef3, 0x2baa9a35, 0x551fba38, 0x7aa733b9, 0x041213b4, 0x2c94c772, 0x5221e77f, 0x6f1ca16c,
    0x11a98161, 0x392f55a7, 0x479a75aa, 0x6822fc2b, 0x1697dc26, 0x3e1108e0, 0x40a428ed, 0x61601be2, 0x1fd53bef,
    0x3753ef29, 0x49e6cf24, 0x665e46a5, 0x18eb66a8, 0x306db26e, 0x4ed89263, 0x4a173e48, 0x34a21e45, 0x1c24ca83,
    0x6291ea8e, 0x4d29630f, 0x339c4302, 0x1b1a97c4, 0x65afb7c9, 0x446b84c6, 0x3adea4cb, 0x1258700d, 0x6ced5000,
    0x4355d981, 0x3de0f98c, 0x15662d4a, 0x6bd30d47, 0x56ee4b54, 0x285b6b59, 0x00ddbf9f, 0x7e689f92, 0x51d01613,
    0x2f65361e, 0x07e3e2d8, 0x7956c2d5, 0x5892f1da, 0x2627d1d7, 0x0ea10511, 0x7014251c, 0x5facac9d, 0x21198c90,
    0x099f5856, 0x772a785b, 0x4c921c31, 0x32273c3c, 0x1aa1e8fa, 0x6414c8f7, 0x4bac4176, 0x3519617b, 0x1d9fb5bd,
    0x632a95b0, 0x42eea6bf, 0x3c5b86b2, 0x14dd5274, 0x6a687279, 0x45d0fbf8, 0x3b65dbf5, 0x13e30f33, 0x6d562f3e,
    0x506b692d, 0x2ede4920, 0x06589de6, 0x78edbdeb, 0x5755346a, 0x29e01467, 0x0166c0a1, 0x7fd3e0ac, 0x5e17d3a3,
    0x20a2f3ae, 0x08242768, 0x76910765, 0x59298ee4, 0x279caee9, 0x0f1a7a2f, 0x71af5a22, 0x7560f609, 0x0bd5d604,
    0x235302c2, 0x5de622cf, 0x725eab4e, 0x0ceb8b43, 0x246d5f85, 0x5ad87f88, 0x7b1c4c87, 0x05a96c8a, 0x2d2fb84c,
    0x539a9841, 0x7c2211c0, 0x029731cd, 0x2a11e50b, 0x54a4c506, 0x69998315, 0x172ca318, 0x3faa77de, 0x411f57d3,
    0x6ea7de52, 0x1012fe5f, 0x38942a99, 0x46210a94, 0x67e5399b, 0x19501996, 0x31d6cd50, 0x4f63ed5d, 0x60db64dc,
    0x1e6e44d1, 0x36e89017, 0x485db01a, 0x3f77c841, 0x41c2e84c, 0x69443c8a, 0x17f11c87, 0x38499506, 0x46fcb50b,
    0x6e7a61cd, 0x10cf41c0, 0x310b72cf, 0x4fbe52c2, 0x67388604, 0x198da609, 0x36352f88, 0x48800f85, 0x6006db43,
    0x1eb3fb4e, 0x238ebd5d, 0x5d3b9d50, 0x75bd4996, 0x0b08699b, 0x24b0e01a, 0x5a05c017, 0x728314d1, 0x0c3634dc,
    0x2df207d3, 0x534727de, 0x7bc1f318, 0x0574d315, 0x2acc5a94, 0x54797a99, 0x7cffae5f, 0x024a8e52, 0x06852279,
    0x78300274, 0x50b6d6b2, 0x2e03f6bf, 0x01bb7f3e, 0x7f0e5f33, 0x57888bf5, 0x293dabf8, 0x08f998f7, 0x764cb8fa,
    0x5eca6c3c, 0x207f4c31, 0x0fc7c5b0, 0x7172e5bd, 0x59f4317b, 0x27411176, 0x1a7c5765, 0x64c97768, 0x4c4fa3ae,
    0x32fa83a3, 0x1d420a22, 0x63f72a2f, 0x4b71fee9, 0x35c4dee4, 0x1400edeb, 0x6ab5cde6, 0x42331920, 0x3c86392d,
    0x133eb0ac, 0x6d8b90a1, 0x450d4467, 0x3bb8646a,
];

/// The hash of the bytes that `hash` covers followed by `byte`, without removing the byte that drops out of the window:
/// xoring [`U`] does that.
#[inline]
fn hash_in(hash: u32, byte: u8) -> u32 {
    ((hash << 8) | u32::from(byte)) ^ T[(hash >> SHIFT) as usize]
}

/// The hash of `bytes`, which are [`WINDOW`] bytes or fewer.
fn hash_of(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |hash, &byte| hash_in(hash, byte))
}

/// A source indexed to compute deltas against it, as Git's `create_delta_index()` indexes it.
///
/// The index holds the hash of each block of 16 bytes of the source from its second byte on, with the offset of the
/// last byte of the block. Of consecutive blocks with the same hash, only the first one is kept, and a bucket of more
/// than 64 entries keeps 64 of them, spread over the bucket. Only the first 4 GiB of a source are indexed, as copy
/// instructions hold offsets of 32 bits.
///
/// Build the index once to compute the deltas of several targets against the same source.
#[derive(Debug, Clone)]
pub struct Index {
    /// The length of the source.
    source_len: usize,
    /// The amount of buckets less one: an entry whose hash is `h` is in bucket `h & mask`.
    mask: u32,
    /// Where the entries of each bucket start in `entries`, followed by the amount of entries.
    starts: Vec<u32>,
    /// The entries, grouped by bucket, and ordered by offset within each bucket.
    entries: Vec<Entry>,
}

/// A block of the source.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The hash of the 16 bytes of the block.
    hash: u32,
    /// The offset of the last byte of the block, where a copy that this block finds starts.
    end: u32,
}

/// An entry while the index is built, linked to the next entry of its bucket.
struct Unpacked {
    entry: Entry,
    /// The position of the next entry of the bucket in the entries being built, or [`NONE`].
    next: u32,
}

impl Index {
    /// Index `source`, or return `None` if it is empty, as Git doesn't compute deltas against an empty source.
    pub fn new(source: &[u8]) -> Option<Self> {
        if source.is_empty() {
            return None;
        }
        let mut count: u32 = if source.len() as u64 >= 0xffff_ffff {
            0xffff_fffe / WINDOW as u32
        } else {
            ((source.len() - 1) / WINDOW) as u32
        };
        let mut bits = 4;
        while (1u32 << bits) < count / 4 {
            bits += 1;
        }
        let buckets = 1usize << bits;
        let mask = (buckets - 1) as u32;

        // The blocks are visited from the last to the first, and each is put first in its bucket, so that each bucket
        // lists its entries by ascending offset.
        let mut heads = vec![NONE; buckets];
        let mut counts = vec![0u32; buckets];
        let mut unpacked: Vec<Unpacked> = Vec::with_capacity(count as usize);
        let mut previous = !0;
        let mut start = count as usize * WINDOW;
        while start > 0 {
            start -= WINDOW;
            let hash = hash_of(&source[start + 1..=start + WINDOW]);
            let end = (start + WINDOW) as u32;
            if hash == previous {
                unpacked
                    .last_mut()
                    .expect("the block after this one has an entry")
                    .entry
                    .end = end;
                count -= 1;
            } else {
                previous = hash;
                let bucket = (hash & mask) as usize;
                unpacked.push(Unpacked {
                    entry: Entry { hash, end },
                    next: heads[bucket],
                });
                heads[bucket] = (unpacked.len() - 1) as u32;
                counts[bucket] += 1;
            }
        }

        // Remove the entries over the limit from each bucket, spread evenly over the list of the bucket.
        for (bucket, &bucket_count) in counts.iter().enumerate() {
            if bucket_count <= BUCKET_LIMIT {
                continue;
            }
            count -= bucket_count - BUCKET_LIMIT;
            let excess = i64::from(bucket_count - BUCKET_LIMIT);
            let mut entry = heads[bucket];
            let mut acc = 0i64;
            loop {
                acc += excess;
                if acc > 0 {
                    let keep = entry;
                    loop {
                        entry = unpacked[entry as usize].next;
                        acc -= i64::from(BUCKET_LIMIT);
                        if acc <= 0 {
                            break;
                        }
                    }
                    unpacked[keep as usize].next = unpacked[entry as usize].next;
                }
                entry = unpacked[entry as usize].next;
                if entry == NONE {
                    break;
                }
            }
        }

        let mut starts = Vec::with_capacity(buckets + 1);
        let mut entries = Vec::with_capacity(count as usize);
        for &head in &heads {
            starts.push(entries.len() as u32);
            let mut entry = head;
            while entry != NONE {
                let linked = &unpacked[entry as usize];
                entries.push(linked.entry);
                entry = linked.next;
            }
        }
        starts.push(entries.len() as u32);
        debug_assert_eq!(entries.len(), count as usize, "each removed entry was counted");
        Some(Index {
            source_len: source.len(),
            mask,
            starts,
            entries,
        })
    }

    /// The amount of bytes that Git's index of the same source takes on this platform, which `git pack-objects` counts
    /// against `pack.windowMemory`: its `struct delta_index`, a pointer to the entries of each bucket and one more, and a
    /// `struct index_entry` for each entry, with the sizes that C gives them.
    pub(crate) fn memory_size(&self) -> usize {
        /// Git's `struct delta_index` without its pointers to the buckets.
        #[allow(dead_code)]
        #[repr(C)]
        struct DeltaIndex {
            memsize: std::ffi::c_ulong,
            src_buf: *const u8,
            src_size: std::ffi::c_ulong,
            hash_mask: std::ffi::c_uint,
        }
        /// Git's `struct index_entry`.
        #[allow(dead_code)]
        #[repr(C)]
        struct IndexEntry {
            ptr: *const u8,
            val: std::ffi::c_uint,
        }
        size_of::<DeltaIndex>()
            + size_of::<*const u8>() * self.starts.len()
            + size_of::<IndexEntry>() * self.entries.len()
    }

    /// Write the delta that builds `target` from `source` into `out`, replacing its content, and return true, or return
    /// false and leave `out` empty, as Git's `create_delta()` does.
    ///
    /// No delta is written for an empty target, nor once the delta written so far is longer than `max_size` bytes.
    /// Like Git, the encoder stops at the first time that the delta is too long, although a copy that is found later
    /// can extend backwards over bytes that were to be inserted and make the delta shorter again. A delta that is
    /// written has the same bytes whatever `max_size` is.
    ///
    /// # Panics
    ///
    /// If `source` doesn't have the length of the source that the index was built from.
    pub fn encode(&self, source: &[u8], target: &[u8], max_size: Option<usize>, out: &mut Vec<u8>) -> bool {
        assert_eq!(
            source.len(),
            self.source_len,
            "The source must be the one that the index was built from"
        );
        out.clear();
        if target.is_empty() {
            return false;
        }
        push_size(out, source.len());
        push_size(out, target.len());

        // The first 16 bytes of the target are inserted, and their hash starts the rolling hash.
        out.push(0);
        let mut pos = 0;
        while pos < WINDOW && pos < target.len() {
            out.push(target[pos]);
            pos += 1;
        }
        let mut hash = hash_of(&target[..pos]);
        // The amount of bytes of the insert instruction being written, whose count byte precedes them in `out`.
        let mut inserted = pos as i32;
        // The offset in the source and the length of the copy that is found.
        let mut copy_offset: u64 = 0;
        let mut copy_len: usize = 0;
        while pos < target.len() {
            if copy_len < GOOD_ENOUGH {
                hash ^= U[usize::from(target[pos - WINDOW])];
                hash = hash_in(hash, target[pos]);
                let bucket = (hash & self.mask) as usize;
                for entry in &self.entries[self.starts[bucket] as usize..self.starts[bucket + 1] as usize] {
                    if entry.hash != hash {
                        continue;
                    }
                    let end = entry.end as usize;
                    // Git counts the bytes to compare in 32 bits.
                    let mut len = (source.len() - end) as u32;
                    let remaining = target.len() - pos;
                    if len as usize > remaining {
                        len = remaining as u32;
                    }
                    if len as usize <= copy_len {
                        break;
                    }
                    let len = len as usize;
                    let matched = common_prefix(&source[end..end + len], &target[pos..pos + len]);
                    if copy_len < matched {
                        copy_len = matched;
                        copy_offset = end as u64;
                        if copy_len >= GOOD_ENOUGH {
                            break;
                        }
                    }
                }
            }

            if copy_len < MIN_COPY {
                if inserted == 0 {
                    out.push(0);
                }
                out.push(target[pos]);
                pos += 1;
                inserted += 1;
                if inserted == MAX_INSERT {
                    set_insert_count(out, inserted);
                    inserted = 0;
                }
                copy_len = 0;
            } else {
                if inserted != 0 {
                    // Extend the copy backwards over the bytes of the insert instruction being written.
                    while copy_offset != 0 && source[copy_offset as usize - 1] == target[pos - 1] {
                        copy_len += 1;
                        copy_offset -= 1;
                        pos -= 1;
                        out.pop();
                        inserted -= 1;
                        if inserted != 0 {
                            continue;
                        }
                        // The copy took over all bytes of the insert instruction: remove its count byte too.
                        out.pop();
                        inserted = -1;
                        break;
                    }
                    if inserted > 0 {
                        set_insert_count(out, inserted);
                    }
                    inserted = 0;
                }

                let left = copy_len.saturating_sub(MAX_COPY);
                copy_len -= left;
                push_copy(out, copy_offset as u32, copy_len);
                pos += copy_len;
                copy_offset += copy_len as u64;
                copy_len = left;
                if copy_offset > 0xffff_ffff {
                    copy_len = 0;
                }
                if copy_len < GOOD_ENOUGH {
                    hash = hash_of(&target[pos - WINDOW..pos]);
                }
            }

            if max_size.is_some_and(|max_size| out.len() > max_size) {
                out.clear();
                return false;
            }
        }
        if inserted != 0 {
            set_insert_count(out, inserted);
        }
        if max_size.is_some_and(|max_size| out.len() > max_size) {
            out.clear();
            return false;
        }
        true
    }
}

/// The amount of bytes at the start of `a` and `b` that are the same.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let len = a.len().min(b.len());
    let (a, b) = (&a[..len], &b[..len]);
    let mut same = 0;
    while same + 8 <= len {
        let a_word = u64::from_le_bytes(a[same..same + 8].try_into().expect("eight bytes"));
        let b_word = u64::from_le_bytes(b[same..same + 8].try_into().expect("eight bytes"));
        let difference = a_word ^ b_word;
        if difference != 0 {
            // The first byte is the least significant one of a little-endian word.
            return same + (difference.trailing_zeros() / 8) as usize;
        }
        same += 8;
    }
    while same < len && a[same] == b[same] {
        same += 1;
    }
    same
}

/// Append `size` as a size of the delta header: seven bits at a time, the least significant first, with the high bit set
/// in each byte but the last.
fn push_size(out: &mut Vec<u8>, mut size: usize) {
    while size >= 0x80 {
        out.push(0x80 | size as u8);
        size >>= 7;
    }
    out.push(size as u8);
}

/// Write `count` into the count byte of the insert instruction whose `count` bytes end `out`.
fn set_insert_count(out: &mut [u8], count: i32) {
    let at = out.len() - count as usize - 1;
    out[at] = count as u8;
}

/// Append a copy instruction of `len` bytes, at most [`MAX_COPY`], from `offset`. Its first byte has the high bit set,
/// and a bit for each byte of the offset and of the length that follows it, the least significant first. Bytes that are
/// zero are left out, so a length of 0x10000 has no bytes, which readers take as 0x10000.
fn push_copy(out: &mut Vec<u8>, offset: u32, len: usize) {
    debug_assert!((1..=MAX_COPY).contains(&len), "copies have between 1 and 0x10000 bytes");
    let first = out.len();
    out.push(0);
    let mut instruction = 0x80;
    for (bit, byte) in offset.to_le_bytes().into_iter().enumerate() {
        if byte != 0 {
            instruction |= 1 << bit;
            out.push(byte);
        }
    }
    for (bit, byte) in (len as u16).to_le_bytes().into_iter().enumerate() {
        if byte != 0 {
            instruction |= 0x10 << bit;
            out.push(byte);
        }
    }
    out[first] = instruction;
}

/// Return the delta that builds `target` from `source`, or `None` if either of them is empty, as Git computes no deltas
/// for them.
///
/// To compute the deltas of several targets against the same source, build its [`Index`] once instead.
pub fn encode(source: &[u8], target: &[u8]) -> Option<Vec<u8>> {
    let index = Index::new(source)?;
    let mut delta = Vec::new();
    index.encode(source, target, None, &mut delta).then_some(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_linear_keep_31_bits_and_remove_the_first_byte_of_a_window() {
        for a in 0..256 {
            assert_eq!(
                T[a] >> 31,
                (a & 1) as u32,
                "T[{a:#x}] clears the bit that the shift moves to bit 31"
            );
            for b in 0..256 {
                assert_eq!(T[a ^ b], T[a] ^ T[b], "T[{a:#x} ^ {b:#x}]");
                assert_eq!(U[a ^ b], U[a] ^ U[b], "U[{a:#x} ^ {b:#x}]");
            }
        }
        let bytes: Vec<u8> = (0u32..1000)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        for start in 0..bytes.len() - WINDOW {
            let rolled = hash_in(
                hash_of(&bytes[start..start + WINDOW]) ^ U[usize::from(bytes[start])],
                bytes[start + WINDOW],
            );
            assert_eq!(
                rolled,
                hash_of(&bytes[start + 1..=start + WINDOW]),
                "window at {}",
                start + 1
            );
            assert!(rolled < 1 << 31);
        }
    }

    #[test]
    fn sizes_have_seven_bits_per_byte() {
        for (size, expected) in [
            (0, &[0x00][..]),
            (0x7F, &[0x7F]),
            (0x80, &[0x80, 0x01]),
            (0x3FFF, &[0xFF, 0x7F]),
            (0x4000, &[0x80, 0x80, 0x01]),
        ] {
            let mut out = Vec::new();
            push_size(&mut out, size);
            assert_eq!(out, expected, "size {size:#x}");
        }
    }

    #[test]
    fn copies_leave_out_zero_bytes_and_write_0x10000_without_length_bytes() {
        for (offset, len, expected) in [
            (0, 1, &[0x90, 0x01][..]),
            (0x0100, 0x10, &[0x92, 0x01, 0x10]),
            (0x1234_5678, 0xCDEF, &[0xBF, 0x78, 0x56, 0x34, 0x12, 0xEF, 0xCD]),
            (0x0001_0000, 0x0001_0000, &[0x84, 0x01]),
            (0, 0x0100, &[0xA0, 0x01]),
            (0xFF00_0000, 0xFF00, &[0xA8, 0xFF, 0xFF]),
        ] {
            let mut out = Vec::new();
            push_copy(&mut out, offset, len);
            assert_eq!(out, expected, "copy of {len:#x} bytes at {offset:#x}");
        }
    }

    #[test]
    fn consecutive_blocks_with_the_same_hash_keep_the_first_one() {
        let index = Index::new(&[b'a'; 1 + WINDOW * 100]).expect("not empty");
        assert_eq!(index.entries.len(), 1, "all blocks have the same hash");
        assert_eq!(index.entries[0].end, WINDOW as u32, "the first block is kept");
    }

    #[test]
    fn buckets_keep_64_entries_spread_over_the_bucket() {
        // Blocks of `a` alternate with blocks of `b`, so that no two blocks of `a` are consecutive.
        let mut source = vec![b'-'];
        for _ in 0..500 {
            source.extend_from_slice(&[b'a'; WINDOW]);
            source.extend_from_slice(&[b'b'; WINDOW]);
        }
        let index = Index::new(&source).expect("not empty");
        let a = hash_of(&[b'a'; WINDOW]);
        let ends: Vec<u32> = index
            .entries
            .iter()
            .filter(|entry| entry.hash == a)
            .map(|entry| entry.end)
            .collect();
        assert_eq!(ends.len(), BUCKET_LIMIT as usize);
        assert_eq!(
            ends.first(),
            Some(&(WINDOW as u32)),
            "the first entry of a bucket stays"
        );
        assert!(ends.windows(2).all(|pair| pair[0] < pair[1]), "offsets stay ordered");
        let spacing = ends.windows(2).map(|pair| (pair[1] - pair[0]) as usize / (2 * WINDOW));
        assert!(
            spacing.into_iter().all(|blocks| (7..=8).contains(&blocks)),
            "one in about 500 / 64 entries is kept"
        );
    }

    #[test]
    fn the_memory_size_is_that_of_git_on_the_platform() {
        let source: Vec<u8> = (0u32..1 << 20)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect();
        let index = Index::new(&source).expect("not empty");
        let buckets = ((source.len() - 1) / WINDOW / 4).next_power_of_two();
        assert_eq!(index.starts.len(), buckets + 1);
        // Git's `struct delta_index` has 32 bytes where `unsigned long` and pointers have 64 bits, 24 on 64-bit Windows,
        // where `unsigned long` has 32 bits, and 16 on 32-bit platforms; each entry is a pointer and an `unsigned int`.
        let (header, pointer) = match (cfg!(target_pointer_width = "64"), cfg!(windows)) {
            (true, false) => (32, 8),
            (true, true) => (24, 8),
            (false, _) => (16, 4),
        };
        assert_eq!(
            index.memory_size(),
            header + pointer * (buckets + 1) + 2 * pointer * index.entries.len()
        );
    }
}
