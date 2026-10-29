use gix_error::Result;
use std::io::Write;

use gix_error::{ErrorExt, ResultExt, bail, ensure, message};
use gix_hash::{ObjectId, oid};

use crate::{data, data::output, find};

mod deltas;

///
pub mod iter_from_counts;
pub use iter_from_counts::function::iter_from_counts;

/// The kind of pack entry to be written
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Kind {
    /// A complete base object, including its kind
    Base(gix_object::Kind),
    /// A delta against the object with the given index. It's always an index that was already encountered to refer only
    /// to object we have written already.
    DeltaRef {
        /// The absolute index to the object to serve as base. It's up to the writer to maintain enough state to allow producing
        /// a packed delta object from it.
        object_index: usize,
    },
    /// A delta against the given object as identified by its `ObjectId`.
    /// This is the case for thin packs only, i.e. those that are sent over the wire.
    /// Note that there is the option of the `ObjectId` being used to refer to an object within
    /// the same pack, but it's a discontinued practice which won't be encountered here.
    DeltaOid {
        /// The object serving as base for this delta
        id: ObjectId,
    },
}

/// The base of a delta that an [`Entry`](output::Entry) is made of with [`from_delta()`](output::Entry::from_delta()).
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
pub enum DeltaBase {
    /// The base is the entry with this index among the entries written before, as with [`Kind::DeltaRef`].
    Index(usize),
    /// The base is the object with this id, which the receiver of a thin pack has, as with [`Kind::DeltaOid`].
    Id(ObjectId),
}

impl output::Entry {
    /// An object which can be identified as invalid easily which happens if objects didn't exist even if they were referred to.
    pub fn invalid() -> output::Entry {
        output::Entry {
            id: gix_hash::Kind::shortest().null(), // NOTE: the actual object hash used in the repo doesn't matter here, this is a sentinel value.
            kind: Kind::Base(gix_object::Kind::Blob),
            decompressed_size: 0,
            compressed_data: vec![],
        }
    }

    /// Returns true if this object doesn't really exist but still has to be handled responsibly
    ///
    /// Note that this is true for tree entries that are commits/git submodules, or for objects which aren't present in our local clone
    /// due to shallow clones.
    pub fn is_invalid(&self) -> bool {
        self.id.is_null()
    }

    /// Create an Entry from a previously counted object which is located in a pack. It's `entry` is provided here.
    /// The `target_version` specifies what kind of target `Entry` version the caller desires. Both supported versions use
    /// the same entry encoding.
    ///
    /// Errors from `pack_offset_to_oid` are propagated rather than triggering recompression as if the base were absent.
    pub fn from_pack_entry(
        entry: find::Entry,
        count: &output::Count,
        potential_bases: &[output::Count],
        bases_index_offset: usize,
        pack_offset_to_oid: Option<impl FnMut(u32, u64) -> Result<Option<ObjectId>>>,
        target_version: data::Version,
    ) -> Option<Result<Self>> {
        if entry.version != target_version {
            return None;
        }

        let pack_offset_must_be_zero = 0;
        let pack_entry = match data::Entry::from_bytes(&entry.data, pack_offset_must_be_zero, count.id.kind()) {
            Ok(e) => e,
            Err(err) => return Some(Err(err)),
        };

        use crate::data::entry::Header::*;
        match pack_entry.header {
            Commit => Some(output::entry::Kind::Base(gix_object::Kind::Commit)),
            Tree => Some(output::entry::Kind::Base(gix_object::Kind::Tree)),
            Blob => Some(output::entry::Kind::Base(gix_object::Kind::Blob)),
            Tag => Some(output::entry::Kind::Base(gix_object::Kind::Tag)),
            OfsDelta { base_distance } => {
                let pack_location = count.entry_pack_location.as_ref().expect("packed");
                let Some(base_offset) =
                    crate::data::entry::Header::verified_base_pack_offset(pack_location.pack_offset, base_distance)
                else {
                    return Some(Err(
                        message("an ofs-delta base distance pointing before pack start").corrupted_error()
                    ));
                };
                match potential_bases.binary_search_by(|e| {
                    e.entry_pack_location
                        .as_ref()
                        .expect("packed")
                        .pack_offset
                        .cmp(&base_offset)
                }) {
                    Ok(idx) => Some(output::entry::Kind::DeltaRef {
                        object_index: idx + bases_index_offset,
                    }),
                    Err(_) => match pack_offset_to_oid {
                        Some(mut lookup) => match lookup(pack_location.pack_id, base_offset) {
                            Ok(base_id) => base_id.map(|id| output::entry::Kind::DeltaOid { id }),
                            Err(err) => return Some(Err(err)),
                        },
                        None => None,
                    },
                }
            }
            RefDelta { base_id: _ } => None, // ref deltas are for thin packs or legacy, repack them as base objects
        }
        .map(|kind| {
            Ok(output::Entry {
                id: count.id.to_owned(),
                kind,
                decompressed_size: pack_entry.decompressed_size as usize,
                compressed_data: compressed_data(entry.data, pack_entry.data_offset),
            })
        })
    }

    /// Create a new instance from the given `oid` and its corresponding git object data `obj`,
    /// deflating it with `compression`.
    ///
    /// Note that `git` compresses pack entries with the level configured with `pack.compression`,
    /// whose default is [`Compression::DEFAULT`](gix_zlib::Compression::DEFAULT).
    pub fn from_data(
        count: &output::Count,
        obj: &gix_object::Data<'_>,
        compression: gix_zlib::Compression,
    ) -> Result<Self> {
        Ok(output::Entry {
            id: count.id.to_owned(),
            kind: Kind::Base(obj.kind),
            decompressed_size: obj.data.len(),
            compressed_data: deflate(obj.data, compression)?,
        })
    }

    /// Create a new instance for the object `id` that is stored as `delta`, which builds it from `base`, deflating `delta`
    /// with `compression`.
    ///
    /// This is how `git pack-objects` writes the deltas that it computes, such as those of
    /// [`find_deltas()`](crate::data::delta::search::find_deltas()): the delta is compressed like the data of an object
    /// stored whole, and the entry refers to a base among the entries written before it by the distance to the base's
    /// offset in the pack, and to a base that the receiver of a thin pack has by its id.
    pub fn from_delta(id: ObjectId, base: DeltaBase, delta: &[u8], compression: gix_zlib::Compression) -> Result<Self> {
        Ok(output::Entry {
            id,
            kind: match base {
                DeltaBase::Index(object_index) => Kind::DeltaRef { object_index },
                DeltaBase::Id(id) => Kind::DeltaOid { id },
            },
            decompressed_size: delta.len(),
            compressed_data: deflate(delta, compression)?,
        })
    }

    /// Create a new instance for the object `id` that is stored as the `delta` that
    /// [`find_deltas()`](crate::data::delta::search::find_deltas()) found for it against the object `base_id`, which the
    /// entry refers to as `base` says, as [`from_delta()`](Self::from_delta()) does.
    ///
    /// The delta is the one that the search kept, or else the delta that [`encode()`](crate::data::delta::encode()) computes
    /// again from the base and the object, which it reads from `objects`, as Git's `get_delta()` does. Like Git, this is
    /// an error if the delta computed again doesn't have the length that the search found, and so is an object that
    /// `objects` doesn't have.
    pub fn from_found_delta(
        id: ObjectId,
        base: DeltaBase,
        base_id: &oid,
        delta: &crate::data::delta::search::Delta,
        objects: &dyn gix_object::Find,
        compression: gix_zlib::Compression,
    ) -> Result<Self> {
        if let Some(data) = &delta.data {
            return Self::from_delta(id, base, data, compression);
        }
        let (mut base_buf, mut buf) = (Vec::new(), Vec::new());
        let base_data = read(objects, base_id, &mut base_buf)?;
        let data = read(objects, &id, &mut buf)?;
        let computed = crate::data::delta::encode(base_data.data, data.data).unwrap_or_default();
        ensure!(
            computed.len() == delta.size,
            "The delta of {id} against {base_id} has {} bytes when it is computed again, instead of {}",
            computed.len(),
            delta.size
        );
        Self::from_delta(id, base, &computed, compression)
    }

    /// Transform ourselves into a pack entry header which can be written into a pack of `version`.
    ///
    /// `index_to_pack(object_index) -> pack_offset` is a function to convert the base object's index into
    /// the input object array (if each object is numbered) to an offset into the pack.
    /// This information is known to the one calling the method.
    pub fn to_entry_header(
        &self,
        _version: data::Version,
        index_to_base_distance: impl FnOnce(usize) -> u64,
    ) -> data::entry::Header {
        use Kind::*;
        match self.kind {
            Base(kind) => {
                use gix_object::Kind::*;
                match kind {
                    Tree => data::entry::Header::Tree,
                    Blob => data::entry::Header::Blob,
                    Commit => data::entry::Header::Commit,
                    Tag => data::entry::Header::Tag,
                }
            }
            DeltaOid { id } => data::entry::Header::RefDelta { base_id: id.to_owned() },
            DeltaRef { object_index } => data::entry::Header::OfsDelta {
                base_distance: index_to_base_distance(object_index),
            },
        }
    }
}

/// The object `id` that `objects` reads into `buf`, or an error if it doesn't exist.
fn read<'a>(objects: &dyn gix_object::Find, id: &oid, buf: &'a mut Vec<u8>) -> Result<gix_object::Data<'a>> {
    match objects
        .try_find(id, buf)
        .or_raise(|| message!("Could not read the object {id}"))?
    {
        Some(data) => Ok(data),
        None => bail!("The object {id} disappeared before it was written".not_found()),
    }
}

/// The compressed data of the pack entry whose bytes, its header followed by its data, are `entry_data`, and whose data
/// starts at `data_offset`.
fn compressed_data(mut entry_data: Vec<u8>, data_offset: data::Offset) -> Vec<u8> {
    let data_offset = usize::try_from(data_offset).expect("offset representable as usize");
    entry_data.copy_within(data_offset.., 0);
    entry_data.truncate(entry_data.len() - data_offset);
    entry_data
}

/// `data` compressed with `compression`, as the data of a pack entry.
fn deflate(data: &[u8], compression: gix_zlib::Compression) -> Result<Vec<u8>> {
    let mut out = gix_zlib::stream::deflate::Write::new(Vec::new(), compression);
    if let Err(err) = std::io::copy(&mut &*data, &mut out) {
        match err.kind() {
            std::io::ErrorKind::Other => {
                bail!(err.and_raise(message("Failed to compress pack entry")));
            }
            err => unreachable!("Should never see other errors than zlib, but got {:?}", err),
        }
    }
    out.flush()
        .or_raise(|| message("Failed to finish compressing pack entry"))?;
    Ok(out.into_inner())
}
