use gix_hash::ObjectId;

use crate::{Header, Version};

/// Options for [`create()`](crate::create()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The version of the format, or `None` for [`Version::V2`] if the object ids are SHA-1 hashes and [`Version::V3`]
    /// otherwise, as git chooses.
    pub version: Option<Version>,
    /// The amount of threads to use at most to write the pack, or `None` to use as many as there are logical cores.
    pub thread_limit: Option<usize>,
    /// The compression level of the objects that aren't copied from a pack.
    pub compression: gix_zlib::Compression,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            version: None,
            thread_limit: None,
            compression: gix_zlib::Compression::DEFAULT,
        }
    }
}

/// What [`create()`](crate::create()) wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The header of the bundle, with the prerequisites that were found.
    pub header: Header,
    /// The amount of objects in the pack.
    pub num_objects: u32,
    /// The checksum that ends the pack and with it the bundle.
    pub pack_checksum: ObjectId,
}

pub(crate) mod function {
    use std::{
        collections::HashSet,
        io::Write,
        sync::atomic::{AtomicBool, Ordering},
    };

    use bstr::{BString, ByteSlice};
    use gix_error::{Result, ResultExt, bail, cancelled, ensure, message};
    use gix_hash::ObjectId;
    use gix_object::{FindExt, Kind};
    use gix_pack::data::output;
    use gix_utils::progress::{self, Count, DynNestedProgress, Progress};

    use super::{Options, Outcome};
    use crate::{
        Header, Prerequisite, Reference, Version,
        connectivity::{Peeled, peel},
        decode::{hash_by_name, is_git_space, trim_end},
    };

    /// Write a bundle that provides `references` to repositories having the `hidden` commits to `out`, reading objects
    /// from `objects`, as `git bundle create` does.
    ///
    /// * `objects` provides the objects and the locations of those stored in packs, which a `gix-odb` handle
    ///   reports once `prevent_pack_unload()` was called on it. It must return the object it is asked for, not its
    ///   replacement, so a `gix-odb` handle whose store has replacements needs `ignore_replacements` set.
    /// * `references` are the references the bundle provides, with the objects they point to. Their names must differ,
    ///   and their ids determine the object hash of the bundle, which `objects` must use as well.
    /// * `hidden` are commits of `objects` that the repositories using the bundle have, along with their history.
    /// * `progress` shows how many objects were selected and how many bytes were written.
    /// * `should_interrupt` is checked regularly, and once it is set, writing stops with an error.
    ///
    /// The prerequisites are the commits in the history of `hidden` that the commits of the pack name as parents, and
    /// those that references point to, directly or through annotated tags. The comment of each is the subject of its
    /// commit's message, as `git log --oneline` shows it. The pack carries the commits that the references reach and
    /// `hidden` don't, the annotated tags of the references, and the trees and blobs of these commits and references
    /// that the trees of the prerequisites don't contain. As in git, the empty tree is available even if `objects`
    /// doesn't store it.
    ///
    /// If no reference points into the history of `hidden`, the bundle has the references and prerequisites that
    /// `git bundle create` writes, though it may list the prerequisites in another order, and its pack carries the same
    /// objects, those that `git rev-list --objects` lists for the references and `hidden`. git leaves out a reference
    /// that points into that history directly, and doesn't name the commit that a tag points to as a prerequisite, so
    /// that its bundle passes verification in a repository that lacks the commit, but fetching from it fails there.
    /// Comments are taken from messages as they are stored, while git converts a message to UTF-8, or to the encoding
    /// configured for its log output, if its commit names another encoding.
    ///
    /// Objects are copied from the packs of `objects` where possible, a delta only if its base is in the pack as well.
    /// As deltas aren't computed, all other objects are stored whole, and the pack is never thin.
    ///
    /// If an error is returned, `out` may hold the beginning of a bundle, which must be discarded.
    pub fn create<T>(
        objects: T,
        references: Vec<Reference>,
        hidden: impl IntoIterator<Item = ObjectId>,
        out: &mut dyn Write,
        progress: &mut dyn DynNestedProgress,
        should_interrupt: &AtomicBool,
        Options {
            version,
            thread_limit,
            compression,
        }: Options,
    ) -> Result<Outcome>
    where
        T: gix_object::Find + gix_pack::Find + Clone + Send + 'static,
    {
        let Some(object_hash) = references.first().map(|reference| reference.id.kind()) else {
            bail!(message("A bundle needs at least one reference").validation());
        };
        let version = version.unwrap_or(if hash_by_name(b"sha1") == Some(object_hash) {
            Version::V2
        } else {
            Version::V3
        });
        let mut header = Header {
            version,
            object_hash,
            filter: None,
            prerequisites: Vec::new(),
            references,
        };
        header.check_writable()?;
        let mut names = HashSet::new();
        for reference in &header.references {
            ensure!(
                names.insert(&reference.name),
                "The bundle would provide the reference {:?} more than once".validation(),
                reference.name
            );
        }
        let hidden = hidden.into_iter().collect::<Vec<_>>();
        if let Some(commit_id) = hidden.iter().find(|commit_id| commit_id.kind() != object_hash) {
            bail!("The hidden commit {commit_id} isn't a {object_hash} hash like the references".validation());
        }

        let mut selecting = progress.add_child("select objects".into());
        selecting.init(None, progress::count("objects"));
        let Selection {
            prerequisites,
            mut object_ids,
        } = select(&objects, &header.references, hidden, &selecting, should_interrupt)?;
        header.prerequisites = prerequisites;
        let Ok(num_objects) = u32::try_from(object_ids.len()) else {
            bail!(
                "The bundle needs {} objects, more than a pack can hold".unsupported(),
                object_ids.len()
            );
        };
        header.write_to(out)?;

        // The pack needs the empty tree even if the repository doesn't store it.
        let empty_tree_id = ObjectId::empty_tree(object_hash);
        let empty_tree_entry = match object_ids.iter().position(|id| *id == empty_tree_id) {
            Some(index) if !gix_pack::Find::contains(&objects, &empty_tree_id) => {
                let count = output::Count {
                    id: object_ids.remove(index),
                    entry_pack_location: output::count::PackLocation::NotLookedUp,
                };
                let data = gix_object::Data {
                    kind: Kind::Tree,
                    object_hash,
                    data: &[],
                };
                Some(output::Entry::from_data(&count, &data, compression)?)
            }
            _ => None,
        };
        let counts = object_ids
            .into_iter()
            .map(|id| output::Count {
                id,
                entry_pack_location: output::count::PackLocation::NotLookedUp,
            })
            .collect();
        let entries = gix_parallel::InOrderIter::from(output::entry::iter_from_counts(
            counts,
            objects,
            Box::new(progress.add_child("create entries".into())),
            output::entry::iter_from_counts::Options {
                thread_limit,
                allow_thin_pack: false,
                compression,
                ..Default::default()
            },
        )?)
        .map(|entries| -> Result<_> {
            let entries = entries?;
            // Every object was found while it was selected, but one can disappear before it is written.
            ensure!(
                !entries.iter().any(output::Entry::is_invalid),
                message("An object of the bundle disappeared while its pack was written").not_found()
            );
            Ok(entries)
        })
        .chain(empty_tree_entry.map(|entry| Ok(vec![entry])));
        let mut writing = progress.add_child("write pack".into());
        writing.init(None, progress::bytes());
        let mut pack =
            output::bytes::FromEntriesIter::new(entries, out, num_objects, gix_pack::data::Version::V2, object_hash);
        for written in pack.by_ref() {
            if should_interrupt.load(Ordering::Relaxed) {
                bail!(cancelled("Interrupted while writing the pack of a bundle"));
            }
            writing.inc_by(written? as usize);
        }
        Ok(Outcome {
            header,
            num_objects,
            pack_checksum: pack.digest().expect("the pack was written to its end"),
        })
    }

    /// The prerequisites of a bundle and the objects that its pack carries.
    struct Selection {
        prerequisites: Vec<Prerequisite>,
        object_ids: Vec<ObjectId>,
    }

    /// Select the objects of the pack of a bundle that provides `references` to repositories having the history of
    /// `hidden`, as `git rev-list --objects` lists them, and find the prerequisites of the bundle.
    fn select<T>(
        objects: &T,
        references: &[Reference],
        hidden: Vec<ObjectId>,
        progress: &dyn Count,
        should_interrupt: &AtomicBool,
    ) -> Result<Selection>
    where
        T: gix_object::Find + gix_pack::Find,
    {
        let mut buf = Vec::new();
        let (mut tag_ids, mut tip_ids, mut tree_ids, mut blob_ids) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for reference in references {
            match peel(objects, reference.id, &mut tag_ids, &mut buf)? {
                Peeled::Missing(id) => bail!(
                    "The object {id} that the reference {:?} needs is missing".not_found(),
                    reference.name
                ),
                Peeled::Present(commit_id, Kind::Commit) => tip_ids.push(commit_id),
                Peeled::Present(tree_id, Kind::Tree) => tree_ids.push(tree_id),
                Peeled::Present(blob_id, Kind::Blob) => blob_ids.push(blob_id),
                Peeled::Present(_, Kind::Tag) => unreachable!("peeling follows annotated tags to their end"),
            }
        }

        // The walk ignores hidden commits that are missing, which git refuses.
        for commit_id in &hidden {
            objects
                .find_commit(commit_id, &mut buf)
                .or_raise(|| message!("Could not read the hidden commit {commit_id}"))?;
        }
        let mut commit_ids = Vec::new();
        let mut commit_tree_ids = Vec::new();
        let mut parent_ids = Vec::new();
        let mut walk = gix_traverse::commit::Simple::new(tip_ids.iter().copied(), objects).hide(hidden)?;
        while let Some(commit) = walk.next() {
            if should_interrupt.load(Ordering::Relaxed) {
                bail!(cancelled("Interrupted while selecting the commits of a bundle"));
            }
            let commit = commit.or_raise(|| message("Could not walk the commits of the bundle"))?;
            // Without a commit-graph, the walk read the commit it returned.
            let tree_id = walk
                .commit_iter()
                .tree_id()
                .or_raise(|| message!("Could not decode the commit {}", commit.id).corrupted())?;
            commit_ids.push(commit.id);
            commit_tree_ids.push(tree_id);
            parent_ids.extend(commit.parent_ids);
        }

        let sent_commit_ids = commit_ids.iter().copied().collect::<gix_hashtable::HashSet<_>>();
        let mut seen = gix_hashtable::HashSet::default();
        let mut prerequisites = Vec::new();
        for commit_id in parent_ids.into_iter().chain(tip_ids) {
            if sent_commit_ids.contains(&commit_id) || !seen.insert(commit_id) {
                continue;
            }
            let tree_id = {
                let commit = objects.find_commit(&commit_id, &mut buf)?;
                prerequisites.push(Prerequisite {
                    id: commit_id,
                    comment: comment(commit.message),
                });
                commit.tree()
            };
            add_tree(objects, tree_id, &mut seen, None, &mut buf, should_interrupt)?;
        }

        let mut object_ids = commit_ids;
        for tag_id in tag_ids {
            if seen.insert(tag_id) {
                object_ids.push(tag_id);
            }
        }
        progress.set(object_ids.len());
        for tree_id in commit_tree_ids.into_iter().chain(tree_ids) {
            add_tree(
                objects,
                tree_id,
                &mut seen,
                Some(&mut object_ids),
                &mut buf,
                should_interrupt,
            )?;
            progress.set(object_ids.len());
        }
        for blob_id in blob_ids {
            if seen.insert(blob_id) {
                object_ids.push(blob_id);
            }
        }
        progress.set(object_ids.len());
        Ok(Selection {
            prerequisites,
            object_ids,
        })
    }

    /// Add the tree `tree_id` with the trees and blobs it contains to `seen`, without entering trees that are in `seen`
    /// already, and append those that weren't seen to `sent` if it is set.
    fn add_tree<T>(
        objects: &T,
        tree_id: ObjectId,
        seen: &mut gix_hashtable::HashSet<ObjectId>,
        mut sent: Option<&mut Vec<ObjectId>>,
        buf: &mut Vec<u8>,
        should_interrupt: &AtomicBool,
    ) -> Result
    where
        T: gix_object::Find + gix_pack::Find,
    {
        let mut tree_ids = vec![tree_id];
        while let Some(tree_id) = tree_ids.pop() {
            if !seen.insert(tree_id) {
                continue;
            }
            if should_interrupt.load(Ordering::Relaxed) {
                bail!(cancelled("Interrupted while selecting the trees and blobs of a bundle"));
            }
            if let Some(sent) = sent.as_deref_mut() {
                sent.push(tree_id);
            }
            // git provides the empty tree whether or not a repository stores it.
            if tree_id.is_empty_tree() {
                continue;
            }
            for entry in objects.find_tree_iter(&tree_id, buf)? {
                let entry = entry.or_raise(|| message!("Could not decode the tree {tree_id}").corrupted())?;
                // Like git, this leaves out the commits of submodules, which belong to other repositories.
                if entry.mode.is_tree() {
                    tree_ids.push(entry.oid.to_owned());
                } else if !entry.mode.is_commit()
                    && seen.insert(entry.oid.to_owned())
                    && let Some(sent) = sent.as_deref_mut()
                {
                    ensure!(
                        gix_pack::Find::contains(objects, entry.oid),
                        "The blob {} of the tree {tree_id} is missing".not_found(),
                        entry.oid
                    );
                    sent.push(entry.oid.to_owned());
                }
            }
        }
        Ok(())
    }

    /// The comment that git writes for a prerequisite whose commit has `message`: the subject of the message, which is
    /// formed by the lines that precede its first blank line, after any blank lines it starts with. They are joined by
    /// spaces after removing the whitespace that each ends with, and the whitespace that the first starts with is
    /// removed as well. As in git, the message ends at a NUL byte, and whitespace means spaces, tabs, carriage returns
    /// and line feeds.
    fn comment(message: &[u8]) -> BString {
        let message = message.find_byte(0).map_or(message, |end| &message[..end]);
        let mut subject = BString::default();
        for line in message
            .lines()
            .map(trim_end)
            .skip_while(|line| line.is_empty())
            .take_while(|line| !line.is_empty())
        {
            if !subject.is_empty() {
                subject.push(b' ');
            }
            subject.extend_from_slice(line);
        }
        let start = subject.iter().position(|&byte| !is_git_space(byte)).unwrap_or_default();
        subject.drain(..start);
        subject
    }
}
