//! The chains of deltas that `git pack-objects` writes: where it cuts the chains of deltas that it copies from other packs,
//! how deep the copied deltas that depend on an object are, and the order in which it writes objects, each delta after
//! its base.
//!
//! Objects are given in the order in which `git pack-objects` lists them, and the base of each as its index among them, or
//! `None` for objects that are stored whole.

/// Drop the deltas in `bases` that would form a cycle or a chain of more than `depth` deltas, as Git's
/// `break_delta_chains()` does: a chain that is too long is cut into chains of `depth` deltas on one object stored
/// whole, and one chain with the deltas that remain, which the object the chain was followed from is in.
pub fn break_chains(bases: &mut [Option<u32>], depth: u32) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Unseen,
        /// On the chain that is being followed.
        Active,
        /// Its depth is final.
        Done,
    }
    let modulus = u64::from(depth) + 1;
    let mut state = vec![State::Unseen; bases.len()];
    let mut depths = vec![0u32; bases.len()];
    for start in 0..bases.len() {
        // Follow the chain from `start` down to an object stored whole or one whose depth is known, and drop the delta
        // that would lead back into the chain.
        let mut total_depth = 0u64;
        let mut current = start;
        loop {
            if state[current] == State::Done {
                total_depth += u64::from(depths[current]);
                break;
            }
            let Some(base) = bases[current].map(|base| base as usize) else {
                state[current] = State::Done;
                break;
            };
            state[current] = State::Active;
            if state[base] == State::Active {
                bases[current] = None;
                state[current] = State::Done;
                break;
            }
            current = base;
            total_depth += 1;
        }
        // Then assign the depths from `start` on, and store each object whose depth would be a multiple of `depth + 1`
        // whole.
        let mut current = Some(start);
        while let Some(index) = current {
            if state[index] == State::Done {
                break;
            }
            current = bases[index].map(|base| base as usize);
            let object_depth = total_depth % modulus;
            total_depth -= 1;
            depths[index] = object_depth as u32;
            if object_depth == 0 {
                bases[index] = None;
            }
            state[index] = State::Done;
        }
    }
}

/// The amount of deltas in the longest chain of `bases` that depends on each object, as Git's `check_delta_limit()`
/// counts them, or 0 for objects that no delta depends on. `bases` must not contain cycles.
pub fn dependents_depths(bases: &[Option<u32>]) -> Vec<u32> {
    let mut depths = vec![0u32; bases.len()];
    let mut order = Order::new(bases.len());
    for object in 0..bases.len() {
        order.write_one(object as u32, |object| bases[object as usize], |_| false);
    }
    // In reverse of an order that has each base before its deltas, each delta comes before its base.
    for &object in order.written.iter().rev() {
        if let Some(base) = bases[object as usize] {
            let depth = depths[object as usize] + 1;
            let base_depth = &mut depths[base as usize];
            *base_depth = (*base_depth).max(depth);
        }
    }
    depths
}

/// What the write order needs to know of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    /// The index of the object that the object is stored as a delta of, or `None` if it is stored whole.
    pub base: Option<u32>,
    /// The kind of the object, or `None` if it is copied as a delta from another pack or its kind isn't known. Git knows
    /// the kind of the entry of a copied delta instead, so it orders such objects with neither the commits and tags nor
    /// the trees.
    pub kind: Option<gix_object::Kind>,
    /// Whether a tag reference points to the object or peels to it, which moves it forward as Git's `mark_tagged()` does
    /// for the references in `refs/tags/`.
    pub tagged: bool,
    /// Whether the object is a preferred base of a thin pack, which isn't written.
    pub preferred_base: bool,
}

/// The indices of `objects` that aren't preferred bases, in the order in which `git pack-objects` writes them, as its
/// `compute_write_order()` and `write_one()` do. The bases of the objects must not contain cycles.
///
/// The objects come in the order of `objects` until the first one that a tag points to. From that one on, the objects
/// that tags point to come first, then the other commits and tags, then the trees, and then the other objects, each
/// with the objects whose chains of deltas end in the same object stored whole: that object, its deltas, and then the
/// deltas of each of these in turn, as Git's `add_family_to_write_order()` lists them. Then each object is written
/// after its base, which is written right before it if it wasn't yet, unless the base is a preferred base.
pub fn write_order(objects: &[Object]) -> Vec<u32> {
    use gix_object::Kind;
    let len = objects.len();
    // The first delta of each object and the next delta of the same base, in the order of `objects`.
    let (mut child, mut sibling) = (vec![None::<u32>; len], vec![None::<u32>; len]);
    for index in (0..len).rev() {
        if let Some(base) = objects[index].base {
            sibling[index] = child[base as usize];
            child[base as usize] = Some(index as u32);
        }
    }

    let mut listing = Listing {
        listed: Vec::with_capacity(len),
        filled: vec![false; len],
    };
    let last_untagged = objects.iter().position(|object| object.tagged).unwrap_or(len);
    for index in 0..last_untagged {
        listing.add(index);
    }
    let rest = || last_untagged..len;
    for index in rest().filter(|&index| objects[index].tagged) {
        listing.add(index);
    }
    for index in rest().filter(|&index| matches!(objects[index].kind, Some(Kind::Commit | Kind::Tag))) {
        listing.add(index);
    }
    for index in rest().filter(|&index| objects[index].kind == Some(Kind::Tree)) {
        listing.add(index);
    }
    for index in rest() {
        if listing.filled[index] {
            continue;
        }
        let mut root = index;
        while let Some(base) = objects[root].base {
            root = base as usize;
        }
        // Git's `add_descendants_to_write_order()`: each object with its siblings, then down to the deltas of the first
        // of them, and back up to the siblings of the objects above once an object has no deltas.
        let mut next = Some(root);
        let mut add_to_order = true;
        while let Some(current) = next {
            if add_to_order {
                listing.add(current);
                let mut later = sibling[current];
                while let Some(other) = later {
                    listing.add(other as usize);
                    later = sibling[other as usize];
                }
            }
            if let Some(first) = child[current] {
                add_to_order = true;
                next = Some(first as usize);
                continue;
            }
            add_to_order = false;
            if let Some(other) = sibling[current] {
                next = Some(other as usize);
                continue;
            }
            let mut up = objects[current].base.map(|base| base as usize);
            while let Some(above) = up.filter(|&above| sibling[above].is_none()) {
                up = objects[above].base.map(|base| base as usize);
            }
            next = up.and_then(|above| sibling[above]).map(|other| other as usize);
        }
    }

    let mut order = Order::new(len);
    for object in listing.listed {
        order.write_one(
            object,
            |object| objects[object as usize].base,
            |object| objects[object as usize].preferred_base,
        );
    }
    order.written
}

/// The objects in the order in which they are passed to Git's `write_one()`, as its `add_to_write_order()` lists them.
struct Listing {
    listed: Vec<u32>,
    filled: Vec<bool>,
}

impl Listing {
    fn add(&mut self, index: usize) {
        if !self.filled[index] {
            self.filled[index] = true;
            self.listed.push(index as u32);
        }
    }
}

/// The objects written so far, as Git's `write_one()` writes them.
struct Order {
    written: Vec<u32>,
    placed: Vec<bool>,
    chain: Vec<u32>,
}

impl Order {
    fn new(len: usize) -> Self {
        Order {
            written: Vec::with_capacity(len),
            placed: vec![false; len],
            chain: Vec::new(),
        }
    }

    /// Write `object` unless it was written or `is_skipped()`, after its base, whose base is written first in turn, as
    /// `base()` returns them, until a base was written or is skipped.
    fn write_one(&mut self, object: u32, base: impl Fn(u32) -> Option<u32>, is_skipped: impl Fn(u32) -> bool) {
        let mut current = Some(object);
        while let Some(index) = current.filter(|&index| !self.placed[index as usize] && !is_skipped(index)) {
            self.placed[index as usize] = true;
            self.chain.push(index);
            current = base(index);
        }
        self.written.extend(self.chain.drain(..).rev());
    }
}
