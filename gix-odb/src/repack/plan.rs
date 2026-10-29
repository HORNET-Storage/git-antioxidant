use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use gix_error::{Result, ResultExt, bail, cancelled, ensure, message};
use gix_hash::{ObjectId, oid};
use gix_pack::cache::lru::MemoryCappedHashmap;
use gix_pack::data::{
    decode::header::ResolvedBase,
    delta::{
        chain,
        search::{self, InPack},
    },
    entry::Header,
};
use gix_utils::progress::{self, Count, DynNestedProgress, Progress};

use crate::repack::{
    Options, Pack,
    deltas::{self, Facts},
    loose, names,
};

/// How many objects are looked at between two checks of the interrupt flag.
const OBJECTS_PER_INTERRUPT_CHECK: usize = 4096;

/// How many bytes of decoded delta bases git keeps while it reads objects by default, its `core.deltaBaseCacheLimit`.
const DELTA_BASE_CACHE_LIMIT: usize = 96 * 1024 * 1024;

/// A combined pack, opened to copy its entries.
pub(crate) struct Source {
    pub(crate) data: gix_pack::data::File,
    pub(crate) index: gix_pack::index::File,
    /// The offset of each entry and its position in `index`, ordered by offset.
    entries: Vec<(u64, u32)>,
}

impl Source {
    fn open(pack: &Pack, object_hash: gix_hash::Kind) -> Result<Self> {
        let index = open_index(pack, object_hash)?;
        let data_path = pack.data_path();
        let data = gix_pack::data::File::at(&data_path, object_hash)
            .or_raise(|| message!("Could not open the pack {}", data_path.display()))?;
        ensure!(
            data.num_objects() == index.num_objects() && data.checksum() == index.pack_checksum(),
            "The pack {} doesn't match its index".corrupted(),
            data_path.display()
        );
        let mut entries: Vec<(u64, u32)> = (0..index.num_objects())
            .map(|position| (index.pack_offset_at_index(position), position))
            .collect();
        entries.sort_unstable();
        let pack_end = data.pack_end() as u64;
        ensure!(
            entries
                .first()
                .is_none_or(|&(offset, _)| offset >= gix_pack::data::header::SIZE as u64)
                && entries.last().is_none_or(|&(offset, _)| offset < pack_end)
                && entries.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "The index of {} lists entries outside of the pack or several at one offset".corrupted(),
            data_path.display()
        );
        Ok(Source { data, index, entries })
    }

    /// The kind and the size of the object at `offset`, and if the entry is a delta, the id of the object that it
    /// applies to and its length, as `git pack-objects` reads them in its `check_object()`.
    fn describe(&self, offset: u64, inflate: &mut gix_zlib::Inflate) -> Result<Described> {
        let pack_path = || self.data.path().display();
        let entry = self
            .data
            .entry(offset)
            .or_raise(|| message!("Could not read the entry at offset {offset} of {}", pack_path()))?;
        let delta_base = match entry.header {
            Header::RefDelta { base_id } => Some(base_id),
            Header::OfsDelta { base_distance } => {
                let position = Header::verified_base_pack_offset(offset, base_distance).and_then(|base_offset| {
                    self.entries
                        .binary_search_by_key(&base_offset, |&(entry_offset, _)| entry_offset)
                        .ok()
                });
                let Some(position) = position else {
                    bail!(
                        "The delta at offset {offset} of {} doesn't apply to an entry before it".corrupted(),
                        pack_path()
                    );
                };
                Some(self.index.oid_at_index(self.entries[position].1).to_owned())
            }
            Header::Commit | Header::Tree | Header::Blob | Header::Tag => None,
        };
        let delta = delta_base.map(|base| (base, entry.decompressed_size));
        // Like git, look for the objects that deltas refer to by id in the same pack only.
        let resolve = |id: &oid| -> Result<Option<ResolvedBase>> {
            self.index
                .lookup(id)
                .map(|position| {
                    self.data
                        .entry(self.index.pack_offset_at_index(position))
                        .map(ResolvedBase::InPack)
                })
                .transpose()
        };
        let header = self
            .data
            .decode_header(entry, inflate, &resolve)
            .or_raise(|| message!("Could not read the object at offset {offset} of {}", pack_path()))?;
        Ok(Described {
            kind: header.kind,
            size: header.object_size,
            delta,
        })
    }
}

/// Where an object of the new pack is copied or read from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Location {
    /// The entry of [`Plan::sources`]`[source]` at `offset`, which ends at `end` and is at `position` in its index.
    Packed {
        source: usize,
        offset: u64,
        end: u64,
        position: u32,
    },
    /// A loose object.
    Loose,
}

/// What a combined pack holds at an offset.
struct Described {
    kind: gix_object::Kind,
    /// The size of the object's data.
    size: u64,
    /// The id of the object that the entry applies to and the length of the delta, if it's a delta.
    delta: Option<(ObjectId, u64)>,
}

/// An object of the new pack.
pub(crate) struct Object {
    pub(crate) id: ObjectId,
    pub(crate) location: Location,
}

/// The objects that go into the new pack, and how.
pub(crate) struct Plan {
    /// The combined packs, the one modified last first.
    pub(crate) sources: Vec<Source>,
    /// The objects of the new pack, in the order they were selected in.
    pub(crate) objects: Vec<Object>,
    /// For each object, the object that it is stored as a delta of as an index into `objects`, whether the delta is
    /// copied from its pack or was computed, or `None` if the object is stored whole.
    pub(crate) bases: Vec<Option<u32>>,
    /// For each object, the delta that the search found for it, or `None` if its delta is copied or it is stored
    /// whole.
    pub(crate) computed: Vec<Option<search::Delta>>,
    /// The indices into `objects` in the order the objects are written in, each delta after the object it applies to.
    pub(crate) order: Vec<u32>,
    /// The loose objects that couldn't be read or whose data doesn't hash to their name.
    pub(crate) skipped_loose_objects: Vec<ObjectId>,
}

impl Plan {
    /// Select the objects of the `combined` packs and the loose objects of the object database in `objects_dir` that
    /// neither the `retained` nor the `kept` packs hold, as `git pack-objects --stdin-packs --unpacked` does with
    /// `--honor-pack-keep` if the `combined` packs are its included and the `retained` packs its excluded packs. Plan
    /// the deltas of the new pack as it does: copy deltas in chains of at most [`Options::delta_depth`] deltas, and
    /// search deltas for the other objects with the name hashes of its walk through the history of the combined packs
    /// and the loose commits.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        objects_dir: &Path,
        object_hash: gix_hash::Kind,
        combined: &[&Pack],
        retained: &[&Pack],
        kept: &[&Pack],
        options: &Options,
        progress: &mut dyn DynNestedProgress,
        should_interrupt: &AtomicBool,
    ) -> Result<Self> {
        let mut selecting = progress.add_child("select objects".into());
        selecting.init(None, progress::count("objects"));
        let open_indices = |packs: &[&Pack]| {
            packs
                .iter()
                .map(|pack| open_index(pack, object_hash))
                .collect::<Result<Vec<_>>>()
        };
        let (retained, kept) = (open_indices(retained)?, open_indices(kept)?);
        let is_retained = |id: &oid| retained.iter().any(|index| index.lookup(id).is_some());
        let is_excluded = |id: &oid| is_retained(id) || kept.iter().any(|index| index.lookup(id).is_some());

        // Like git, list the objects of the combined pack modified last first, and copy each object from the first
        // combined pack that holds it.
        let mut combined = combined.to_vec();
        combined.sort_by(|a, b| {
            seconds(b.modified)
                .cmp(&seconds(a.modified))
                .then_with(|| a.index_path.cmp(&b.index_path))
        });
        let sources = combined
            .iter()
            .map(|pack| Source::open(pack, object_hash))
            .collect::<Result<Vec<_>>>()?;

        let mut selection = Selection::default();
        // The commits that the walk for name hashes starts at.
        let mut commits = Vec::new();
        let mut inflate = gix_zlib::Inflate::default();
        for (source_index, source) in sources.iter().enumerate() {
            for (entry, &(offset, position)) in source.entries.iter().enumerate() {
                check_interrupt(entry, should_interrupt)?;
                let id = source.index.oid_at_index(position).to_owned();
                if selection.contains(&id) {
                    continue;
                }
                let described = source.describe(offset, &mut inflate)?;
                // Like git, start the walk at the commits of the combined packs even if a pack that remains holds them.
                if described.kind == gix_object::Kind::Commit {
                    commits.push(id);
                }
                if is_excluded(&id) {
                    continue;
                }
                let end = source
                    .entries
                    .get(entry + 1)
                    .map_or(source.data.pack_end() as u64, |&(next_offset, _)| next_offset);
                selection.add(
                    id,
                    Location::Packed {
                        source: source_index,
                        offset,
                        end,
                        position,
                    },
                    Selected {
                        kind: described.kind,
                        size: described.size,
                        in_pack: Some(InPack {
                            pack_id: source_index as u32,
                            is_delta: described.delta.is_some(),
                        }),
                        delta: described.delta,
                    },
                )?;
                selecting.inc();
            }
        }

        let store = crate::loose::Store::at(objects_dir, object_hash);
        let mut skipped_loose_objects = Vec::new();
        let mut buf = Vec::new();
        for (count, id) in loose::list(objects_dir, object_hash)?.into_iter().enumerate() {
            check_interrupt(count, should_interrupt)?;
            if selection.contains(&id) || is_excluded(&id) {
                // Like git, start the walk at every loose commit, even one that a pack holds.
                if let Ok(Some((_, kind))) = store.try_header(&id)
                    && kind == gix_object::Kind::Commit
                {
                    commits.push(id);
                }
                continue;
            }
            match store.try_find(&id, &mut buf) {
                Ok(Some(data))
                    if gix_object::compute_hash(object_hash, data.kind, data.data).is_ok_and(|actual| actual == id) =>
                {
                    if data.kind == gix_object::Kind::Commit {
                        commits.push(id);
                    }
                    selection.add(
                        id,
                        Location::Loose,
                        Selected {
                            kind: data.kind,
                            size: data.data.len() as u64,
                            in_pack: None,
                            delta: None,
                        },
                    )?;
                    selecting.inc();
                }
                // The object was removed since its directory was listed.
                Ok(None) => {}
                _ => {
                    gix_trace::warn!(
                        "Leaving the loose object {} as it is, as it can't be read or its data doesn't hash to its name",
                        id
                    );
                    skipped_loose_objects.push(id);
                }
            }
        }

        let Selection {
            objects,
            selected,
            indices,
        } = selection;
        let mut bases: Vec<Option<u32>> = selected
            .iter()
            .map(|selected| selected.delta.and_then(|(base_id, _)| indices.get(&base_id).copied()))
            .collect();
        // Like git, compare the length of a delta that can be copied with `core.bigFileThreshold` before chains are
        // cut.
        let facts: Vec<Facts> = selected
            .iter()
            .zip(&bases)
            .map(|(selected, base)| Facts {
                kind: selected.kind,
                size: selected.size,
                threshold_size: match (base, selected.delta) {
                    (Some(_), Some((_, delta_size))) => delta_size,
                    _ => selected.size,
                },
                in_pack: selected.in_pack,
            })
            .collect();
        drop(selected);
        // As each object is copied from the first combined pack that holds it, deltas form cycles only where a pack
        // holds a delta whose base it doesn't hold. Chains can be longer than `delta_depth` in any pack.
        chain::break_chains(&mut bases, options.delta_depth.min(search::MAX_DEPTH));

        let mut objects_handle = crate::at(objects_dir, object_hash)
            .and_then(crate::Handle::into_arc)
            .or_raise(|| message!("Could not open the object database at {}", objects_dir.display()))?;
        // Like git, read the objects for the walk and the search through one cache of delta bases.
        objects_handle.set_pack_cache(|| Box::new(MemoryCappedHashmap::new(DELTA_BASE_CACHE_LIMIT)));
        let mut name_hashes = vec![0; objects.len()];
        names::assign(
            &objects_handle,
            &commits,
            is_retained,
            &indices,
            &mut name_hashes,
            should_interrupt,
        )?;
        drop((commits, indices));
        let mut searching = progress.add_child("find deltas".into());
        searching.init(None, progress::count("objects"));
        let computed = deltas::find(
            &objects,
            &facts,
            &name_hashes,
            &mut bases,
            objects_handle,
            options,
            &searching,
            should_interrupt,
        )?;
        // Without tags among the objects, git writes them in the order of the list, each delta after its base. It knows
        // only the kind of the entry of a copied delta, and orders it with neither commits nor trees.
        let order = chain::write_order(
            &bases
                .iter()
                .zip(&facts)
                .zip(&computed)
                .map(|((&base, facts), computed)| chain::Object {
                    base,
                    kind: (base.is_none() || computed.is_some()).then_some(facts.kind),
                    tagged: false,
                    preferred_base: false,
                })
                .collect::<Vec<_>>(),
        );
        Ok(Plan {
            sources,
            objects,
            bases,
            computed,
            order,
            skipped_loose_objects,
        })
    }
}

/// What is known of a selected object when it is selected.
struct Selected {
    kind: gix_object::Kind,
    size: u64,
    in_pack: Option<InPack>,
    /// The id of the object that its pack stores it as a delta of, and the length of the delta.
    delta: Option<(ObjectId, u64)>,
}

/// The objects selected so far.
#[derive(Default)]
struct Selection {
    objects: Vec<Object>,
    /// What is known of each object in `objects`.
    selected: Vec<Selected>,
    /// The index of each selected object in `objects`.
    indices: gix_hashtable::HashMap<ObjectId, u32>,
}

impl Selection {
    fn contains(&self, id: &ObjectId) -> bool {
        self.indices.contains_key(id)
    }

    fn add(&mut self, id: ObjectId, location: Location, selected: Selected) -> Result {
        let Some(index) = u32::try_from(self.objects.len()).ok().filter(|&index| index < u32::MAX) else {
            bail!("The new pack would hold more objects than a pack can".unsupported());
        };
        self.indices.insert(id, index);
        self.objects.push(Object { id, location });
        self.selected.push(selected);
        Ok(())
    }
}

fn open_index(pack: &Pack, object_hash: gix_hash::Kind) -> Result<gix_pack::index::File> {
    gix_pack::index::File::at(&pack.index_path, object_hash)
        .or_raise(|| message!("Could not open the pack index {}", pack.index_path.display()))
}

fn check_interrupt(count: usize, should_interrupt: &AtomicBool) -> Result {
    if count.is_multiple_of(OBJECTS_PER_INTERRUPT_CHECK) && should_interrupt.load(Ordering::Relaxed) {
        bail!(cancelled("Interrupted while selecting the objects of the new pack"));
    }
    Ok(())
}

/// The whole seconds since the epoch of `time`, rounded down, as git compares the times at which packs were modified.
fn seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => {
            let before = before.duration();
            i64::try_from(before.as_secs()).map_or(i64::MIN, |seconds| -seconds - i64::from(before.subsec_nanos() > 0))
        }
    }
}
