//! The second pass: read the files that changed or are new, and write their blobs.

use std::{
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, OptionExt, Result, ResultExt, bail, message};
use gix_filter::pipeline::convert::ToGitOutcome;
use gix_hash::ObjectId;
use gix_index::{
    State,
    entry::{Mode, Stage, Stat},
};

/// A path whose entries are updated from the worktree.
pub(crate) enum Work {
    /// The entry at this position, whose file may have changed or been deleted.
    Tracked(usize),
    /// The stages of a conflict at these positions, which the file in the worktree resolves.
    Unmerged(Range<usize>),
    /// A path that isn't tracked yet, which replaces the entry at the position `replaces` if it is set.
    Untracked { path: BString, replaces: Option<usize> },
}

/// What the worktree has at the path of an item of [`Work`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Update {
    /// Nothing that can be added, so the entries at the path are removed.
    Gone,
    /// A submodule whose `HEAD` doesn't point to a commit, which keeps its entry.
    Unchanged,
    /// A file, symbolic link or repository, with the entry it needs.
    Entry { mode: Mode, id: ObjectId, stat: Stat },
}

/// What reading needs on every thread.
pub(crate) struct Shared<'a, O, H> {
    pub(crate) index: &'a State,
    pub(crate) worktree: &'a Path,
    pub(crate) objects: O,
    pub(crate) repository_head: &'a H,
    pub(crate) stack: gix_worktree::Stack,
    pub(crate) filter: gix_filter::Pipeline,
    pub(crate) fs: gix_fs::Capabilities,
}

/// Read the paths of `work` on up to `thread_limit` threads, and return what the worktree has at each of them in the
/// order of `work`, along with the number of blobs that were written.
pub(crate) fn all<O, H>(
    shared: Shared<'_, O, H>,
    work: &[Work],
    thread_limit: Option<usize>,
    should_interrupt: &AtomicBool,
) -> Result<(Vec<Update>, usize)>
where
    O: gix_object::Find + gix_object::Exists + gix_object::Write + Send + Clone,
    H: Fn(&Path) -> Result<Option<ObjectId>> + Sync,
{
    let Shared {
        index,
        worktree,
        objects,
        repository_head,
        stack,
        filter,
        fs,
    } = shared;
    let written = Written::default();
    let (chunk_size, thread_limit, _) =
        gix_parallel::optimize_chunk_size_and_thread_limit(1, Some(work.len()), thread_limit, None);
    let updates = gix_parallel::in_parallel_if(
        || work.len() > 1,
        gix_utils::interrupt::Iter::new(
            work.chunks(chunk_size)
                .enumerate()
                .map(move |(chunk_index, chunk)| (chunk_index * chunk_size, chunk)),
            should_interrupt,
        ),
        thread_limit,
        {
            let written = &written;
            move |_thread| Reader {
                index,
                worktree,
                objects,
                repository_head,
                stack,
                filter,
                fs,
                written,
                verified_directory: BString::default(),
                path: PathBuf::new(),
                buf: Vec::new(),
            }
        },
        |(offset, chunk), reader| -> Result<(usize, Vec<Update>)> {
            let mut updates = Vec::with_capacity(chunk.len());
            for work in chunk {
                if should_interrupt.load(Ordering::Relaxed) {
                    break;
                }
                updates.push(reader.update(work)?);
            }
            Ok((offset, updates))
        },
        Collect {
            updates: vec![None; work.len()],
        },
    )?;
    let updates = updates
        .into_iter()
        .map(|update| update.ok_or_raise(|| message("Adding the changes of the worktree was interrupted").cancelled()))
        .collect::<Result<_>>()?;
    Ok((updates, written.count.into_inner()))
}

/// Puts the updates of each chunk of the work at their place.
struct Collect {
    updates: Vec<Option<Update>>,
}

impl gix_parallel::Reduce for Collect {
    type Input = Result<(usize, Vec<Update>)>;
    type FeedProduce = ();
    type Output = Vec<Option<Update>>;
    type Error = gix_error::Error;

    fn feed(&mut self, item: Self::Input) -> Result<Self::FeedProduce> {
        let (offset, updates) = item?;
        for (slot, update) in self.updates[offset..].iter_mut().zip(updates) {
            *slot = Some(update);
        }
        Ok(())
    }

    fn finalize(self) -> Result<Self::Output> {
        Ok(self.updates)
    }
}

/// What the worktree has at a path.
enum Disk {
    /// Nothing.
    Missing,
    /// A directory, with the commit that its `HEAD` points to if it is a repository.
    Directory { head: Option<ObjectId>, stat: Stat },
    /// A regular file or a symbolic link, whose blob was written.
    Blob { mode: Mode, id: ObjectId, stat: Stat },
}

/// Reads files and writes their blobs on one thread.
struct Reader<'a, O, H> {
    index: &'a State,
    worktree: &'a Path,
    objects: O,
    repository_head: &'a H,
    stack: gix_worktree::Stack,
    filter: gix_filter::Pipeline,
    fs: gix_fs::Capabilities,
    written: &'a Written,
    /// The deepest directory known to be a directory that isn't reached through a symbolic link.
    verified_directory: BString,
    /// The absolute path that is read.
    path: PathBuf,
    /// The content of the file that is read.
    buf: Vec<u8>,
}

impl<O, H> Reader<'_, O, H>
where
    O: gix_object::Find + gix_object::Exists + gix_object::Write,
    H: Fn(&Path) -> Result<Option<ObjectId>>,
{
    /// Return what the worktree has at the path of `work`.
    fn update(&mut self, work: &Work) -> Result<Update> {
        let index = self.index;
        let entries = index.entries();
        // The entry whose mode decides the mode of a regular file if the file system can't represent it, as Git uses it,
        // and the blob whose line endings decide whether those of the file are converted.
        let (path, reference, crlf_blob) = match work {
            Work::Tracked(entry_index) => {
                let entry = &entries[*entry_index];
                (entry.path(index), Some(entry), Some(entry.id))
            }
            Work::Unmerged(range) => {
                let conflict = &entries[range.clone()];
                let ours = conflict.iter().find(|entry| entry.stage() == Stage::Ours);
                (
                    conflict[0].path(index),
                    ours.or(conflict.first()),
                    ours.map(|entry| entry.id),
                )
            }
            Work::Untracked { path, replaces } => {
                let replaced = replaces.map(|entry_index| &entries[entry_index]);
                (path.as_bstr(), replaced, replaced.map(|entry| entry.id))
            }
        };
        // Entries are deleted if a symbolic link leads to them, as in Git. The walk doesn't follow symbolic links.
        if !matches!(work, Work::Untracked { .. }) && self.has_symlink_leading_path(path)? {
            return Ok(Update::Gone);
        }
        Ok(match self.read(path, reference.map(|entry| entry.mode), crlf_blob)? {
            Disk::Missing => Update::Gone,
            Disk::Blob { mode, id, stat } => Update::Entry { mode, id, stat },
            Disk::Directory { head: Some(id), stat } => Update::Entry {
                mode: Mode::COMMIT,
                id,
                stat,
            },
            // A directory without a commit is no submodule: the entry of a file or symbolic link that it replaced is
            // removed, a submodule that isn't checked out keeps its entry, and the others are refused, as in Git.
            Disk::Directory { head: None, .. } => match work {
                Work::Tracked(entry_index) if entries[*entry_index].mode.is_submodule() => Update::Unchanged,
                Work::Tracked(_) => Update::Gone,
                Work::Unmerged(range) if entries[range.start].mode.is_submodule() => {
                    bail!("'{path}' does not have a commit checked out".validation())
                }
                Work::Unmerged(_) => Update::Gone,
                Work::Untracked { .. } => bail!("'{path}/' does not have a commit checked out".validation()),
            },
        })
    }

    /// Return what the worktree has at `rela_path`, writing the blob of a file or symbolic link.
    ///
    /// `previous_mode` is the mode of the entry that decides the mode of a regular file, and `crlf_blob` is the blob that
    /// decides whether its line endings are converted.
    fn read(&mut self, rela_path: &BStr, previous_mode: Option<Mode>, crlf_blob: Option<ObjectId>) -> Result<Disk> {
        self.path.clear();
        self.path.push(self.worktree);
        self.path.push(gix_path::from_bstr(rela_path)?);
        let metadata = match gix_index::fs::Metadata::from_path_no_follow(&self.path) {
            Ok(metadata) => metadata,
            Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => return Ok(Disk::Missing),
            Err(err) => return Err(err.and_raise(message!("Could not read the metadata of '{rela_path}'"))),
        };
        let stat = Stat::from_fs(&metadata).or_raise(|| message("The clock was off when reading file metadata"))?;
        if metadata.is_dir() {
            let head = (self.repository_head)(&self.path)
                .or_raise(|| message!("Could not find the commit checked out in '{rela_path}'"))?;
            return Ok(Disk::Directory { head, stat });
        }
        if metadata.is_symlink() {
            let target = std::fs::read_link(&self.path)
                .or_raise(|| message!("Could not read the symbolic link '{rela_path}'"))?;
            let target = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(target)?);
            let id = write_blob(&self.objects, self.index.object_hash(), &target, self.written)?;
            return Ok(Disk::Blob {
                mode: Mode::SYMLINK,
                id,
                stat,
            });
        }
        if !metadata.is_file() {
            bail!("{rela_path}: can only add regular files, symbolic links or git-directories".validation());
        }

        let mode = file_mode(metadata.is_executable(), previous_mode, self.fs);
        let file = match gix_fs::open_read_only_no_follow(&self.path) {
            Ok(gix_fs::FileOrSymlink::File(file)) => file,
            Ok(gix_fs::FileOrSymlink::Symlink) => {
                bail!("'{rela_path}' was replaced by a symbolic link while it was added".conflict())
            }
            Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => return Ok(Disk::Missing),
            Err(err) => return Err(err.and_raise(message!("Could not open '{rela_path}'"))),
        };
        let attributes = self
            .stack
            .at_entry(rela_path, Some(mode), &self.objects)
            .or_raise(|| message!("Could not read the attributes of '{rela_path}'"))?;
        let objects = &self.objects;
        let converted = self
            .filter
            .convert_to_git(
                file,
                &gix_path::from_bstr(rela_path)?,
                &mut |_path, out| {
                    attributes.matching_attributes(out);
                },
                &mut |buf| {
                    let Some(id) = crlf_blob else {
                        return Ok(None);
                    };
                    Ok(objects
                        .try_find(&id, buf)?
                        .filter(|data| data.kind == gix_object::Kind::Blob)
                        .map(|_| ()))
                },
            )
            .or_raise(|| message!("Could not convert '{rela_path}' to what Git stores"))?;
        let data = match converted {
            ToGitOutcome::Buffer(data) => data,
            ToGitOutcome::Unchanged(mut file) => {
                self.buf.clear();
                file.read_to_end(&mut self.buf)
                    .or_raise(|| message!("Could not read '{rela_path}'"))?;
                self.buf.as_slice()
            }
            ToGitOutcome::Process(mut output) => {
                self.buf.clear();
                output
                    .read_to_end(&mut self.buf)
                    .or_raise(|| message!("Could not read what the filter made of '{rela_path}'"))?;
                self.buf.as_slice()
            }
        };
        let id = write_blob(objects, self.index.object_hash(), data, self.written)?;
        Ok(Disk::Blob { mode, id, stat })
    }

    /// Return `true` if one of the directories that lead to `rela_path` is a symbolic link. The directories that turn out
    /// to be directories are remembered, so that paths in the same directory only check what they don't share.
    fn has_symlink_leading_path(&mut self, rela_path: &BStr) -> Result<bool> {
        let Some(directory_len) = rela_path.rfind_byte(b'/') else {
            return Ok(false);
        };
        let directory = &rela_path[..directory_len];
        let mut verified = common_directory_len(&self.verified_directory, directory);
        self.verified_directory.truncate(verified);
        while verified < directory.len() {
            let start = if verified == 0 { 0 } else { verified + 1 };
            let end = directory[start..]
                .find_byte(b'/')
                .map_or(directory.len(), |slash| start + slash);
            let leading = directory[..end].as_bstr();
            self.path.clear();
            self.path.push(self.worktree);
            self.path.push(gix_path::from_bstr(leading)?);
            match std::fs::symlink_metadata(&self.path) {
                Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Ok(false),
                Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => return Ok(false),
                Err(err) => return Err(err.and_raise(message!("Could not read the metadata of '{leading}'"))),
            }
            self.verified_directory.extend_from_slice(&directory[verified..end]);
            verified = end;
        }
        Ok(false)
    }
}

/// Return the length of the leading directories that the directories `a` and `b` share.
fn common_directory_len(a: &[u8], b: &[u8]) -> usize {
    let common = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let ends_component = |path: &[u8]| path.get(common).is_none_or(|byte| *byte == b'/');
    if ends_component(a) && ends_component(b) {
        common
    } else {
        a[..common].rfind_byte(b'/').unwrap_or(0)
    }
}

/// The mode of a regular file that is `executable` on disk, given the `previous` mode of its entry, as Git's
/// `ce_mode_from_stat()` decides it: file systems without symbolic links keep the mode of an entry that is one, and file
/// systems without executable bits keep that of a regular file.
fn file_mode(executable: bool, previous: Option<Mode>, fs: gix_fs::Capabilities) -> Mode {
    match previous {
        Some(Mode::SYMLINK) if !fs.symlink => Mode::SYMLINK,
        Some(mode @ (Mode::FILE | Mode::FILE_EXECUTABLE)) if !fs.executable_bit => mode,
        _ if !fs.executable_bit => Mode::FILE,
        _ if executable => Mode::FILE_EXECUTABLE,
        _ => Mode::FILE,
    }
}

/// The blobs that the threads wrote.
#[derive(Default)]
struct Written {
    /// The ids of the blobs that were written or are being written, so that each is written once.
    ids: gix_hashtable::sync::ObjectIdMap<()>,
    /// The number of these blobs.
    count: AtomicUsize,
}

/// Write `data` as a blob to `objects` unless it has it already, and return its id.
///
/// A blob that several files have as their content is written once. The thread that writes it may still be writing when
/// another one returns its id, which is fine as the index only changes after all threads succeeded.
fn write_blob<O>(objects: &O, object_hash: gix_hash::Kind, data: &[u8], written: &Written) -> Result<ObjectId>
where
    O: gix_object::Exists + gix_object::Write,
{
    let id = gix_object::compute_hash(object_hash, gix_object::Kind::Blob, data)?;
    if !objects.exists(&id) && written.ids.insert(id, ()).is_none() {
        objects
            .write_buf_with_known_id(gix_object::Kind::Blob, data, id)
            .or_raise(|| message!("Could not write the blob {id}"))?;
        written.count.fetch_add(1, Ordering::Relaxed);
    }
    Ok(id)
}
