use std::collections::VecDeque;

use gix_hash::ObjectId;

/// The state used and potentially shared by multiple tree traversals.
#[derive(Clone)]
pub struct State {
    next: VecDeque<ObjectId>,
    buf: Vec<u8>,
    /// The depth of the deepest tree to read, with the root tree at depth 0. A deeper tree fails the traversal, as
    /// `core.maxTreeDepth` makes it fail in Git.
    pub max_depth: usize,
}

impl Default for State {
    fn default() -> Self {
        State::new(gix_object::tree::DEFAULT_MAX_DEPTH)
    }
}

impl State {
    /// Create a state for traversals that read trees down to `max_depth`.
    pub fn new(max_depth: usize) -> Self {
        State {
            next: VecDeque::new(),
            buf: Vec::new(),
            max_depth,
        }
    }

    fn clear(&mut self) {
        self.next.clear();
        self.buf.clear();
    }
}

pub(super) mod function {
    use gix_error::Result;
    use std::borrow::BorrowMut;

    use gix_error::{ResultExt, bail, cancelled, validation};
    use gix_object::{FindExt, TreeRefIter};

    use super::State;
    use crate::tree::Visit;

    /// Start a breadth-first iteration over the `root` trees entries.
    ///
    /// Note that non-trees will be listed first, so the natural order of entries within a tree is lost.
    ///
    /// * `root`
    ///   * the tree to iterate in a nested fashion.
    /// * `state` - all state used for the iteration. If multiple iterations are performed, allocations can be minimized by reusing
    ///   this state. It also decides how deep the iteration may go.
    /// * `find` - a way to lookup new object data during traversal by their `ObjectId`, writing their data into buffer and returning
    ///   an iterator over entries if the object is present and is a tree. Caching should be implemented within this function
    ///   as needed. The return value is `Option<TreeIter>` which degenerates all error information. Not finding a commit should also
    ///   be considered an error as all objects in the tree DAG should be present in the database. Hence lookup errors should
    ///   be escalated into a more specific error if encountered by the caller.
    /// * `delegate` - A way to observe entries and control the iteration while allowing the optimizer to let you pay only for what you use.
    pub fn breadthfirst<StateMut, Find, V>(
        root: TreeRefIter<'_>,
        mut state: StateMut,
        objects: Find,
        delegate: &mut V,
    ) -> Result
    where
        Find: gix_object::Find,
        StateMut: BorrowMut<State>,
        V: Visit,
    {
        let state = state.borrow_mut();
        state.clear();
        let mut tree = root;
        let mut depth = 0;
        let mut trees_left_at_depth = 0;
        loop {
            for entry in tree {
                let entry = entry.or_raise(|| gix_error::corruption("A tree could not be decoded during traversal"))?;
                if entry.mode.is_tree() {
                    delegate.push_path_component(entry.filename);
                    let action = delegate.visit_tree(&entry);
                    match action {
                        std::ops::ControlFlow::Continue(false) => {}
                        std::ops::ControlFlow::Continue(true) => {
                            delegate.pop_path_component();
                            delegate.push_back_tracked_path_component(entry.filename);
                            state.next.push_back(entry.oid.to_owned());
                        }
                        std::ops::ControlFlow::Break(()) => {
                            bail!(cancelled("The delegate cancelled the operation"));
                        }
                    }
                } else {
                    delegate.push_path_component(entry.filename);
                    if delegate.visit_nontree(&entry).is_break() {
                        bail!(cancelled("The delegate cancelled the operation"));
                    }
                }
                delegate.pop_path_component();
            }
            // The trees of the next depth are queued behind those of the current one.
            if trees_left_at_depth == 0 {
                trees_left_at_depth = state.next.len();
                depth += 1;
            }
            match state.next.pop_front() {
                Some(oid) => {
                    if depth > state.max_depth {
                        bail!(validation("exceeded maximum allowed tree depth"));
                    }
                    trees_left_at_depth -= 1;
                    delegate.pop_front_tracked_path_and_set_current();
                    tree = objects.find_tree_iter(&oid, &mut state.buf)?;
                }
                None => break Ok(()),
            }
        }
    }
}
