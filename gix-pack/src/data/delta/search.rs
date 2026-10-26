//! Choose the bases that objects are stored as deltas of, as `git pack-objects` does.
//!
//! [`find_deltas()`] is the delta search of `git pack-objects`. It leaves out objects smaller than 50 bytes and sorts the
//! others by kind (tags, blobs, trees, then commits), then by [name hash](Object::name_hash), with the
//! [preferred bases](Object::preferred_base) of each name hash first, then by size, the largest values first, and
//! finally in the order of `objects`. Each object that isn't a preferred base is tried as a delta of the
//! [`window`](Options::window) objects before it, the nearest first, until one of another kind comes, and it is stored
//! as the last delta that was accepted. With `m` being the [`depth`](Options::depth) less the
//! [`dependents_depth`](Object::dependents_depth) of the object:
//!
//! * The first delta of an object of `s` bytes, with ids of `h` bytes, may have `(s / 2 - h) × (m - d) / m` bytes if its
//!   base is at depth `d`. Once a delta of `l` bytes at depth `r` was accepted, a delta against a base at depth `d` may
//!   have `l × (m - d) / (m - r + 1)` bytes, and one of exactly `l` bytes is only accepted if it is less deep.
//! * Bases at depth `m` aren't tried, nor are bases more than 32 times as large as the object, or bases that the object is
//!   larger than by at least the length that its delta may have.
//! * With [`reuse_delta`](Options::reuse_delta), an object that a pack stores whole isn't tried as a delta of the other
//!   objects of that pack, other than preferred bases, as they were searched for deltas when that pack was made.
//!
//! When an object is stored as a delta, its base moves to the place after the object in the window, so that the next
//! object tries that base first, unless the delta has depth `m`: such an object can't be a base, and its place is taken
//! by the next object. With [`window_memory`](Options::window_memory), the oldest objects leave the window before each
//! object is tried while the data and indices of those in it take more than that many bytes, but the object right
//! before always stays. Deltas are kept in memory, up to [`delta_cache_size`](Options::delta_cache_size) bytes in all,
//! if they are shorter than [`delta_cache_limit`](Options::delta_cache_limit) bytes or if their length in KiB is less
//! than the size of their base in MiB plus half the size of their object in MiB. A pack writer computes the other
//! deltas again with [`Index::encode()`], which gives the same bytes.
//!
//! With several [threads](Options::thread_limit), the sorted objects are divided into one part per thread, each extended
//! over the following objects with the same name hash as its last object, other than 0, and the part of each thread
//! but the last stays empty if its share would have fewer than `2 × (window + 1)` objects. A thread that finishes its
//! part takes the second half of the objects left in the part with the most of them, if those are more than
//! `2 × (window + 1)`, starting at the first object in that half whose name hash is 0 or differs from that of the object
//! before, if there is one. Each part starts with an empty window, so the deltas depend on the speed of the threads, as
//! they do in Git. With a single thread, they are the deltas of `git pack-objects --threads=1`.
//!
//! The memory of the window is counted with the sizes that Git's structures have on the platform, as Git counts it.
//! Other sizes are 64-bit numbers, while Git for Windows counts some of them in 32 bits, which only makes a difference
//! for objects and windows of 4 GiB or more.
//!
//! ### Deviation
//!
//! * When Git writes a pack to a file rather than to its standard output, it compresses the deltas that it keeps right
//!   away and counts their compressed length against the cache, which then holds more of them. This changes how many
//!   deltas a writer computes again, but not the pack. Here, kept deltas aren't compressed.
//! * Delta islands (`pack.island`) and the path walk (`--path-walk`, `pack.usePathWalk`) aren't supported.
use std::{
    cmp::Ordering,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};

use gix_error::{Result, ResultExt, bail, cancelled, ensure, message};
use gix_hash::ObjectId;
use gix_utils::progress::{Count, StepShared};

use crate::data::delta::Index;

/// Objects smaller than this many bytes are neither deltas nor bases.
const MIN_SIZE: u64 = 50;

/// The name hash of `path` that `git pack-objects` sorts objects by, that of `--name-hash-version=1`, its default.
///
/// It is made of the bytes of `path` other than spaces, tabs, line feeds and carriage returns, mostly of the last 16 of
/// them and of the last one most, so that objects whose paths end alike, such as files with the same name or extension,
/// sort together.
pub fn name_hash(path: &[u8]) -> u32 {
    path.iter()
        .filter(|&&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .fold(0, |hash: u32, &byte| (hash >> 2).wrapping_add(u32::from(byte) << 24))
}

/// An object for [`find_deltas()`], with what `git pack-objects` knows of it when it looks for deltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// The id of the object.
    pub id: ObjectId,
    /// The kind of the object.
    pub kind: gix_object::Kind,
    /// The size of the object's data.
    pub size: u64,
    /// The [`name_hash()`] of the path that the object was found at, or 0 for objects without one, such as commits.
    pub name_hash: u32,
    /// Whether the object is only a base: the receiver of the pack has it, so that the pack doesn't contain it. It is
    /// skipped if it can't be read.
    pub preferred_base: bool,
    /// The pack that holds the object, if any, for [`Options::reuse_delta`].
    pub in_pack: Option<InPack>,
    /// The amount of deltas in the longest chain of deltas that the pack copies from another pack and that depend on the
    /// object, directly or through other deltas, so that the delta of the object doesn't make them deeper than
    /// [`Options::depth`], or 0 if there are none.
    pub dependents_depth: u32,
}

/// The pack that holds an [`Object`], and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InPack {
    /// An id of the pack, the same for all objects that it holds.
    pub pack_id: u32,
    /// Whether the pack stores the object as a delta.
    pub is_delta: bool,
}

/// Options for [`find_deltas()`], with the meanings and the defaults of `git pack-objects`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How many of the objects before an object are tried as its base (`pack.window`). With 0, no deltas are computed.
    pub window: usize,
    /// The most bytes that the data and indices of the objects in the window of each thread may take
    /// (`pack.windowMemory`), or 0 for no limit. The window then holds fewer objects, but always the one before the
    /// object that is tried.
    pub window_memory: u64,
    /// The longest chain of deltas (`pack.depth`), which Git limits to 4095. With 0, no deltas are computed.
    pub depth: u32,
    /// Whether an object that a pack stores whole isn't tried as a delta of the other objects of that pack, other than
    /// preferred bases, as `git pack-objects` does unless it is given `--no-reuse-delta` or `--no-reuse-object`.
    pub reuse_delta: bool,
    /// The most bytes of deltas that are kept in memory in all (`pack.deltaCacheSize`), or 0 for no limit.
    pub delta_cache_size: u64,
    /// Deltas shorter than this many bytes are kept while they fit (`pack.deltaCacheLimit`), which Git limits to 1 MiB
    /// less one byte; longer ones only if they are short compared to their objects.
    pub delta_cache_limit: u64,
    /// The amount of threads (`pack.threads`), or `None` or `Some(0)` for as many as there are logical cores. Without the
    /// `parallel` feature, one thread is used.
    pub thread_limit: Option<usize>,
}

impl Default for Options {
    /// The defaults of `git pack-objects`: a window of 10 objects without a memory limit, a depth of 50, delta reuse, up
    /// to 256 MiB of kept deltas with a limit of 1000 bytes for short ones, and as many threads as there are logical
    /// cores.
    fn default() -> Self {
        Options {
            window: 10,
            window_memory: 0,
            depth: 50,
            reuse_delta: true,
            delta_cache_size: 256 * 1024 * 1024,
            delta_cache_limit: 1000,
            thread_limit: None,
        }
    }
}

/// The delta that [`find_deltas()`] found for an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    /// The index of the base in the objects that were searched.
    pub base: usize,
    /// The length of the delta.
    pub size: usize,
    /// The delta, if it was kept within [`Options::delta_cache_size`]. Otherwise [`Index::encode()`] computes the same
    /// bytes from the base and the object.
    pub data: Option<Vec<u8>>,
}

/// Find the base that each object of `objects` is stored as a delta of, as the delta search of `git pack-objects` does,
/// and return the delta of each object at its index, or `None` for objects that are stored whole and preferred bases.
///
/// `objects` are what `git pack-objects` searches: the objects of the pack in the order in which it lists them, the
/// newest first, and its preferred bases, but not the objects that the pack copies as deltas from another pack, nor
/// those whose `delta` attribute is unset or that are larger than `core.bigFileThreshold`.
///
/// * `find` reads the objects, a clone of it in each thread.
/// * `progress` counts the objects that were tried as deltas, those that aren't preferred bases.
/// * `should_interrupt` is checked before each object, and once it is set, the search stops with an error.
///
/// It is an error if an object that the search reads doesn't exist, unless it is a preferred base, which then isn't used,
/// or if its data doesn't have its [`size`](Object::size).
pub fn find_deltas<F>(
    objects: &[Object],
    find: F,
    progress: &dyn Count,
    should_interrupt: &AtomicBool,
    options: Options,
) -> Result<Vec<Option<Delta>>>
where
    F: gix_object::Find + Clone + Send,
{
    let mut deltas: Vec<Option<Delta>> = (0..objects.len()).map(|_| None).collect();
    if options.window == 0 || options.depth == 0 {
        return Ok(deltas);
    }
    let mut list: Vec<usize> = (0..objects.len())
        .filter(|&index| objects[index].size >= MIN_SIZE)
        .collect();
    if list.len() < 2 || list.iter().all(|&index| objects[index].preferred_base) {
        return Ok(deltas);
    }
    list.sort_unstable_by(|&a, &b| compare(&objects[a], &objects[b]).then(a.cmp(&b)));
    let shared = Shared {
        objects,
        list: &list,
        window: options.window.saturating_add(1).min(list.len() + 1),
        options,
        cached: Mutex::new(0),
        progress: progress.counter(),
        should_interrupt,
        stop: AtomicBool::new(false),
    };
    let threads = gix_parallel::num_threads(options.thread_limit);
    let found = if threads > 1 {
        shared.in_threads(find, threads)?
    } else {
        let mut searcher = Searcher::new(find, &shared);
        let mut next = list.iter().copied();
        searcher.find_deltas(|| next.next())?;
        searcher.found
    };
    for (index, delta) in found {
        deltas[index] = Some(delta);
    }
    Ok(deltas)
}

/// The order of the search, that of Git's `type_size_sort()`: by kind, tags first and commits last, then by name hash,
/// preferred bases first, and by size, the larger values first.
fn compare(a: &Object, b: &Object) -> Ordering {
    kind_number(b.kind)
        .cmp(&kind_number(a.kind))
        .then(b.name_hash.cmp(&a.name_hash))
        .then(b.preferred_base.cmp(&a.preferred_base))
        .then(b.size.cmp(&a.size))
}

/// The number of `kind` in packs, which orders the kinds in the search.
fn kind_number(kind: gix_object::Kind) -> u8 {
    match kind {
        gix_object::Kind::Commit => 1,
        gix_object::Kind::Tree => 2,
        gix_object::Kind::Blob => 3,
        gix_object::Kind::Tag => 4,
    }
}

/// What the threads of a search share.
struct Shared<'a> {
    objects: &'a [Object],
    /// The indices of the objects that are searched, in the order of the search.
    list: &'a [usize],
    /// The amount of places in a window: one more than the objects that are tried as bases of each object, as in Git,
    /// but at most one more than the objects that are searched, as further places would stay empty.
    window: usize,
    options: Options,
    /// The amount of bytes of the deltas that are kept.
    cached: Mutex<u64>,
    /// The amount of objects that were tried as deltas.
    progress: StepShared,
    should_interrupt: &'a AtomicBool,
    /// Set once a thread fails, so that the others stop.
    stop: AtomicBool,
}

impl Shared<'_> {
    /// Whether a delta of `size` bytes of an object of `trg_size` bytes against a base of `src_size` bytes is kept, after
    /// the kept delta of `replaced` bytes that the object had, if any, was dropped, as Git's `delta_cacheable()` decides.
    fn keep(&self, replaced: Option<usize>, src_size: u64, trg_size: u64, size: usize) -> bool {
        let Options {
            delta_cache_size,
            delta_cache_limit,
            ..
        } = self.options;
        let size = size as u64;
        let mut cached = lock(&self.cached);
        if let Some(replaced) = replaced {
            *cached -= replaced as u64;
        }
        let keep = (delta_cache_size == 0 || cached.saturating_add(size) <= delta_cache_size)
            && (size < delta_cache_limit || (src_size >> 20) + (trg_size >> 21) > (size >> 10));
        if keep {
            *cached += size;
        }
        keep
    }

    /// Whether the object at `position` of the search order has the same name hash as the one before it, other than 0, so
    /// that the part of a thread neither ends nor starts before it.
    fn continues(&self, position: usize) -> bool {
        let name_hash = |position: usize| self.objects[self.list[position]].name_hash;
        name_hash(position) != 0 && name_hash(position) == name_hash(position - 1)
    }

    /// Divide the search order among `threads` threads as Git's `ll_find_deltas()` does: each thread gets an equal share of
    /// the objects that the threads before it left, extended over the following objects with the same name hash as its
    /// last one, other than 0, but a thread other than the last gets none if its share has fewer objects than twice the
    /// places of the window.
    fn divide(&self, threads: usize) -> Vec<Part> {
        let window = self.window;
        let mut parts = Vec::with_capacity(threads);
        let (mut start, mut left) = (0, self.list.len());
        for thread in 0..threads {
            let mut size = left / (threads - thread);
            // Small parts find few deltas, so their objects are left to the following threads.
            if size < 2 * window && thread + 1 < threads {
                size = 0;
            }
            while size > 0 && size < left && self.continues(start + size) {
                size += 1;
            }
            parts.push(Part {
                start,
                size,
                remaining: size,
                working: true,
                ready: false,
            });
            start += size;
            left -= size;
        }
        parts
    }

    /// Give the idle thread `target` the second half of the objects that are left in the part with the most of them, as
    /// Git's `ll_find_deltas()` does, and return their amount, which is 0 if no part has more objects left than twice the
    /// places of the window. The half starts at its first object whose name hash is 0 or differs from that of the object
    /// before, or at its first object if there is no such object.
    fn steal(&self, parts: &mut [Part], target: usize) -> usize {
        let window = self.window;
        let mut victim: Option<usize> = None;
        for (thread, part) in parts.iter().enumerate() {
            if part.remaining > 2 * window && victim.is_none_or(|victim| parts[victim].remaining < part.remaining) {
                victim = Some(thread);
            }
        }
        let Some(victim) = victim else {
            return 0;
        };
        let Part {
            start,
            size: victim_size,
            remaining,
            ..
        } = parts[victim];
        let end = start + victim_size;
        let mut size = remaining / 2;
        let mut first = end - size;
        while size > 0 && self.continues(first) {
            first += 1;
            size -= 1;
        }
        if size == 0 {
            // No other name hash starts in the second half, so it is taken whole.
            size = remaining / 2;
            first = end - size;
        }
        parts[target].start = first;
        parts[victim].size -= size;
        parts[victim].remaining -= size;
        size
    }

    /// Find the deltas with `threads` threads, which divide the search order and take work from each other as those of
    /// Git's `ll_find_deltas()` do, and return them with the index of their object.
    fn in_threads<F>(&self, find: F, threads: usize) -> Result<Vec<(usize, Delta)>>
    where
        F: gix_object::Find + Clone + Send,
    {
        let state = Mutex::new(self.divide(threads));
        let idle = Condvar::new();
        let ready: Vec<Condvar> = (0..threads).map(|_| Condvar::new()).collect();
        let (mut found, mut failure, mut panic) = (Vec::new(), None, None);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(threads);
            let mut active = 0;
            for (thread, thread_ready) in ready.iter().enumerate() {
                if lock(&state)[thread].size == 0 {
                    handles.push(None);
                    continue;
                }
                let (thread_find, thread_state) = (find.clone(), &state);
                let idle = &idle;
                let spawned = std::thread::Builder::new()
                    .name("gitoxide.find_deltas".into())
                    .spawn_scoped(scope, move || {
                        self.work(thread, thread_find, thread_state, idle, thread_ready)
                    })
                    .or_raise(|| message("Could not start a thread to look for deltas"));
                match spawned {
                    Ok(handle) => {
                        handles.push(Some(handle));
                        active += 1;
                    }
                    Err(err) => {
                        self.stop.store(true, Relaxed);
                        failure.get_or_insert(err);
                        handles.push(None);
                    }
                }
            }

            while active > 0 {
                let mut parts = lock(&state);
                let target = loop {
                    if let Some(target) = parts.iter().position(|part| !part.working) {
                        break target;
                    }
                    parts = idle.wait(parts).unwrap_or_else(PoisonError::into_inner);
                };
                let size = if self.stop.load(Relaxed) {
                    0
                } else {
                    self.steal(&mut parts, target)
                };
                let part = &mut parts[target];
                part.size = size;
                part.remaining = size;
                part.working = true;
                part.ready = true;
                drop(parts);
                ready[target].notify_one();

                if size == 0 {
                    let handle = handles[target].take().expect("only started threads become idle");
                    match handle.join() {
                        Ok(Ok(deltas)) => found.extend(deltas),
                        Ok(Err(err)) => {
                            failure.get_or_insert(err);
                        }
                        Err(payload) => {
                            panic.get_or_insert(payload);
                        }
                    }
                    active -= 1;
                }
            }
        });
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
        match failure {
            Some(err) => Err(err),
            None => Ok(found),
        }
    }

    /// Find deltas in the parts of the search order that `thread` is given until it is given an empty one, as Git's
    /// `threaded_find_deltas()` does, and return them with the index of their object.
    fn work<F: gix_object::Find>(
        &self,
        thread: usize,
        find: F,
        state: &Mutex<Vec<Part>>,
        idle: &Condvar,
        ready: &Condvar,
    ) -> Result<Vec<(usize, Delta)>> {
        let _idle_on_panic = IdleOnPanic {
            stop: &self.stop,
            state,
            idle,
            thread,
        };
        let mut searcher = Searcher::new(find, self);
        let mut result = Ok(());
        let mut parts = lock(state);
        while parts[thread].remaining > 0 {
            let start = parts[thread].start;
            drop(parts);
            if result.is_ok() {
                let mut taken = 0;
                result = searcher.find_deltas(|| {
                    let mut parts = lock(state);
                    let part = &mut parts[thread];
                    (part.remaining > 0).then(|| {
                        part.remaining -= 1;
                        taken += 1;
                        self.list[start + taken - 1]
                    })
                });
                if result.is_err() {
                    self.stop.store(true, Relaxed);
                }
            }
            parts = lock(state);
            parts[thread].working = false;
            idle.notify_one();
            while !parts[thread].ready {
                parts = ready.wait(parts).unwrap_or_else(PoisonError::into_inner);
            }
            parts[thread].ready = false;
        }
        drop(parts);
        result.map(|()| searcher.found)
    }
}

/// The part of the search order that a thread searches, like Git's `struct thread_params`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Part {
    /// The position of the first object of the part in the search order.
    start: usize,
    /// The amount of objects in the part.
    size: usize,
    /// The amount of objects at the end of the part that the thread hasn't taken yet.
    remaining: usize,
    /// Whether the thread is searching, or was given a part to search, or none.
    working: bool,
    /// Whether the thread was given another part, which may be empty, and hasn't seen it yet.
    ready: bool,
}

/// Marks the thread of a search idle when it panics, so that the thread that divides the work doesn't wait for it.
struct IdleOnPanic<'a> {
    stop: &'a AtomicBool,
    state: &'a Mutex<Vec<Part>>,
    idle: &'a Condvar,
    thread: usize,
}

impl Drop for IdleOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.stop.store(true, Relaxed);
            lock(self.state)[self.thread].working = false;
            self.idle.notify_one();
        }
    }
}

/// A thread of the search, with a window of its own.
struct Searcher<'a, F> {
    find: F,
    shared: &'a Shared<'a>,
    /// The places of the window, like Git's array of `struct unpacked`.
    slots: Vec<Slot>,
    /// The delta that is computed.
    delta: Vec<u8>,
    /// The deltas that were found, with the index of their object.
    found: Vec<(usize, Delta)>,
}

/// A place in the window, like Git's `struct unpacked`.
#[derive(Default)]
struct Slot {
    /// The index of the object in the objects that are searched, or `None` if the place is empty.
    object: Option<usize>,
    /// The data of the object, once it was read.
    data: Option<Vec<u8>>,
    /// The index of the data of the object, once the object was tried as a base.
    index: Option<Index>,
    /// The depth of the delta of the object, or 0 if it is stored whole.
    depth: u32,
}

impl<'a, F: gix_object::Find> Searcher<'a, F> {
    fn new(find: F, shared: &'a Shared<'a>) -> Self {
        Searcher {
            find,
            shared,
            slots: Vec::new(),
            delta: Vec::new(),
            found: Vec::new(),
        }
    }

    /// Find the deltas of the objects that `next` returns, starting with an empty window, as Git's `find_deltas()` does.
    fn find_deltas(&mut self, mut next: impl FnMut() -> Option<usize>) -> Result<()> {
        let shared = self.shared;
        let window = shared.window;
        self.slots.clear();
        self.slots.resize_with(window, Slot::default);
        // The place of the object that is tried as a delta, the amount of objects before it in the window, and the bytes
        // that the data and indices of the objects in the window take.
        let (mut idx, mut count, mut memory) = (0, 0, 0);
        loop {
            if shared.should_interrupt.load(Relaxed) {
                bail!(cancelled("Interrupted while looking for deltas"));
            }
            if shared.stop.load(Relaxed) {
                return Ok(());
            }
            let Some(entry) = next() else {
                return Ok(());
            };
            if !shared.objects[entry].preferred_base {
                shared.progress.fetch_add(1, Relaxed);
            }
            memory -= self.free(idx);
            self.slots[idx].object = Some(entry);
            while shared.options.window_memory != 0 && memory > shared.options.window_memory && count > 1 {
                memory -= self.free((idx + window - count) % window);
                count -= 1;
            }
            if self.try_bases(entry, idx, &mut memory)? {
                idx = (idx + 1) % window;
                if count + 1 < window {
                    count += 1;
                }
            }
        }
    }

    /// Try the objects before `entry`, which is at place `idx` of the window, as its bases, keep the last delta that is
    /// accepted, and return whether the object stays in the window, which it doesn't if its delta has the greatest depth.
    fn try_bases(&mut self, entry: usize, idx: usize, memory: &mut u64) -> Result<bool> {
        let shared = self.shared;
        let window = shared.window;
        let object = &shared.objects[entry];
        // Preferred bases are only bases, and copied deltas that depend on an object limit the depth of its delta.
        let max_depth = i64::from(shared.options.depth) - i64::from(object.dependents_depth);
        if object.preferred_base || max_depth <= 0 {
            return Ok(true);
        }
        let max_depth = max_depth as u32;
        let (mut best, mut best_base) = (None, None);
        for j in (1..window).rev() {
            let other = (idx + j) % window;
            if self.slots[other].object.is_none() {
                break;
            }
            match self.try_delta(idx, other, max_depth, memory, &mut best)? {
                Ordering::Less => break,
                Ordering::Equal => {}
                Ordering::Greater => best_base = Some(other),
            }
        }
        let (Some(delta), Some(best_base)) = (best, best_base) else {
            return Ok(true);
        };
        self.found.push((entry, delta));
        if max_depth <= self.slots[idx].depth {
            return Ok(false);
        }
        // The base moves to the place after the object, to be tried first as a base of the next object.
        let mut slot = best_base;
        for _ in 0..(window + idx - best_base) % window {
            let next = (slot + 1) % window;
            self.slots.swap(slot, next);
            slot = next;
        }
        Ok(true)
    }

    /// Try the object at place `src` of the window as the base of the object at place `trg`, whose delta may have the depth
    /// `max_depth` at most, as Git's `try_delta()` does. Return `Less` if the objects are of different kinds, so that no
    /// further bases are tried, `Greater` if the delta was accepted as `best`, and `Equal` otherwise.
    fn try_delta(
        &mut self,
        trg: usize,
        src: usize,
        max_depth: u32,
        memory: &mut u64,
        best: &mut Option<Delta>,
    ) -> Result<Ordering> {
        let Searcher {
            find,
            shared,
            slots,
            delta,
            ..
        } = self;
        let [trg, src] = slots
            .get_disjoint_mut([trg, src])
            .expect("an object and its base are at different places");
        let (Some(trg_entry), Some(src_entry)) = (trg.object, src.object) else {
            unreachable!("the object and its base are in the window")
        };
        let (trg_object, src_object) = (&shared.objects[trg_entry], &shared.objects[src_entry]);
        if trg_object.kind != src_object.kind {
            return Ok(Ordering::Less);
        }
        if shared.options.reuse_delta
            && !src_object.preferred_base
            && trg_object.in_pack.is_some_and(|trg_pack| {
                !trg_pack.is_delta
                    && src_object
                        .in_pack
                        .is_some_and(|src_pack| src_pack.pack_id == trg_pack.pack_id)
            })
        {
            return Ok(Ordering::Equal);
        }
        if src.depth >= max_depth {
            return Ok(Ordering::Equal);
        }

        let (trg_size, src_size) = (trg_object.size, src_object.size);
        let (max_size, ref_depth) = match best {
            None => (
                (trg_size / 2).wrapping_sub(trg_object.id.kind().len_in_bytes() as u64),
                1,
            ),
            Some(best) => (best.size as u64, trg.depth),
        };
        let max_size = max_size.wrapping_mul(u64::from(max_depth - src.depth)) / u64::from(max_depth - ref_depth + 1);
        if max_size == 0 || trg_size.saturating_sub(src_size) >= max_size || trg_size < src_size / 32 {
            return Ok(Ordering::Equal);
        }

        let Some(trg_data) = load(&mut trg.data, find, trg_object, memory)? else {
            let id = trg_object.id;
            bail!("The object {id} doesn't exist".not_found());
        };
        let Some(src_data) = load(&mut src.data, find, src_object, memory)? else {
            // The pack doesn't contain its preferred bases, and can be made without them.
            if src_object.preferred_base {
                return Ok(Ordering::Equal);
            }
            let id = src_object.id;
            bail!("The object {id} doesn't exist".not_found());
        };
        let index = match src.index.take() {
            Some(index) => index,
            None => {
                let Some(index) = Index::new(src_data) else {
                    return Ok(Ordering::Equal);
                };
                *memory += index.memory_size() as u64;
                index
            }
        };
        let index = src.index.insert(index);
        if !index.encode(
            src_data,
            trg_data,
            Some(usize::try_from(max_size).unwrap_or(usize::MAX)),
            delta,
        ) {
            return Ok(Ordering::Equal);
        }
        let size = delta.len();
        // Of deltas of the same length, the less deep one is kept.
        if best
            .as_ref()
            .is_some_and(|best| size == best.size && src.depth + 1 >= trg.depth)
        {
            return Ok(Ordering::Equal);
        }

        let replaced = best.as_ref().filter(|best| best.data.is_some()).map(|best| best.size);
        let data = shared.keep(replaced, src_size, trg_size, size).then(|| {
            let mut data = std::mem::take(delta);
            data.shrink_to_fit();
            data
        });
        *best = Some(Delta {
            base: src_entry,
            size,
            data,
        });
        trg.depth = src.depth + 1;
        Ok(Ordering::Greater)
    }

    /// Empty the place `slot` of the window and return the amount of bytes that its data and index took, as Git's
    /// `free_unpacked()` does.
    fn free(&mut self, slot: usize) -> u64 {
        let objects = self.shared.objects;
        let slot = &mut self.slots[slot];
        let mut freed = slot.index.take().map_or(0, |index| index.memory_size() as u64);
        if slot.data.take().is_some() {
            freed += slot.object.map_or(0, |object| objects[object].size);
        }
        slot.object = None;
        slot.depth = 0;
        freed
    }
}

/// Return the data of `object` from `data`, after reading it into `data` and counting its size in `memory` if it wasn't
/// read yet, or return `None` if the object doesn't exist.
fn load<'d, F: gix_object::Find>(
    data: &'d mut Option<Vec<u8>>,
    find: &F,
    object: &Object,
    memory: &mut u64,
) -> Result<Option<&'d [u8]>> {
    if data.is_none() {
        let Some(read) = read(find, object)? else {
            return Ok(None);
        };
        *memory += object.size;
        *data = Some(read);
    }
    Ok(data.as_deref())
}

/// Read the data of `object` into a buffer of its own and check its size, or return `None` if it doesn't exist.
fn read<F: gix_object::Find>(find: &F, object: &Object) -> Result<Option<Vec<u8>>> {
    let id = object.id;
    let mut buffer = Vec::new();
    let Some(found) = find
        .try_find(&id, &mut buffer)
        .or_raise(|| message!("Could not read the object {id}"))?
    else {
        return Ok(None);
    };
    let (address, len) = (found.data.as_ptr().addr(), found.data.len());
    let offset = address.wrapping_sub(buffer.as_ptr().addr());
    let data = if offset <= buffer.len() && len <= buffer.len() - offset {
        // The data is in the buffer, which becomes the buffer of the object without a copy.
        buffer.truncate(offset + len);
        buffer.drain(..offset);
        buffer
    } else {
        // The data is in memory that outlives the buffer, and is read again to be copied.
        let mut scratch = Vec::new();
        match find
            .try_find(&id, &mut scratch)
            .or_raise(|| message!("Could not read the object {id}"))?
        {
            Some(found) => found.data.to_vec(),
            None => return Ok(None),
        }
    };
    let (actual, expected) = (data.len() as u64, object.size);
    ensure!(
        actual == expected,
        "The object {id} has {actual} bytes rather than the {expected} bytes that it was listed with".corrupted()
    );
    Ok(Some(data))
}

/// Lock `mutex`, also if a thread panicked while it held it, as the threads only change the state under the lock in steps
/// that leave it consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicBool};

    use super::{Object, Options, Part, Shared, name_hash};

    #[test]
    fn name_hashes_are_made_of_the_last_bytes_other_than_whitespace() {
        assert_eq!(name_hash(b""), 0);
        assert_eq!(
            name_hash(b"ab"),
            0x7a40_0000,
            "each byte enters the top 8 bits after the hash moved right by 2"
        );
        assert_eq!(name_hash(b"a.c"), 0x7490_0000);
        assert_eq!(
            name_hash(b" a.\tc\r\n"),
            name_hash(b"a.c"),
            "spaces, tabs, line feeds and carriage returns are left out"
        );
        assert_ne!(
            name_hash(b"a.c\x0b"),
            name_hash(b"a.c"),
            "vertical tabs count, as in Git"
        );
        assert_ne!(name_hash(b"a.c\x0c"), name_hash(b"a.c"), "form feeds count, as in Git");
        assert_eq!(
            name_hash(b"\xff\xff"),
            0x3ec0_0000,
            "bytes above 0x7f count with their 8 bits"
        );
    }

    /// Blobs of 100 bytes whose name hashes are `name_hashes`.
    fn objects(name_hashes: &[u32]) -> Vec<Object> {
        name_hashes
            .iter()
            .map(|&name_hash| Object {
                id: gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
                kind: gix_object::Kind::Blob,
                size: 100,
                name_hash,
                preferred_base: false,
                in_pack: None,
                dependents_depth: 0,
            })
            .collect()
    }

    /// Call `test` with a search of `objects` in their order whose window has `window` places.
    fn with_search(objects: &[Object], window: usize, test: impl FnOnce(&Shared<'_>)) {
        let list: Vec<usize> = (0..objects.len()).collect();
        let should_interrupt = AtomicBool::new(false);
        let search = Shared {
            objects,
            list: &list,
            window,
            options: Options::default(),
            cached: Mutex::new(0),
            progress: Default::default(),
            should_interrupt: &should_interrupt,
            stop: AtomicBool::new(false),
        };
        test(&search);
    }

    /// Parts at the positions and with the sizes of `spans`, which have the amount of objects of `spans` left.
    fn parts(spans: &[(usize, usize, usize)]) -> Vec<Part> {
        spans
            .iter()
            .map(|&(start, size, remaining)| Part {
                start,
                size,
                remaining,
                working: remaining > 0,
                ready: false,
            })
            .collect()
    }

    /// The position of the first object and the amount of objects of each part.
    fn spans(parts: &[Part]) -> Vec<(usize, usize)> {
        parts.iter().map(|part| (part.start, part.size)).collect()
    }

    #[test]
    fn threads_get_equal_parts_that_end_where_a_name_hash_ends() {
        let mut name_hashes: Vec<u32> = (1..=100).collect();
        with_search(&objects(&name_hashes), 11, |search| {
            assert_eq!(spans(&search.divide(4)), [(0, 25), (25, 25), (50, 25), (75, 25)]);
        });
        name_hashes[23..30].fill(1000);
        with_search(&objects(&name_hashes), 11, |search| {
            assert_eq!(
                spans(&search.divide(4)),
                [(0, 30), (30, 23), (53, 23), (76, 24)],
                "a part extends over the objects with the name hash of its last object, and the others share the rest"
            );
        });
        name_hashes[23..30].fill(0);
        with_search(&objects(&name_hashes), 11, |search| {
            assert_eq!(
                spans(&search.divide(4)),
                [(0, 25), (25, 25), (50, 25), (75, 25)],
                "objects without a name hash don't extend parts"
            );
        });
    }

    #[test]
    fn threads_other_than_the_last_get_no_share_smaller_than_twice_the_window() {
        let name_hashes: Vec<u32> = (1..=50).collect();
        with_search(&objects(&name_hashes), 11, |search| {
            assert_eq!(spans(&search.divide(4)), [(0, 0), (0, 0), (0, 25), (25, 25)]);
        });
        with_search(&objects(&name_hashes[..30]), 11, |search| {
            assert_eq!(spans(&search.divide(4)), [(0, 0), (0, 0), (0, 0), (0, 30)]);
        });
    }

    #[test]
    fn an_idle_thread_takes_the_second_half_of_what_the_largest_part_has_left() {
        let name_hashes: Vec<u32> = (1..=100).collect();
        with_search(&objects(&name_hashes), 4, |search| {
            let mut parts = parts(&[(0, 40, 9), (40, 30, 20), (70, 30, 20), (0, 0, 0)]);
            assert_eq!(
                search.steal(&mut parts, 3),
                10,
                "the first of the parts with the most objects left gives the second half of them"
            );
            assert_eq!(parts[3].start, 60);
            assert_eq!(spans(&parts[..3]), [(0, 40), (40, 20), (70, 30)]);
            assert_eq!(parts[1].remaining, 10);
        });
    }

    #[test]
    fn the_taken_half_starts_where_a_name_hash_starts() {
        let mut name_hashes: Vec<u32> = (1..=100).collect();
        name_hashes[58..64].fill(1000);
        with_search(&objects(&name_hashes), 4, |search| {
            let mut parts = parts(&[(40, 30, 20), (0, 0, 0)]);
            assert_eq!(search.steal(&mut parts, 1), 6);
            assert_eq!(parts[1].start, 64);
            assert_eq!((parts[0].size, parts[0].remaining), (24, 14));
        });
        name_hashes[55..70].fill(1000);
        with_search(&objects(&name_hashes), 4, |search| {
            let mut parts = parts(&[(40, 30, 20), (0, 0, 0)]);
            assert_eq!(
                search.steal(&mut parts, 1),
                10,
                "a half in which no name hash starts is taken whole"
            );
            assert_eq!(parts[1].start, 60);
            assert_eq!((parts[0].size, parts[0].remaining), (20, 10));
        });
    }

    #[test]
    fn an_idle_thread_takes_nothing_if_no_part_has_more_than_twice_the_window_left() {
        let name_hashes: Vec<u32> = (1..=100).collect();
        with_search(&objects(&name_hashes), 4, |search| {
            let mut parts = parts(&[(0, 50, 8), (50, 50, 8), (0, 0, 0)]);
            let before = parts.clone();
            assert_eq!(search.steal(&mut parts, 2), 0);
            assert_eq!(parts, before);
        });
    }
}
