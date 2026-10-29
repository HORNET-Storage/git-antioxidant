//! The commits of an object database as the walks of a rebase read them.

use std::collections::HashMap;

use bstr::{BStr, BString, ByteSlice};
use gix_error::{Result, ResultExt, bail, message};
use gix_hash::{ObjectId, oid};

/// A commit as a rebase uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The tree of the commit.
    pub tree: ObjectId,
    /// The parents of the commit, in order.
    pub parents: Vec<ObjectId>,
    /// The value of the `author` header, like `A U Thor <author@example.com> 1112912053 -0700`.
    pub author: BString,
    /// The message of the commit, everything after its headers.
    pub message: BString,
}

impl Commit {
    /// Return the subject of the commit, its first paragraph without leading empty lines, with each line trimmed at its
    /// end and the lines joined with spaces, as `find_commit_subject()` and `format_subject()` make it for `%s`.
    pub fn subject(&self) -> BString {
        subject(self.message.as_bstr())
    }

    /// Return the message from its first line that isn't empty on, as `find_commit_subject()` finds it.
    pub fn message_from_subject(&self) -> &BStr {
        skip_blank_lines(self.message.as_bstr())
    }
}

/// The commits of an object database, read once and kept, with the object database for the trees and blobs they
/// name.
pub struct Commits<'a> {
    objects: &'a dyn gix_object::Find,
    object_hash: gix_hash::Kind,
    cache: HashMap<ObjectId, Commit>,
    buf: Vec<u8>,
}

impl<'a> Commits<'a> {
    /// Read commits from `objects`, whose ids are of `object_hash`.
    pub fn new(objects: &'a dyn gix_object::Find, object_hash: gix_hash::Kind) -> Self {
        Commits {
            objects,
            object_hash,
            cache: HashMap::new(),
            buf: Vec::new(),
        }
    }

    /// Return the object database.
    pub fn objects(&self) -> &'a dyn gix_object::Find {
        self.objects
    }

    /// Return the kind of the ids of the objects.
    pub fn object_hash(&self) -> gix_hash::Kind {
        self.object_hash
    }

    /// Return the commit `id`, or an error if it isn't a commit, as `repo_parse_commit()` fails then.
    pub fn get(&mut self, id: &oid) -> Result<&Commit> {
        if !self.cache.contains_key(id) {
            let Some(data) = self.objects.try_find(id, &mut self.buf)? else {
                bail!(message!("could not parse commit {id}").not_found());
            };
            if data.kind != gix_object::Kind::Commit {
                bail!(message!("could not parse commit {id}").validation());
            }
            let commit = gix_object::CommitRef::from_bytes(data.data, self.object_hash)
                .or_raise(|| message!("could not parse commit {id}"))?;
            let commit = Commit {
                tree: commit.tree(),
                parents: commit.parents().collect(),
                author: commit.author.into(),
                message: commit.message.into(),
            };
            self.cache.insert(id.to_owned(), commit);
        }
        Ok(&self.cache[id])
    }

    /// Return the parents of the commit `id`.
    pub fn parents(&mut self, id: &oid) -> Result<Vec<ObjectId>> {
        Ok(self.get(id)?.parents.clone())
    }
}

/// Make `&dyn gix_object::Find` usable where `impl gix_object::Find` is expected.
pub(crate) struct ByRef<'a>(pub(crate) &'a dyn gix_object::Find);

impl gix_object::Find for ByRef<'_> {
    fn try_find<'b>(&self, id: &oid, buffer: &'b mut Vec<u8>) -> Result<Option<gix_object::Data<'b>>> {
        self.0.try_find(id, buffer)
    }
}

/// Return the subject of `message`, as [`Commit::subject()`] does.
pub fn subject(message: &BStr) -> BString {
    let mut out = BString::default();
    for line in skip_blank_lines(message).lines() {
        let line = line.trim_end_with(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
        if line.is_empty() {
            break;
        }
        if !out.is_empty() {
            out.push(b' ');
        }
        out.extend_from_slice(line);
    }
    out
}

/// Return `text` without its leading lines that hold only whitespace, as `skip_blank_lines()` does.
fn skip_blank_lines(text: &BStr) -> &BStr {
    let mut rest = text.as_bytes();
    loop {
        let end = rest.find_byte(b'\n').map_or(rest.len(), |end| end + 1);
        let line = &rest[..end];
        if line.is_empty() || !line.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r')) {
            return rest.as_bstr();
        }
        rest = &rest[end..];
    }
}
