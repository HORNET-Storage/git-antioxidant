pub(crate) mod function {
    use gix_error::Result;
    use std::{
        cmp::Ordering,
        sync::{Arc, atomic::AtomicBool},
    };

    use gix_error::{OptionExt, ResultExt, message, not_found};
    use gix_parallel as parallel;
    use gix_parallel::SequenceId;
    use gix_utils::progress::{
        Progress,
        prodash::{Count, DynNestedProgress},
    };

    use super::{Mode, Options, Outcome, ProgressId, reduce, util};
    use crate::data::output::{self, entry::deltas};

    /// Given a known list of object `counts`, calculate entries ready to be put into a data pack.
    ///
    /// This allows objects to be written quite soon without having to wait for the entire pack to be built in memory.
    /// A chunk of objects is held in memory and compressed using DEFLATE, and serve the output of this iterator.
    /// That way slow writers will naturally apply back pressure, and communicate to the implementation that more time can be
    /// spent compressing objects.
    ///
    /// * `counts`
    ///   * A list of previously counted objects to add to the pack. Duplication checks are not performed, no object is expected to be duplicated.
    ///   * [Preferred bases](output::Count::preferred_base) aren't written, so the pack has one entry for each of the other
    ///     counts.
    /// * `progress`
    ///   * a way to obtain progress information
    /// * `should_interrupt`
    ///   * is checked while [`Mode::PackCopyAndDeltaSearch`] plans the entries, which then stops with an error once it is
    ///     set.
    /// * `options`
    ///   * more configuration
    ///
    /// Returns an iterator over entry chunks, or an error if resolving pack locations fails before iteration, or if planning
    /// the entries fails.
    ///
    /// ## Discussion
    ///
    /// ### Advantages
    ///
    /// * Begins writing immediately and supports back-pressure, unless the deltas of [`Mode::PackCopyAndDeltaSearch`] are
    ///   searched first.
    /// * Abstract over object databases and how input is provided.
    ///
    /// ### Disadvantages
    ///
    /// * ~~currently there is no way to easily write the pack index, even though the state here is uniquely positioned to do
    ///   so with minimal overhead (especially compared to `gix index-from-pack`)~~ Probably works now by chaining Iterators
    ///   or keeping enough state to write a pack and then generate an index with recorded data.
    ///
    pub fn iter_from_counts<Find>(
        mut counts: Vec<output::Count>,
        db: Find,
        mut progress: Box<dyn DynNestedProgress + 'static>,
        should_interrupt: &AtomicBool,
        Options {
            version,
            mode,
            allow_thin_pack,
            thread_limit,
            chunk_size,
            compression,
        }: Options,
    ) -> Result<
        impl Iterator<Item = Result<(SequenceId, Vec<output::Entry>)>>
        + parallel::Finalize<Reduce = reduce::Statistics<gix_error::Error>>
        + use<Find>,
    >
    where
        Find: crate::Find + Send + Clone + 'static,
    {
        // Preferred bases are only the bases of the deltas that the search finds for a thin pack, and aren't written.
        if !allow_thin_pack || !matches!(mode, Mode::PackCopyAndDeltaSearch(_)) {
            counts.retain(|count| !count.preferred_base);
        }
        let search_thread_limit = thread_limit;
        let (chunk_size, thread_limit, _) =
            parallel::optimize_chunk_size_and_thread_limit(chunk_size, Some(counts.len()), thread_limit, None);
        {
            let progress = Arc::new(parking_lot::Mutex::new(
                progress.add_child_with_id("resolving".into(), ProgressId::ResolveCounts.into()),
            ));
            progress.lock().init(None, gix_utils::progress::count("counts"));
            let enough_counts_present = counts.len() > 4_000;
            let start = std::time::Instant::now();
            parallel::in_parallel_if(
                || enough_counts_present,
                counts.chunks_mut(chunk_size),
                thread_limit,
                |_n| Vec::<u8>::new(),
                {
                    let progress = Arc::clone(&progress);
                    let db = db.clone();
                    move |chunk, buf| {
                        let chunk_size = chunk.len();
                        for count in chunk {
                            use crate::data::output::count::PackLocation::*;
                            match count.entry_pack_location {
                                LookedUp(_) => continue,
                                NotLookedUp => {
                                    count.entry_pack_location = LookedUp(db.location_by_oid(&count.id, buf)?);
                                }
                            }
                        }
                        progress.lock().inc_by(chunk_size);
                        Ok::<_, gix_error::Error>(())
                    }
                },
                parallel::IdentityWithResult::<(), gix_error::Error>::default(),
            )?;
            progress.lock().show_throughput(start);
        }
        let work = match mode {
            Mode::PackCopyAndBaseObjects => {
                let mut progress = progress.add_child_with_id("sorting".into(), ProgressId::SortEntries.into());
                progress.init(Some(counts.len()), gix_utils::progress::count("counts"));
                let start = std::time::Instant::now();

                use crate::data::output::count::PackLocation::*;
                counts.sort_by(|lhs, rhs| match (&lhs.entry_pack_location, &rhs.entry_pack_location) {
                    (LookedUp(None), LookedUp(None)) => Ordering::Equal,
                    (LookedUp(Some(_)), LookedUp(None)) => Ordering::Greater,
                    (LookedUp(None), LookedUp(Some(_))) => Ordering::Less,
                    (LookedUp(Some(lhs)), LookedUp(Some(rhs))) => lhs
                        .pack_id
                        .cmp(&rhs.pack_id)
                        .then(lhs.pack_offset.cmp(&rhs.pack_offset)),
                    (_, _) => unreachable!("counts were resolved beforehand"),
                });

                let mut index: Vec<(u32, std::ops::Range<usize>)> = Vec::new();
                let mut chunks_pack_start = counts.partition_point(|e| e.entry_pack_location.is_none());
                let mut slice = &counts[chunks_pack_start..];
                while !slice.is_empty() {
                    let current_pack_id = slice[0].entry_pack_location.as_ref().expect("packed object").pack_id;
                    let pack_end = slice.partition_point(|e| {
                        e.entry_pack_location.as_ref().expect("packed object").pack_id == current_pack_id
                    });
                    index.push((current_pack_id, chunks_pack_start..chunks_pack_start + pack_end));
                    slice = &slice[pack_end..];
                    chunks_pack_start += pack_end;
                }

                progress.set(counts.len());
                progress.show_throughput(start);

                Work::Copy {
                    counts_range_by_pack_id: index,
                }
            }
            Mode::PackCopyAndDeltaSearch(search) => Work::Deltas(deltas::Plan::new(
                &counts,
                &db,
                search,
                version,
                chunk_size,
                search_thread_limit,
                &mut *progress,
                should_interrupt,
            )?),
        };
        let num_entries = match &work {
            Work::Copy { .. } => counts.len(),
            Work::Deltas(plan) => plan.order.len(),
        };

        let counts = Arc::new(counts);
        let work = Arc::new(work);
        let progress = Arc::new(parking_lot::Mutex::new(progress));
        let chunks = util::ChunkRanges::new(chunk_size, num_entries);

        Ok(parallel::Stepwise::new(
            chunks.enumerate(),
            thread_limit,
            {
                let progress = Arc::clone(&progress);
                move |n| {
                    (
                        Vec::new(), // object data buffer
                        progress
                            .lock()
                            .add_child_with_id(format!("thread {n}"), gix_utils::progress::UNKNOWN),
                    )
                }
            },
            {
                let counts = Arc::clone(&counts);
                move |(chunk_id, chunk_range): (SequenceId, std::ops::Range<usize>),
                      (buf, progress)|
                      -> Result<(SequenceId, Vec<output::Entry>, Outcome)> {
                    let mut stats = Outcome::default();
                    progress.init(Some(chunk_range.len()), gix_utils::progress::count("objects"));
                    let counts_range_by_pack_id = match &*work {
                        Work::Copy {
                            counts_range_by_pack_id,
                        } => counts_range_by_pack_id,
                        Work::Deltas(plan) => {
                            let out = plan.entries(chunk_range, &counts, &db, version, compression, buf, &mut stats)?;
                            progress.inc_by(out.len());
                            return Ok((chunk_id, out, stats));
                        }
                    };
                    let mut out = Vec::new();
                    let chunk = &counts[chunk_range];
                    let mut pack_offsets_to_id = None;

                    for count in chunk.iter() {
                        out.push(match count
                            .entry_pack_location
                            .as_ref()
                            .and_then(|l| db.entry_by_location(l).map(|pe| (l, pe)))
                        {
                            Some((location, pack_entry)) => {
                                if let Some((cached_pack_id, _)) = &pack_offsets_to_id
                                    && *cached_pack_id != location.pack_id
                                {
                                    pack_offsets_to_id = None;
                                }
                                let pack_range = counts_range_by_pack_id[counts_range_by_pack_id
                                    .binary_search_by_key(&location.pack_id, |e| e.0)
                                    .expect("pack-id always present")]
                                .1
                                .clone();
                                let base_index_offset = pack_range.start;
                                let counts_in_pack = &counts[pack_range];
                                let entry = output::Entry::from_pack_entry(
                                    pack_entry,
                                    count,
                                    counts_in_pack,
                                    base_index_offset,
                                    allow_thin_pack.then_some({
                                        |pack_id, base_offset| {
                                            if pack_offsets_to_id.is_none() {
                                                let mut offsets =
                                                    db.pack_offsets_and_oid(pack_id)?.ok_or_raise(|| {
                                                        not_found("Pack used for counts is no longer available")
                                                    })?;
                                                offsets.sort_by_key(|e| e.0);
                                                pack_offsets_to_id = Some((pack_id, offsets));
                                            }
                                            let (cached_pack_id, cache) =
                                                pack_offsets_to_id.as_ref().expect("just set");
                                            debug_assert_eq!(*cached_pack_id, pack_id);
                                            stats.ref_delta_objects += 1;
                                            Ok(cache
                                                .binary_search_by_key(&base_offset, |e| e.0)
                                                .ok()
                                                .map(|idx| cache[idx].1))
                                        }
                                    }),
                                    version,
                                );
                                match entry {
                                    Some(entry) => {
                                        stats.objects_copied_from_pack += 1;
                                        entry
                                    }
                                    None => {
                                        match db
                                            .try_find(&count.id, buf)
                                            .or_raise(|| message("Could not find object while generating pack"))?
                                        {
                                            Some((obj, _location)) => {
                                                stats.decoded_and_recompressed_objects += 1;
                                                output::Entry::from_data(count, &obj, compression)
                                            }
                                            None => {
                                                stats.missing_objects += 1;
                                                Ok(output::Entry::invalid())
                                            }
                                        }
                                    }
                                }
                            }
                            None => match db
                                .try_find(&count.id, buf)
                                .or_raise(|| message("Could not find object while generating pack"))?
                            {
                                Some((obj, _location)) => {
                                    stats.decoded_and_recompressed_objects += 1;
                                    output::Entry::from_data(count, &obj, compression)
                                }
                                None => {
                                    stats.missing_objects += 1;
                                    Ok(output::Entry::invalid())
                                }
                            },
                        }?);
                        progress.inc();
                    }
                    Ok((chunk_id, out, stats))
                }
            },
            reduce::Statistics::default(),
        ))
    }

    /// What the entries are made from.
    enum Work {
        /// The counts are ordered by their location, and the counts of each pack are at its range.
        Copy {
            counts_range_by_pack_id: Vec<(u32, std::ops::Range<usize>)>,
        },
        /// The entries of [`Mode::PackCopyAndDeltaSearch`].
        Deltas(deltas::Plan),
    }
}

mod util {
    #[derive(Clone)]
    pub struct ChunkRanges {
        cursor: usize,
        size: usize,
        len: usize,
    }

    impl ChunkRanges {
        pub fn new(size: usize, total: usize) -> Self {
            ChunkRanges {
                cursor: 0,
                size,
                len: total,
            }
        }
    }

    impl Iterator for ChunkRanges {
        type Item = std::ops::Range<usize>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.cursor >= self.len {
                None
            } else {
                let upper = (self.cursor + self.size).min(self.len);
                let range = self.cursor..upper;
                self.cursor = upper;
                Some(range)
            }
        }
    }
}

mod reduce {
    use std::marker::PhantomData;

    use gix_parallel as parallel;
    use gix_parallel::SequenceId;

    use super::Outcome;
    use crate::data::output;

    pub struct Statistics<E> {
        total: Outcome,
        _err: PhantomData<E>,
    }

    impl<E> Default for Statistics<E> {
        fn default() -> Self {
            Statistics {
                total: Default::default(),
                _err: PhantomData,
            }
        }
    }

    impl<Error> parallel::Reduce for Statistics<Error> {
        type Input = Result<(SequenceId, Vec<output::Entry>, Outcome), Error>;
        type FeedProduce = (SequenceId, Vec<output::Entry>);
        type Output = Outcome;
        type Error = Error;

        fn feed(&mut self, item: Self::Input) -> Result<Self::FeedProduce, Self::Error> {
            item.map(|(cid, entries, stats)| {
                self.total.aggregate(stats);
                (cid, entries)
            })
        }

        fn finalize(self) -> Result<Self::Output, Self::Error> {
            Ok(self.total)
        }
    }
}

mod types {
    /// Information gathered during the run of [`iter_from_counts()`][crate::data::output::entry::iter_from_counts()].
    #[derive(Default, PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub struct Outcome {
        /// The amount of fully decoded objects. These are the most expensive as they are fully decoded.
        pub decoded_and_recompressed_objects: usize,
        /// The amount of objects that could not be located despite them being mentioned during iteration
        pub missing_objects: usize,
        /// The amount of base or delta objects that could be copied directly from the pack. These are cheapest as they
        /// only cost a memory copy for the most part.
        pub objects_copied_from_pack: usize,
        /// The amount of objects that ref to their base as ref-delta, an indication for a thin back being created.
        pub ref_delta_objects: usize,
        /// The amount of objects that are stored as the deltas that the search of [`Mode::PackCopyAndDeltaSearch`]
        /// found for them.
        pub computed_delta_objects: usize,
    }

    impl Outcome {
        pub(in crate::data::output::entry) fn aggregate(
            &mut self,
            Outcome {
                decoded_and_recompressed_objects: decoded_objects,
                missing_objects,
                objects_copied_from_pack,
                ref_delta_objects,
                computed_delta_objects,
            }: Self,
        ) {
            self.decoded_and_recompressed_objects += decoded_objects;
            self.missing_objects += missing_objects;
            self.objects_copied_from_pack += objects_copied_from_pack;
            self.ref_delta_objects += ref_delta_objects;
            self.computed_delta_objects += computed_delta_objects;
        }
    }

    /// The way the iterator operates.
    #[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub enum Mode {
        /// Copy base objects and deltas from packs, while non-packed objects will be treated as base objects
        /// (i.e. without trying to delta compress them). This is a fast way of obtaining a back while benefiting
        /// from existing pack compression and spending the smallest possible time on compressing unpacked objects at
        /// the cost of bandwidth.
        PackCopyAndBaseObjects,
        /// Copy objects and deltas from packs, search deltas for the other objects, and write the entries in the order of
        /// `git pack-objects`, each delta after its base, as it does with the given options:
        ///
        /// * A delta that a pack stores is copied if it refers to its base by offset and the base is among the counts, at the
        ///   same location or another, as Git's `check_object()` copies deltas whose base it packs. Then the chains of copied
        ///   deltas are cut where they are longer than [`DeltaSearch::depth`], as Git's `break_delta_chains()` cuts them.
        /// * The other objects that aren't larger than [`DeltaSearch::big_file_threshold`] are searched for deltas in the
        ///   order of the counts and with their [name hashes](crate::data::output::Count::name_hash), as Git's
        ///   `prepare_pack()` searches them with [`find_deltas()`](crate::data::delta::search::find_deltas()), in up to
        ///   [`Options::thread_limit`] threads, along with the [preferred bases](crate::data::output::Count::preferred_base)
        ///   among the counts if [`Options::allow_thin_pack`] is set. The delta that the search finds for an object is
        ///   written with [`Entry::from_found_delta()`](crate::data::output::Entry::from_found_delta()), against an entry
        ///   written before it, or against a preferred base, which the delta refers to by its id. The objects that the
        ///   search finds no delta for are copied where a pack stores them whole, and compressed whole otherwise.
        /// * The entries come in the order of [`write_order()`](crate::data::delta::chain::write_order()), as Git's
        ///   `compute_write_order()` and `write_one()` order them, with the objects of
        ///   [tagged](crate::data::output::Count::tagged) counts moved forward.
        ///
        /// The objects are examined and their deltas searched before the first entry is produced.
        ///
        /// ### Deviation
        ///
        /// * Git also copies deltas that refer to their base by id if it packs the base, and deltas whose base its
        ///   reachability bitmaps show the receiver of a thin pack to have, while here the objects of such deltas are
        ///   searched for deltas instead, as [`PackCopyAndBaseObjects`](Mode::PackCopyAndBaseObjects) doesn't copy them
        ///   either.
        /// * The `delta` attribute isn't read, so objects whose `delta` attribute is unset are searched like the others.
        PackCopyAndDeltaSearch(DeltaSearch),
    }

    /// The options of the delta search of [`Mode::PackCopyAndDeltaSearch`], with the meanings and the defaults of
    /// `git pack-objects`.
    #[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub struct DeltaSearch {
        /// How many of the objects before an object are tried as its base (`pack.window`). With 0, no deltas are
        /// computed.
        pub window: usize,
        /// The longest chain of deltas (`pack.depth`), which Git limits to 4095. With 0, no deltas are computed, and the
        /// objects that packs store as deltas are stored whole.
        pub depth: u32,
        /// The most bytes that the data and indices of the objects in the window of each thread may take
        /// (`pack.windowMemory`), or 0 for no limit.
        pub window_memory: u64,
        /// The most bytes of deltas that are kept in memory until they are written (`pack.deltaCacheSize`), or 0 for no
        /// limit. The other deltas are computed again when they are written.
        pub delta_cache_size: u64,
        /// Deltas shorter than this many bytes are kept while they fit (`pack.deltaCacheLimit`), which Git limits to 1 MiB
        /// less one byte.
        pub delta_cache_limit: u64,
        /// Objects larger than this many bytes aren't searched for deltas (`core.bigFileThreshold`). For an object that a
        /// pack stores as a delta that could be copied, the length of that delta counts, as in Git.
        pub big_file_threshold: u64,
    }

    impl Default for DeltaSearch {
        /// The defaults of `git pack-objects`: a window of 10 objects without a memory limit, a depth of 50, up to 256 MiB
        /// of kept deltas with a limit of 1000 bytes for short ones, and a threshold of 512 MiB for large objects.
        fn default() -> Self {
            DeltaSearch {
                window: 10,
                depth: 50,
                window_memory: 0,
                delta_cache_size: 256 * 1024 * 1024,
                delta_cache_limit: 1000,
                big_file_threshold: 512 * 1024 * 1024,
            }
        }
    }

    /// Configuration options for the pack generation functions provided in [`iter_from_counts()`][crate::data::output::entry::iter_from_counts()].
    #[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub struct Options {
        /// The amount of threads to use at most when resolving the pack, and for the delta search of
        /// [`Mode::PackCopyAndDeltaSearch`] (`pack.threads`). If `None`, all logical cores are used.
        pub thread_limit: Option<usize>,
        /// The algorithm to produce a pack
        pub mode: Mode,
        /// If set, the resulting back can have deltas that refer to an object which is not in the pack. This can happen
        /// if the initial counted objects do not contain an object that an existing packed delta refers to, for example, because
        /// it wasn't part of the iteration, for instance when the iteration was performed on tree deltas or only a part of the
        /// commit graph. Please note that thin packs are not valid packs at rest, thus they are only valid for packs in transit.
        ///
        /// If set to false, delta objects will be decompressed and recompressed as base objects.
        ///
        /// With [`Mode::PackCopyAndDeltaSearch`], deltas refer to objects outside of the pack only if these are
        /// [preferred bases](crate::data::output::Count::preferred_base) among the counts, which are used only if this is
        /// set.
        pub allow_thin_pack: bool,
        /// The amount of objects per chunk or unit of work to be sent to threads for processing
        /// TODO: could this become the window size?
        pub chunk_size: usize,
        /// The pack data version to produce for each entry
        pub version: crate::data::Version,
        /// The compression level to use for objects that are not copied from an existing pack,
        /// but deflated from their object data.
        ///
        /// Defaults to [`Compression::DEFAULT`](gix_zlib::Compression::DEFAULT), which is
        /// also the default that `git` uses when writing packs, unless configured otherwise
        /// with `pack.compression`.
        pub compression: gix_zlib::Compression,
    }

    impl Default for Options {
        fn default() -> Self {
            Options {
                thread_limit: None,
                mode: Mode::PackCopyAndBaseObjects,
                allow_thin_pack: false,
                chunk_size: 10,
                version: Default::default(),
                compression: gix_zlib::Compression::DEFAULT,
            }
        }
    }

    /// The progress ids used in [`write_to_directory()`][crate::Bundle::write_to_directory()].
    ///
    /// Use this information to selectively extract the progress of interest in case the parent application has custom visualization.
    #[derive(Debug, Copy, Clone)]
    pub enum ProgressId {
        /// The amount of [`Count`][crate::data::output::Count] objects which are resolved to their pack location.
        ResolveCounts,
        /// Layout pack entries for placement into a pack (by pack-id and by offset).
        SortEntries,
        /// The amount of objects whose kind, size and stored delta are examined for the delta search of
        /// [`Mode::PackCopyAndDeltaSearch`].
        ExamineCounts,
        /// The amount of objects that the delta search of [`Mode::PackCopyAndDeltaSearch`] tried as deltas.
        FindDeltas,
    }

    impl From<ProgressId> for gix_utils::progress::Id {
        fn from(v: ProgressId) -> Self {
            match v {
                ProgressId::ResolveCounts => *b"ECRC",
                ProgressId::SortEntries => *b"ECSE",
                ProgressId::ExamineCounts => *b"ECEC",
                ProgressId::FindDeltas => *b"ECFD",
            }
        }
    }
}
pub use types::{DeltaSearch, Mode, Options, Outcome, ProgressId};
