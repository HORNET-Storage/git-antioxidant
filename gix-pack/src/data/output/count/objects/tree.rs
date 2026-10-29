/// The path of the entry that a traversal visits, tracked as the recorders of `gix-diff` and `gix-traverse` track it, for
/// the name hashes of the objects that counting finds.
#[derive(Default)]
struct Path {
    current: Vec<u8>,
    /// The paths of the trees that are queued to be visited.
    queued: std::collections::VecDeque<Vec<u8>>,
}

impl Path {
    fn clear(&mut self) {
        self.current.clear();
        self.queued.clear();
    }

    fn push(&mut self, component: &[u8]) {
        if component.is_empty() {
            return;
        }
        if !self.current.is_empty() {
            self.current.push(b'/');
        }
        self.current.extend_from_slice(component);
    }

    fn pop(&mut self) {
        let len = self.current.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
        self.current.truncate(len);
    }

    fn push_back_tracked(&mut self, component: &[u8]) {
        self.push(component);
        self.queued.push_back(self.current.clone());
    }

    fn pop_front_tracked(&mut self) {
        self.current = self.queued.pop_front().unwrap_or_default();
    }

    fn pop_back_tracked(&mut self) {
        self.current = self.queued.pop_back().unwrap_or_default();
    }

    fn name_hash(&self) -> u32 {
        crate::data::delta::search::name_hash(&self.current)
    }
}

pub mod changes {
    use gix_diff::tree::{
        Visit,
        visit::{Action, Change},
    };
    use gix_hash::ObjectId;
    use gix_object::bstr::BStr;

    use crate::data::output::count::objects_impl::util::InsertImmutable;

    pub struct AllNew<'a, H> {
        /// The objects that are new, with the name hash of the path at which they are found.
        pub objects: Vec<(ObjectId, u32)>,
        all_seen: &'a H,
        path: super::Path,
    }

    impl<'a, H> AllNew<'a, H>
    where
        H: InsertImmutable,
    {
        pub fn new(all_seen: &'a H) -> Self {
            AllNew {
                objects: Default::default(),
                all_seen,
                path: Default::default(),
            }
        }
        pub fn clear(&mut self) {
            self.objects.clear();
            self.path.clear();
        }
    }

    impl<H> Visit for AllNew<'_, H>
    where
        H: InsertImmutable,
    {
        fn pop_front_tracked_path_and_set_current(&mut self) {
            self.path.pop_front_tracked();
        }

        fn push_back_tracked_path_component(&mut self, component: &BStr) {
            self.path.push_back_tracked(component);
        }

        fn push_path_component(&mut self, component: &BStr) {
            self.path.push(component);
        }

        fn pop_path_component(&mut self) {
            self.path.pop();
        }

        fn visit(&mut self, change: Change) -> Action {
            match change {
                Change::Addition {
                    oid,
                    entry_mode,
                    relation: _,
                }
                | Change::Modification { oid, entry_mode, .. } => {
                    if entry_mode.is_commit() {
                        return std::ops::ControlFlow::Continue(());
                    }
                    let inserted = self.all_seen.insert(oid);
                    if inserted {
                        self.objects.push((oid, self.path.name_hash()));
                    }
                }
                Change::Deletion { .. } => {}
            }
            std::ops::ControlFlow::Continue(())
        }
    }
}

pub mod traverse {
    use gix_hash::ObjectId;
    use gix_object::{bstr::BStr, tree::EntryRef};
    use gix_traverse::tree::{Visit, visit::Action};

    use crate::data::output::count::objects_impl::util::InsertImmutable;

    pub struct AllUnseen<'a, H> {
        /// The objects other than trees that weren't seen before, with the name hash of the path at which they are found.
        pub non_trees: Vec<(ObjectId, u32)>,
        /// The name hash of the path of each tree that wasn't seen before.
        pub tree_name_hashes: gix_hashtable::HashMap<ObjectId, u32>,
        all_seen: &'a H,
        path: super::Path,
    }

    impl<'a, H> AllUnseen<'a, H>
    where
        H: InsertImmutable,
    {
        pub fn new(all_seen: &'a H) -> Self {
            AllUnseen {
                non_trees: Default::default(),
                tree_name_hashes: Default::default(),
                all_seen,
                path: Default::default(),
            }
        }
        pub fn clear(&mut self) {
            self.non_trees.clear();
            self.tree_name_hashes.clear();
            self.path.clear();
        }
    }

    impl<H> Visit for AllUnseen<'_, H>
    where
        H: InsertImmutable,
    {
        fn pop_back_tracked_path_and_set_current(&mut self) {
            self.path.pop_back_tracked();
        }

        fn pop_front_tracked_path_and_set_current(&mut self) {
            self.path.pop_front_tracked();
        }

        fn push_back_tracked_path_component(&mut self, component: &BStr) {
            self.path.push_back_tracked(component);
        }

        fn push_path_component(&mut self, component: &BStr) {
            self.path.push(component);
        }

        fn pop_path_component(&mut self) {
            self.path.pop();
        }

        fn visit_tree(&mut self, entry: &EntryRef<'_>) -> Action {
            let inserted = self.all_seen.insert(entry.oid.to_owned());
            if inserted {
                self.tree_name_hashes
                    .insert(entry.oid.to_owned(), self.path.name_hash());
                std::ops::ControlFlow::Continue(true)
            } else {
                std::ops::ControlFlow::Continue(false)
            }
        }

        fn visit_nontree(&mut self, entry: &EntryRef<'_>) -> Action {
            if entry.mode.is_commit() {
                return std::ops::ControlFlow::Continue(true);
            }
            let inserted = self.all_seen.insert(entry.oid.to_owned());
            if inserted {
                self.non_trees.push((entry.oid.to_owned(), self.path.name_hash()));
            }
            std::ops::ControlFlow::Continue(true)
        }
    }
}
