//! The state of a merge that stopped before its commit, which Git keeps in files of the `.git` directory until the merge
//! is committed or aborted.
//!
//! `git merge` writes it when a merge has conflicts or was told not to commit, as `write_merge_state()` in
//! `builtin/merge.c` does, `git commit` reads it to make the merge commit, as `builtin/commit.c` does, and
//! `remove_merge_branch_state()` in `branch.c` removes it once the merge is committed or aborted:
//!
//! * `MERGE_HEAD` lists the commits that are merged into `HEAD`, one per line. Its existence tells that a merge is in
//!   progress.
//! * `MERGE_MSG` holds the message of the merge commit, which `git commit` offers.
//! * `MERGE_MODE` holds `no-ff` if the merge has to make a merge commit even where `HEAD` could be fast-forwarded, and
//!   nothing otherwise.
//!
//! Each file is written to a lock file beside it, `MERGE_MSG.lock` for example, which then replaces the file, so that a
//! reader never sees part of its content. Git writes these files in place.
//!
//! ### Not handled here
//!
//! * `AUTO_MERGE`, the tree of the merge with its conflict markers, is a pseudo-reference that Git writes and deletes
//!   through its reference backend: `merge_switch_to_result()` in `merge-ort.c` writes it with `refs_update_ref()`, and
//!   `remove_merge_branch_state()` deletes it with `refs_delete_ref()`. It is left to the references of `gix`, as this
//!   crate doesn't depend on `gix-ref`.
//! * `MERGE_AUTOSTASH`, which `remove_merge_branch_state()` saves to the stash, as the stash isn't available.
//! * `SQUASH_MSG`, which `git merge --squash` writes instead of this state.

use std::{io::Write, path::Path};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, Result, ResultExt, bail, message};
use gix_hash::ObjectId;

/// The file that lists the merged commits.
const MERGE_HEAD: &str = "MERGE_HEAD";
/// The file with the message of the merge commit.
const MERGE_MSG: &str = "MERGE_MSG";
/// The file with the mode of the merge.
const MERGE_MODE: &str = "MERGE_MODE";
/// The file in which `git rerere` records the conflicts of the merge, which goes along with the state.
const MERGE_RR: &str = "MERGE_RR";

/// The commits, the message and the mode of a merge that stopped before its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The commits that are merged into `HEAD`, in the order of the lines of `MERGE_HEAD`. A merged annotated tag is listed
    /// with its own id, as Git records it.
    pub heads: Vec<ObjectId>,
    /// The message of the merge commit as `MERGE_MSG` holds it, or `None` if there is no `MERGE_MSG`.
    pub message: Option<BString>,
    /// Whether the merge commit has to be made even where `HEAD` could be fast-forwarded to the merged commits, which
    /// `MERGE_MODE` says with exactly `no-ff`, as `git merge --no-ff` writes it. `git commit` then keeps all parents.
    pub no_ff: bool,
}

impl State {
    /// Read the state of the merge in the repository whose `.git` directory is `git_dir`, and whose objects use
    /// `object_hash`, as `git commit` reads it, or return `None` if no merge is in progress as `MERGE_HEAD` doesn't exist.
    ///
    /// The lines of `MERGE_HEAD` are separated by line feeds, and the last one may lack its own.
    ///
    /// ### Errors
    ///
    /// A line of `MERGE_HEAD` that isn't the full hexadecimal id of an object of `object_hash` makes `MERGE_HEAD` corrupt,
    /// as Git says. Git also resolves other names of revisions there, which it never writes.
    pub fn read(git_dir: &Path, object_hash: gix_hash::Kind) -> Result<Option<State>> {
        let Some(content) = read_file(git_dir, MERGE_HEAD)? else {
            return Ok(None);
        };
        let mut lines: Vec<&[u8]> = content.split(|byte| *byte == b'\n').collect();
        if lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        let mut heads = Vec::with_capacity(lines.len());
        for line in lines {
            match ObjectId::from_hex(line) {
                Ok(id) if id.kind() == object_hash => heads.push(id),
                _ => bail!("Corrupt MERGE_HEAD file ({})".corrupted(), line.as_bstr()),
            }
        }
        Ok(Some(State {
            heads,
            message: read_file(git_dir, MERGE_MSG)?.map(Into::into),
            no_ff: read_file(git_dir, MERGE_MODE)?.is_some_and(|mode| mode == b"no-ff"),
        }))
    }

    /// Write this state into `git_dir`, the `.git` directory of a repository, as `write_merge_state()` writes it:
    /// `MERGE_HEAD` with the id of each head on a line of its own, `MERGE_MODE` with `no-ff` or nothing, and `MERGE_MSG`
    /// with the message, which is removed instead if there is none.
    ///
    /// `write_merge_state()` writes the message of the merge followed by a line feed, and `git merge` appends the
    /// [hint about the conflicts](conflicts_hint()) if it stops with conflicts.
    pub fn write(&self, git_dir: &Path) -> Result<()> {
        let mut heads = Vec::new();
        let mut hex = gix_hash::Kind::hex_buf();
        for id in &self.heads {
            heads.extend_from_slice(id.hex_to_buf(&mut hex).as_bytes());
            heads.push(b'\n');
        }
        write_file(git_dir, MERGE_HEAD, &heads)?;
        write_file(git_dir, MERGE_MODE, if self.no_ff { b"no-ff" } else { b"" })?;
        match &self.message {
            Some(message) => write_file(git_dir, MERGE_MSG, message),
            None => remove_file(git_dir, MERGE_MSG),
        }
    }

    /// Remove the state of the merge from `git_dir`, the `.git` directory of a repository, as
    /// `remove_merge_branch_state()` removes it: `MERGE_HEAD` first, so that the merge stops being in progress, then
    /// `MERGE_RR` of `git rerere`, `MERGE_MSG` and `MERGE_MODE`. Files that don't exist are fine.
    pub fn remove(git_dir: &Path) -> Result<()> {
        for name in [MERGE_HEAD, MERGE_RR, MERGE_MSG, MERGE_MODE] {
            remove_file(git_dir, name)?;
        }
        Ok(())
    }
}

/// Return the hint that lists the paths of the conflicts of `index`, which `git merge` appends to `MERGE_MSG` when it
/// stops with conflicts, as `append_conflicts_hint()` in `sequencer.c` writes it.
///
/// Its lines start with `comment_prefix`, the value of `core.commentChar` or `core.commentString`, which is `#` by
/// default, so that `git commit` removes them from the message. With `scissors`, which `commit.cleanup=scissors` asks
/// for, the hint begins with the scissors line, below which `git commit` ignores everything.
pub fn conflicts_hint(index: &gix_index::State, comment_prefix: &BStr, scissors: bool) -> BString {
    let mut hint = BString::default();
    if scissors {
        hint.push(b'\n');
        add_commented_lines(
            &mut hint,
            comment_prefix,
            b"------------------------ >8 ------------------------\n",
        );
        add_commented_lines(
            &mut hint,
            comment_prefix,
            b"Do not modify or remove the line above.\nEverything below it will be ignored.",
        );
        hint.extend_from_slice(comment_prefix);
    }
    hint.push(b'\n');
    add_commented_lines(&mut hint, comment_prefix, b"Conflicts:\n");
    let mut previous = None;
    for entry in index.entries().iter().filter(|entry| entry.stage_raw() != 0) {
        let path = entry.path(index);
        if previous == Some(path) {
            continue;
        }
        previous = Some(path);
        let mut line = BString::from("\t");
        line.extend_from_slice(path);
        line.push(b'\n');
        add_commented_lines(&mut hint, comment_prefix, &line);
    }
    hint
}

/// Append each line of `text` to `out` behind `prefix`, separated from it by a space unless the line is empty or starts
/// with a tab, and end `out` with a line feed, as `strbuf_add_commented_lines()` does.
fn add_commented_lines(out: &mut BString, prefix: &BStr, text: &[u8]) {
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find_byte(b'\n').map_or(rest.len(), |line_feed| line_feed + 1);
        let (line, tail) = rest.split_at(end);
        out.extend_from_slice(prefix);
        if !matches!(line.first(), Some(b'\n' | b'\t')) {
            out.push(b' ');
        }
        out.extend_from_slice(line);
        rest = tail;
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
}

/// Return the content of the file `name` in `git_dir`, or `None` if it doesn't exist.
fn read_file(git_dir: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    match std::fs::read(git_dir.join(name)) {
        Ok(content) => Ok(Some(content)),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(None),
        Err(err) => Err(err.and_raise(message!("Could not read {name}"))),
    }
}

/// Replace the file `name` in `git_dir` with `content` through a lock file beside it, which is created with the
/// permissions that Git gives these files.
fn write_file(git_dir: &Path, name: &str, content: &[u8]) -> Result<()> {
    let lock = git_dir.join(format!("{name}.lock"));
    let mut file = create_lock_file(&lock).or_raise(|| message!("Could not create {name}.lock"))?;
    file.write_all(content)
        .or_raise(|| message!("Could not write {name}.lock"))?;
    file.persist(git_dir.join(name))
        .map_err(|err| err.error)
        .or_raise(|| message!("Could not replace {name} with {name}.lock"))?;
    Ok(())
}

/// Create the file at `path` for writing, failing if it exists, and remove it if it isn't persisted. On Unix, it starts
/// with the mode `0o666` before the umask applies, as the files that Git writes do.
fn create_lock_file(path: &Path) -> std::io::Result<gix_tempfile::Handle<gix_tempfile::handle::Writable>> {
    let (directory, cleanup) = (
        gix_tempfile::ContainingDirectory::Exists,
        gix_tempfile::AutoRemove::Tempfile,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        gix_tempfile::writable_at_with_permissions(path, directory, cleanup, std::fs::Permissions::from_mode(0o666))
    }
    #[cfg(not(unix))]
    {
        gix_tempfile::writable_at(path, directory, cleanup)
    }
}

/// Remove the file `name` from `git_dir` if it exists.
fn remove_file(git_dir: &Path, name: &str) -> Result<()> {
    match std::fs::remove_file(git_dir.join(name)) {
        Ok(()) => Ok(()),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(()),
        Err(err) => Err(err.and_raise(message!("Could not remove {name}"))),
    }
}
