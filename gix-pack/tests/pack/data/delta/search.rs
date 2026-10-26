//! The search is compared with the bases that `git pack-objects` chose for the objects of `make_deltas.sh` with a single
//! thread, and each of its rules is checked with objects made for it. The deltas that several threads find are checked by
//! building their objects from them.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gix_hash::ObjectId;
use gix_object::Kind;
use gix_pack::data::{
    delta::{
        encode,
        search::{Delta, InPack, Object, Options, find_deltas, name_hash},
    },
    entry::Header,
};
use gix_testtools::TestResult;
use gix_utils::progress::{Count, Step, StepShared};

use crate::data::delta::{Packed, Rng, assert_pack_holds};

/// The kind and the data of objects by their id.
type Data = HashMap<ObjectId, (Kind, Vec<u8>)>;

/// Objects in memory, which the search reads.
#[derive(Clone)]
struct Store(Arc<Data>);

impl gix_object::Find for Store {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix_error::Result<Option<gix_object::Data<'a>>> {
        let Some((kind, data)) = self.0.get(id) else {
            return Ok(None);
        };
        buffer.clear();
        buffer.extend_from_slice(data);
        Ok(Some(gix_object::Data {
            kind: *kind,
            object_hash: id.kind(),
            data: buffer,
        }))
    }
}

/// Counts the objects that the search tries as deltas.
#[derive(Default)]
struct Counter(StepShared);

impl Count for Counter {
    fn set(&self, step: Step) {
        self.0.store(step, Ordering::Relaxed);
    }

    fn step(&self) -> Step {
        self.0.load(Ordering::Relaxed)
    }

    fn inc_by(&self, step: Step) {
        self.0.fetch_add(step, Ordering::Relaxed);
    }

    fn counter(&self) -> StepShared {
        Arc::clone(&self.0)
    }
}

/// Search `objects` with `options`, reading them from `store`, and return the deltas and the amount of objects that were
/// tried as deltas.
fn search(objects: &[Object], store: &Store, options: Options) -> gix_error::Result<(Vec<Option<Delta>>, usize)> {
    let counter = Counter::default();
    let deltas = find_deltas(objects, store.clone(), &counter, &AtomicBool::new(false), options)?;
    Ok((deltas, counter.step()))
}

/// The options of `git pack-objects` with a single thread.
fn one_thread() -> Options {
    Options {
        thread_limit: Some(1),
        ..Options::default()
    }
}

/// The index of the base of the object at `index`, if it is stored as a delta.
fn base_of(deltas: &[Option<Delta>], index: usize) -> Option<usize> {
    deltas[index].as_ref().map(|delta| delta.base)
}

/// `data` followed by `end`.
fn appended(data: &[u8], end: &[u8]) -> Vec<u8> {
    [data, end].concat()
}

/// Objects made for a test, in the order in which they are listed for the search.
#[derive(Default)]
struct Objects {
    list: Vec<Object>,
    data: Data,
}

impl Objects {
    /// Add a blob with `data` that was found at a path whose name hash is `name_hash`, and return its index.
    fn blob(&mut self, name_hash: u32, data: &[u8]) -> usize {
        self.add(Kind::Blob, name_hash, data)
    }

    /// Add an object of `kind` with `data` that was found at a path whose name hash is `name_hash`, and return its index.
    fn add(&mut self, kind: Kind, name_hash: u32, data: &[u8]) -> usize {
        let id = gix_object::compute_hash(gix_testtools::object_hash(), kind, data).expect("data in memory is hashed");
        self.data.insert(id, (kind, data.to_vec()));
        self.list.push(Object {
            id,
            kind,
            size: data.len() as u64,
            name_hash,
            preferred_base: false,
            in_pack: None,
            dependents_depth: 0,
        });
        self.list.len() - 1
    }

    /// Search the objects with `options`, and return the deltas and the amount of objects that were tried as deltas.
    fn search(&self, options: Options) -> gix_error::Result<(Vec<Option<Delta>>, usize)> {
        search(&self.list, &Store(Arc::new(self.data.clone())), options)
    }
}

/// A pack that `git pack-objects` wrote for `make_deltas.sh`: the kind and the data of each of its objects, and the base
/// and the delta of each object that it stores as a delta.
struct GitPack {
    data: Data,
    deltas: HashMap<ObjectId, (ObjectId, Vec<u8>)>,
}

impl GitPack {
    /// Read the one pack in `dir`.
    fn read(dir: &Path) -> TestResult<Self> {
        let object_hash = gix_testtools::object_hash();
        let mut packs = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "pack") {
                packs.push(path);
            }
        }
        let [pack_path] = packs.as_slice() else {
            return Err(format!("git pack-objects wrote one pack, but {} holds {packs:?}", dir.display()).into());
        };
        let pack = gix_pack::data::File::at(pack_path, object_hash)?;
        let mut inflate = gix_zlib::Inflate::default();
        let (mut data, mut deltas, mut ids) = (Data::new(), HashMap::new(), HashMap::new());
        for entry in pack.streaming_iter()? {
            let entry = entry?;
            let mut object = Vec::new();
            let outcome = pack.decode_entry(
                pack.entry(entry.pack_offset)?,
                &mut object,
                &mut inflate,
                &|_, _| Ok(None),
                &mut gix_pack::cache::Never,
            )?;
            let id = gix_object::compute_hash(object_hash, outcome.kind, &object)?;
            match entry.header {
                Header::OfsDelta { base_distance } => {
                    let delta_entry = pack.entry(entry.pack_offset)?;
                    let mut delta = vec![0; usize::try_from(delta_entry.decompressed_size)?];
                    pack.decompress_entry(&delta_entry, &mut inflate, &mut delta)?;
                    let base = ids
                        .get(&delta_entry.base_pack_offset(base_distance))
                        .copied()
                        .ok_or("git writes bases before their deltas")?;
                    deltas.insert(id, (base, delta));
                }
                Header::RefDelta { .. } => return Err("git was asked to refer to bases by their offset".into()),
                _ => {}
            }
            ids.insert(entry.pack_offset, id);
            data.insert(id, (outcome.kind, object));
        }
        Ok(GitPack { data, deltas })
    }
}

/// The objects of `objects.list` in `dir`, in the order in which `git pack-objects` read them, with the name hashes of
/// their paths and the kinds and the sizes that they have in `pack`.
fn objects_in_order(dir: &Path, pack: &GitPack) -> TestResult<Vec<Object>> {
    let hex_len = gix_testtools::object_hash().len_in_hex();
    let list = std::fs::read(dir.join("objects.list"))?;
    let mut objects = Vec::new();
    for line in list.split(|&byte| byte == b'\n').filter(|line| !line.is_empty()) {
        let id = ObjectId::from_hex(&line[..hex_len])?;
        let (kind, data) = pack.data.get(&id).ok_or("git packed every listed object")?;
        objects.push(Object {
            id,
            kind: *kind,
            size: data.len() as u64,
            // Git hashes what follows the id and a space, which the lines of commits don't have.
            name_hash: name_hash(line.get(hex_len + 1..).unwrap_or_default()),
            preferred_base: false,
            in_pack: None,
            dependents_depth: 0,
        });
    }
    Ok(objects)
}

/// Check that each of `deltas` builds its object of `objects` from a base of the same kind, in chains of at most `depth`
/// deltas, by writing the objects into a pack in which each delta follows its base and having gix-pack build every
/// object from it.
fn assert_deltas_build_their_objects(
    objects: &[Object],
    data: &Data,
    deltas: &[Option<Delta>],
    depth: u32,
) -> TestResult {
    let mut depths = Vec::with_capacity(objects.len());
    for (start, object) in objects.iter().enumerate() {
        let (mut index, mut chain) = (start, 0);
        while let Some(delta) = &deltas[index] {
            chain += 1;
            assert!(
                chain <= depth,
                "the chain of deltas of {} has more than {depth} deltas",
                object.id
            );
            index = delta.base;
        }
        depths.push(chain);
    }
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by_key(|&index| depths[index]);
    let mut position = vec![0; objects.len()];
    for (at, &index) in order.iter().enumerate() {
        position[index] = at;
    }

    let mut packed = Vec::with_capacity(objects.len());
    for &index in &order {
        let object = &objects[index];
        let (kind, object_data) = &data[&object.id];
        packed.push(match &deltas[index] {
            None => Packed::whole(*kind, object_data),
            Some(delta) => {
                let base = &objects[delta.base];
                assert_eq!(base.kind, object.kind, "the base of {} has its kind", object.id);
                let bytes = match &delta.data {
                    Some(bytes) => bytes.clone(),
                    None => encode(&data[&base.id].1, object_data).expect("neither is empty"),
                };
                assert_eq!(bytes.len(), delta.size, "the delta of {} has its length", object.id);
                Packed::delta(*kind, object_data, position[delta.base], bytes)
            }
        });
    }
    assert_pack_holds(&packed)
}

/// With a single thread, the search chooses the bases that `git pack-objects --threads=1` chose for the objects of
/// `make_deltas.sh`: with windows of 250 and 10 objects and a depth of 50, and with a window of 50 objects within 64 KiB
/// and a depth of 3. Its deltas have the bytes of Git's deltas.
#[test]
fn bases_are_those_of_git_pack_objects_with_one_thread() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    let mut git_bases = Vec::new();
    for (name, window, depth, window_memory) in [
        ("packed", 250, 50, 0),
        ("default", 10, 50, 0),
        ("shallow", 50, 3, 64 * 1024),
    ] {
        let pack = GitPack::read(&dir.join(name))?;
        let objects = objects_in_order(&dir, &pack)?;
        let options = Options {
            window,
            depth,
            window_memory,
            ..one_thread()
        };
        let (deltas, tried) = search(&objects, &Store(Arc::new(pack.data.clone())), options)?;
        assert_eq!(
            tried,
            objects.iter().filter(|object| object.size >= 50).count(),
            "{name}: each object of 50 bytes or more is tried as a delta"
        );
        let mut matched = 0;
        for (object, delta) in objects.iter().zip(&deltas) {
            match (delta, pack.deltas.get(&object.id)) {
                (None, None) => {}
                (Some(delta), Some((git_base, git_delta))) => {
                    let base = &objects[delta.base];
                    assert_eq!(base.id, *git_base, "{name}: the base of {}", object.id);
                    let ours = match &delta.data {
                        Some(data) => data.clone(),
                        None => encode(&pack.data[&base.id].1, &pack.data[&object.id].1).expect("neither is empty"),
                    };
                    assert_eq!(
                        delta.size,
                        ours.len(),
                        "{name}: the length of the delta of {}",
                        object.id
                    );
                    assert!(ours == *git_delta, "{name}: the delta of {} has Git's bytes", object.id);
                    matched += 1;
                }
                (ours, git) => panic!(
                    "{name}: {} is stored as a delta here: {}, and by Git: {}",
                    object.id,
                    ours.is_some(),
                    git.is_some()
                ),
            }
        }
        assert_eq!(
            matched,
            pack.deltas.len(),
            "{name}: Git's deltas are deltas of listed objects"
        );
        git_bases.push((
            pack.deltas
                .iter()
                .map(|(id, (base, _))| (*id, *base))
                .collect::<HashMap<_, _>>(),
            pack,
        ));
    }

    // The fixture tests what the options change: each pack has other bases, and objects of every kind are deltas.
    let (packed, packed_pack) = &git_bases[0];
    assert_ne!(*packed, git_bases[1].0, "a window of 10 objects changes Git's bases");
    assert_ne!(
        *packed, git_bases[2].0,
        "a depth of 3 and a window memory change Git's bases"
    );
    for kind in [Kind::Blob, Kind::Tree, Kind::Commit, Kind::Tag] {
        assert!(
            packed.keys().any(|id| packed_pack.data[id].0 == kind),
            "Git stores objects of the kind {kind} as deltas"
        );
    }
    Ok(())
}

/// Without limits, the cache keeps every delta, a cache of one byte keeps none, and by default the deltas of objects
/// smaller than 1 MiB are kept if they are shorter than 1000 bytes. The bases don't depend on the deltas that are kept.
#[test]
fn the_cache_keeps_short_deltas_within_its_size() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    let pack = GitPack::read(&dir.join("packed"))?;
    let objects = objects_in_order(&dir, &pack)?;
    let store = Store(Arc::new(pack.data.clone()));
    let options = Options {
        window: 250,
        ..one_thread()
    };
    let unlimited = Options {
        delta_cache_size: 0,
        delta_cache_limit: u64::MAX,
        ..options
    };
    let (all, _) = search(&objects, &store, unlimited)?;
    let (none, _) = search(
        &objects,
        &store,
        Options {
            delta_cache_size: 1,
            ..unlimited
        },
    )?;
    let (short, _) = search(&objects, &store, options)?;

    let bases = |deltas: &[Option<Delta>]| -> Vec<Option<(usize, usize)>> {
        deltas
            .iter()
            .map(|delta| delta.as_ref().map(|delta| (delta.base, delta.size)))
            .collect()
    };
    assert_eq!(bases(&none), bases(&all));
    assert_eq!(bases(&short), bases(&all));
    for ((all, none), short) in all
        .iter()
        .flatten()
        .zip(none.iter().flatten())
        .zip(short.iter().flatten())
    {
        assert_eq!(
            all.data.as_ref().map(Vec::len),
            Some(all.size),
            "without limits, every delta is kept"
        );
        assert_eq!(none.data, None, "a cache of one byte keeps no delta");
        assert_eq!(
            short.data.is_some(),
            short.size < 1000,
            "by default, the deltas of these objects are kept if they are shorter than 1000 bytes"
        );
    }
    Ok(())
}

/// Deltas that aren't shorter than `delta_cache_limit` bytes are kept if their length in KiB is less than the size of
/// their base in MiB plus half the size of their object in MiB.
#[test]
fn longer_deltas_are_kept_if_they_are_short_compared_to_their_objects() -> TestResult {
    let mut rng = Rng(0x0de1_7a5e);
    let mut objects = Objects::default();
    let mut targets = Vec::new();
    // Each target is a base of 2 MiB followed by random bytes, so that its delta is a little longer than these bytes:
    // 2 KiB and 3 KiB, of which only the first is less than 2 for the base of 2 MiB plus 1 for the object of 2 MiB.
    for (name_hash, added) in [(4, 2500), (2, 3500)] {
        let base = rng.bytes(2 << 20);
        let added = rng.bytes(added);
        objects.blob(name_hash, &base);
        targets.push(objects.blob(name_hash - 1, &appended(&base, &added)));
    }
    let (deltas, _) = objects.search(Options {
        delta_cache_limit: 0,
        ..one_thread()
    })?;
    let [kept, dropped] = [targets[0], targets[1]].map(|target| {
        deltas[target]
            .clone()
            .expect("each target is a delta of the base before it")
    });
    assert!(
        kept.size >> 10 == 2 && kept.data.is_some(),
        "a delta of 2 KiB is kept: {} bytes",
        kept.size
    );
    assert!(
        dropped.size >> 10 == 3 && dropped.data.is_none(),
        "a delta of 3 KiB isn't kept: {} bytes",
        dropped.size
    );
    Ok(())
}

/// When an object gets a shorter delta, its earlier delta leaves the cache, so that the shorter one fits where the earlier
/// one did.
#[test]
fn a_replaced_delta_leaves_the_cache() -> TestResult {
    let text = Rng(3).text(80);
    let lines: Vec<&[u8]> = text.split_inclusive(|&byte| byte == b'\n').collect();
    let edited = |changed: &[usize]| -> Vec<u8> {
        lines
            .iter()
            .enumerate()
            .flat_map(|(index, line)| {
                if changed.contains(&index) {
                    format!("line {index} changed\n").into_bytes()
                } else {
                    line.to_vec()
                }
            })
            .collect()
    };
    let near = edited(&[40]);
    let far = edited(&[10, 20, 30, 40, 50, 60]);
    let near_delta = encode(&near, &text).expect("neither is empty");
    let far_size = encode(&far, &text).expect("neither is empty").len();
    assert!(
        near_delta.len() < far_size,
        "the target has a shorter delta against the near base"
    );

    let mut objects = Objects::default();
    let near_base = objects.blob(3, &near);
    let far_base = objects.blob(2, &far);
    let target = objects.blob(1, &text);
    // The far base isn't tried as a delta of the near one, so that the target tries the far base first, and then the near
    // one.
    objects.list[far_base].dependents_depth = 50;
    let (deltas, _) = objects.search(Options {
        delta_cache_size: far_size as u64,
        delta_cache_limit: u64::MAX,
        ..one_thread()
    })?;
    assert_eq!(deltas[far_base], None);
    assert_eq!(
        deltas[target],
        Some(Delta {
            base: near_base,
            size: near_delta.len(),
            data: Some(near_delta),
        })
    );
    Ok(())
}

/// Objects smaller than 50 bytes are left out: they are neither deltas nor bases, and they take no place in the window.
#[test]
fn objects_smaller_than_50_bytes_are_left_out() -> TestResult {
    let text = Rng(5).text(20);
    let mut objects = Objects::default();
    let base = objects.blob(3, &text);
    let small = objects.blob(2, &text[..49]);
    let target = objects.blob(1, &appended(&text, b"added\n"));
    let (deltas, tried) = objects.search(Options {
        window: 1,
        ..one_thread()
    })?;
    assert_eq!(
        base_of(&deltas, target),
        Some(base),
        "the small object took no place in the window"
    );
    assert_eq!(deltas[small], None);
    assert_eq!(tried, 2, "the small object wasn't tried");
    Ok(())
}

/// Preferred bases are bases, but they are neither deltas nor tried as such, a preferred base that can't be read is
/// skipped, and preferred bases alone aren't searched.
#[test]
fn preferred_bases_are_only_bases() -> TestResult {
    let text = Rng(9).text(40);
    let mut objects = Objects::default();
    let preferred = objects.blob(2, &appended(&text, b"preferred\n"));
    objects.list[preferred].preferred_base = true;
    let target = objects.blob(1, &text);
    let (deltas, tried) = objects.search(one_thread())?;
    assert_eq!(base_of(&deltas, target), Some(preferred));
    assert_eq!(deltas[preferred], None);
    assert_eq!(tried, 1, "preferred bases aren't tried as deltas");

    let mut objects = Objects::default();
    let base = objects.blob(3, &appended(&text, b"base\n"));
    let missing = objects.blob(2, &appended(&text, b"preferred\n"));
    objects.list[missing].preferred_base = true;
    let id = objects.list[missing].id;
    objects.data.remove(&id);
    let target = objects.blob(1, &text);
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(
        base_of(&deltas, target),
        Some(base),
        "the preferred base that can't be read was skipped"
    );

    let mut objects = Objects::default();
    for (name_hash, end) in [(2, b"one\n"), (1, b"two\n")] {
        let index = objects.blob(name_hash, &appended(&text, end));
        objects.list[index].preferred_base = true;
    }
    objects.data.clear();
    let (deltas, tried) = objects.search(one_thread())?;
    assert!(
        deltas.iter().all(Option::is_none) && tried == 0,
        "preferred bases alone aren't searched"
    );
    Ok(())
}

/// It is an error if an object that the search reads doesn't exist, unless it is a preferred base, or if its data doesn't
/// have its size.
#[test]
fn objects_that_dont_exist_or_have_another_size_are_errors() {
    let text = Rng(13).text(40);
    let failure = |change: &dyn Fn(&mut Objects, usize, usize)| {
        let mut objects = Objects::default();
        let base = objects.blob(2, &appended(&text, b"base\n"));
        let target = objects.blob(1, &text);
        change(&mut objects, base, target);
        objects.search(one_thread()).expect_err("the search fails")
    };
    let err = failure(&|objects, _, target| {
        let id = objects.list[target].id;
        objects.data.remove(&id);
    });
    assert!(err.is_not_found(), "the target doesn't exist: {err:?}");
    let err = failure(&|objects, base, _| {
        let id = objects.list[base].id;
        objects.data.remove(&id);
    });
    assert!(err.is_not_found(), "the base doesn't exist: {err:?}");
    let err = failure(&|objects, _, target| objects.list[target].size += 1);
    assert!(err.is_corrupted(), "the target has another size: {err:?}");
}

/// With `reuse_delta`, an object that a pack stores whole isn't tried as a delta of the other objects of that pack, unless
/// they are preferred bases.
#[test]
fn objects_that_a_pack_stores_whole_arent_deltas_of_the_objects_of_that_pack() -> TestResult {
    let text = Rng(17).text(40);
    let whole = |pack_id| {
        Some(InPack {
            pack_id,
            is_delta: false,
        })
    };
    let as_delta = |pack_id| {
        Some(InPack {
            pack_id,
            is_delta: true,
        })
    };
    for (target_pack, base_pack, base_is_preferred, reuse_delta, is_delta) in [
        (whole(1), whole(1), false, true, false),
        (whole(1), whole(1), false, false, true),
        (whole(1), whole(1), true, true, true),
        (whole(1), whole(2), false, true, true),
        (whole(1), None, false, true, true),
        (None, whole(1), false, true, true),
        (as_delta(1), whole(1), false, true, true),
    ] {
        let mut objects = Objects::default();
        let base = objects.blob(2, &appended(&text, b"base\n"));
        objects.list[base].in_pack = base_pack;
        objects.list[base].preferred_base = base_is_preferred;
        let target = objects.blob(1, &text);
        objects.list[target].in_pack = target_pack;
        let (deltas, _) = objects.search(Options {
            reuse_delta,
            ..one_thread()
        })?;
        assert_eq!(
            base_of(&deltas, target).is_some(),
            is_delta,
            "target in {target_pack:?}, base in {base_pack:?}, preferred base: {base_is_preferred}, reuse_delta: {reuse_delta}"
        );
    }
    Ok(())
}

/// Copied deltas that depend on an object limit the depth of its delta: an object on which chains of as many deltas
/// depend as the depth allows isn't tried as a delta but remains a base, and one with chains of one delta less can only be
/// a delta of an object that is stored whole.
#[test]
fn copied_deltas_that_depend_on_an_object_limit_its_depth() -> TestResult {
    let text = Rng(19).text(40);
    let first = appended(&text, b"first\n");
    let second = appended(&first, b"second\n");
    let third = appended(&second, b"third\n");
    let mut objects = Objects::default();
    let a = objects.blob(4, &text);
    let b = objects.blob(3, &first);
    let c = objects.blob(2, &second);
    let d = objects.blob(1, &third);
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(
        [b, c, d].map(|index| base_of(&deltas, index)),
        [Some(a), Some(b), Some(c)],
        "each object has the shortest delta against the object before it"
    );

    objects.list[c].dependents_depth = 49;
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(
        base_of(&deltas, c),
        Some(a),
        "B is a delta, and the delta of C can only have the depth 1"
    );

    objects.list[c].dependents_depth = 50;
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(base_of(&deltas, c), None);
    assert_eq!(base_of(&deltas, d), Some(c), "C remains a base");
    Ok(())
}

/// The base of a delta moves to the place after its object in the window, so that the next object tries it first.
#[test]
fn bases_move_after_their_deltas() -> TestResult {
    let mut rng = Rng(23);
    let text = rng.text(80);
    // B shares only the first quarter of A, which C extends, so that C can be a delta of A but not of B.
    let b_data = [&text[..text.len() / 4], &rng.bytes(300)].concat();
    let mut objects = Objects::default();
    let a = objects.blob(3, &text);
    let b = objects.blob(2, &b_data);
    let c = objects.blob(1, &appended(&text, b"added\n"));
    let (deltas, _) = objects.search(Options {
        window: 1,
        ..one_thread()
    })?;
    assert_eq!(base_of(&deltas, b), Some(a));
    assert_eq!(
        base_of(&deltas, c),
        Some(a),
        "A moved after B, its delta, so that C, whose window holds one object, tried A rather than B"
    );
    Ok(())
}

/// An object whose delta has the greatest depth leaves the window, and the next object takes its place.
#[test]
fn the_deepest_deltas_leave_the_window() -> TestResult {
    let mut rng = Rng(29);
    let unrelated = rng.text(40);
    let text = rng.text(40);
    let mut objects = Objects::default();
    let y = objects.blob(4, &unrelated);
    let a = objects.blob(3, &text);
    let b = objects.blob(2, &appended(&text, b"added\n"));
    let c = objects.blob(1, &appended(&unrelated, b"added\n"));
    let (deltas, _) = objects.search(Options {
        window: 2,
        depth: 1,
        ..one_thread()
    })?;
    assert_eq!(base_of(&deltas, a), None);
    assert_eq!(base_of(&deltas, b), Some(a));
    assert_eq!(
        base_of(&deltas, c),
        Some(y),
        "B left the window, so that Y remained in the window of two objects"
    );
    Ok(())
}

/// Objects are only deltas of objects of their kind.
#[test]
fn objects_are_only_deltas_of_objects_of_their_kind() -> TestResult {
    let text = Rng(31).text(40);
    let mut objects = Objects::default();
    objects.add(Kind::Blob, 1, &appended(&text, b"blob\n"));
    objects.add(Kind::Tree, 1, &text);
    let (deltas, tried) = objects.search(one_thread())?;
    assert!(deltas.iter().all(Option::is_none));
    assert_eq!(tried, 2);
    Ok(())
}

/// Objects that sort alike are tried in the order in which they are listed, and of deltas of the same length, the first one
/// that is accepted is kept, unless a later one is less deep.
#[test]
fn deltas_of_the_same_length_keep_the_nearer_base_unless_another_is_less_deep() -> TestResult {
    let mut rng = Rng(37);
    let prefix = rng.bytes(1000);
    let ends = [rng.bytes(1500), rng.bytes(1500)];
    for listed in [[0, 1], [1, 0]] {
        let mut objects = Objects::default();
        let bases = listed.map(|end| objects.blob(1, &[&prefix[..], &ends[end]].concat()));
        let target = objects.blob(1, &prefix);
        let (deltas, _) = objects.search(one_thread())?;
        assert_eq!(
            base_of(&deltas, target),
            Some(bases[1]),
            "the base that is listed second is the nearer one, and is tried first"
        );
    }

    let base = [&prefix[..], &ends[0]].concat();
    let mut changed = base.clone();
    *changed.last_mut().expect("not empty") ^= 1;
    let mut objects = Objects::default();
    let whole = objects.blob(3, &base);
    let delta = objects.blob(2, &changed);
    let target = objects.blob(1, &prefix);
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(base_of(&deltas, delta), Some(whole));
    assert_eq!(
        base_of(&deltas, target),
        Some(whole),
        "the delta against the base that is stored whole has the same length, and is less deep"
    );
    Ok(())
}

/// With a window memory, the oldest objects leave the window before an object is tried while the data and the indices
/// of the objects in it take more bytes, but the object before it stays.
#[test]
fn the_window_memory_limits_the_objects_in_the_window() -> TestResult {
    let mut rng = Rng(41);
    let text = rng.text(40);
    let other = rng.text(40);
    let target_data = appended(&text, b"added\n");

    let mut objects = Objects::default();
    let base = objects.blob(3, &text);
    objects.blob(2, &other);
    let target = objects.blob(1, &target_data);
    let (deltas, _) = objects.search(one_thread())?;
    assert_eq!(base_of(&deltas, target), Some(base));
    let (deltas, _) = objects.search(Options {
        window_memory: 1,
        ..one_thread()
    })?;
    assert_eq!(base_of(&deltas, target), None, "the base left the window");

    let mut objects = Objects::default();
    objects.blob(3, &other);
    let previous = objects.blob(2, &text);
    let target = objects.blob(1, &target_data);
    let (deltas, _) = objects.search(Options {
        window_memory: 1,
        ..one_thread()
    })?;
    assert_eq!(
        base_of(&deltas, target),
        Some(previous),
        "the object before the one that is tried stays"
    );
    Ok(())
}

/// A window or a depth of 0 finds no deltas and tries no objects.
#[test]
fn a_window_or_a_depth_of_0_finds_no_deltas() -> TestResult {
    let text = Rng(43).text(40);
    let mut objects = Objects::default();
    objects.blob(2, &appended(&text, b"added\n"));
    objects.blob(1, &text);
    for options in [
        Options {
            window: 0,
            ..one_thread()
        },
        Options {
            depth: 0,
            ..one_thread()
        },
    ] {
        let (deltas, tried) = objects.search(options)?;
        assert!(deltas.iter().all(Option::is_none) && tried == 0, "{options:?}");
    }
    Ok(())
}

/// A search that is interrupted stops with an error.
#[test]
fn an_interrupted_search_fails() {
    let text = Rng(47).text(40);
    let mut objects = Objects::default();
    objects.blob(2, &appended(&text, b"added\n"));
    objects.blob(1, &text);
    for thread_limit in [Some(1), Some(2)] {
        let err = find_deltas(
            &objects.list,
            Store(Arc::new(objects.data.clone())),
            &Counter::default(),
            &AtomicBool::new(true),
            Options {
                thread_limit,
                ..Options::default()
            },
        )
        .expect_err("the search was interrupted");
        assert!(err.is_cancelled(), "{err:?}");
    }
}

/// With several threads, each object is tried once, and each delta builds its object from a base of the same kind, in
/// chains no longer than the depth. Which bases are chosen depends on the speed of the threads.
#[test]
fn deltas_that_several_threads_find_build_their_objects() -> TestResult {
    let mut rng = Rng(0x7468_7265_6164_7321);
    let mut objects = Objects::default();
    for file in 0..24 {
        let hash = name_hash(format!("dir/file-{file}.txt").as_bytes());
        let mut versions = vec![rng.text(60)];
        for _ in 1..6 {
            let next = rng.edit_lines(versions.last().expect("there is a version"));
            versions.push(next);
        }
        for version in versions.iter().rev() {
            objects.blob(hash, version);
        }
    }
    for thread_limit in [Some(2), Some(4)] {
        let options = Options {
            window: 3,
            depth: 4,
            thread_limit,
            ..Options::default()
        };
        let (deltas, tried) = objects.search(options)?;
        assert_eq!(tried, objects.list.len(), "each object is tried once");
        assert_deltas_build_their_objects(&objects.list, &objects.data, &deltas, options.depth)?;
        assert!(
            deltas.iter().flatten().count() >= objects.list.len() / 2,
            "most versions are stored as deltas"
        );
    }

    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?;
    let pack = GitPack::read(&dir.join("packed"))?;
    let objects = objects_in_order(&dir, &pack)?;
    let options = Options {
        thread_limit: Some(3),
        ..Options::default()
    };
    let (deltas, _) = search(&objects, &Store(Arc::new(pack.data.clone())), options)?;
    assert_deltas_build_their_objects(&objects, &pack.data, &deltas, options.depth)
}
