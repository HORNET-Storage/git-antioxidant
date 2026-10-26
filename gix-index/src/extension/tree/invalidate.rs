use bstr::{BStr, ByteSlice};

use crate::extension::Tree;

impl Tree {
    /// Invalidate the nodes of this cache tree along `path`, the path of an index entry that was added, changed or
    /// removed, as Git does whenever it changes the index.
    ///
    /// The root and the nodes of the directories that lead to `path` become invalid, and the node of a directory at
    /// `path` itself is removed, as the entry there replaced it. The path of a sparse directory entry ends in a slash.
    pub fn invalidate_path(&mut self, path: &BStr) {
        let mut node = self;
        let mut rest = path.as_bytes();
        loop {
            node.num_entries = None;
            node.id = node.id.kind().null();
            let Some(slash) = rest.find_byte(b'/') else {
                node.children.retain(|child| child.name.as_slice() != rest);
                return;
            };
            let name = &rest[..slash];
            let Some(child) = node.children.iter_mut().find(|child| child.name.as_slice() == name) else {
                return;
            };
            node = child;
            rest = &rest[slash + 1..];
        }
    }
}
