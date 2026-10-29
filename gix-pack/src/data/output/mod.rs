use gix_hash::ObjectId;

///
pub mod count;

/// An item representing a future Entry in the leanest way possible.
///
/// One can expect to have one of these in memory when building big objects, so smaller is better here.
/// They should contain everything of importance to generate a pack as fast as possible.
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Count {
    /// The hash of the object to write
    pub id: ObjectId,
    /// A way to locate a pack entry in the object database, only available if the object is in a pack.
    pub entry_pack_location: count::PackLocation,
    /// The [name hash](crate::data::delta::search::name_hash()) of the path at which counting found the object, as
    /// `git pack-objects` records it, which orders the objects for the delta search of
    /// [`iter_from_counts()`](entry::iter_from_counts()), or 0 for objects without a path, such as commits.
    pub name_hash: u32,
    /// Whether the object is only a base for the deltas of a thin pack: the receiver of the pack has it, so that the pack
    /// doesn't contain it, as `git pack-objects --thin` takes such preferred bases from the trees at the edge of what it
    /// sends.
    pub preferred_base: bool,
    /// Whether a tag reference points to the object or peels to it, as `git pack-objects` marks the objects of the
    /// references in `refs/tags/`, which moves it forward in the order in which
    /// [`Mode::PackCopyAndDeltaSearch`](entry::iter_from_counts::Mode::PackCopyAndDeltaSearch) writes objects.
    pub tagged: bool,
}

/// An entry to be written to a file.
///
/// Some of these will be in-flight and in memory while waiting to be written. Memory requirements depend on the amount of compressed
/// data they hold.
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entry {
    /// The hash of the object to write
    pub id: ObjectId,
    /// The kind of entry represented by `data`. It's used alongside with it to complete the pack entry
    /// at rest or in transit.
    pub kind: entry::Kind,
    /// The size in bytes needed once `data` gets decompressed
    pub decompressed_size: usize,
    /// The compressed data right behind the header
    pub compressed_data: Vec<u8>,
}

///
pub mod entry;

///
pub mod bytes;
