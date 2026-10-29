//! Reading and writing the files of the state directories, as Git reads and writes them.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use bstr::BString;
use gix_error::{ErrorExt, Result, ResultExt, message};
use gix_hash::ObjectId;

/// Return the content of the file at `path`, or `None` if it doesn't exist.
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(None),
        Err(err) => Err(err.and_raise(message!("could not read '{}'", path.display()))),
    }
}

/// Return the content of the file at `path` without the line feed at its end and a carriage return before it, as
/// `read_oneliner()` reads the files that a shell script could have written, or `None` if it doesn't exist.
pub(crate) fn read_line(path: &Path) -> Result<Option<BString>> {
    Ok(read(path)?.map(|mut content| {
        if content.last() == Some(&b'\n') {
            content.pop();
            if content.last() == Some(&b'\r') {
                content.pop();
            }
        }
        content.into()
    }))
}

/// Return `true` if something exists at `path`, as `file_exists()` tells.
pub(crate) fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Replace the file at `path` with `content` through a lock file beside it, as `write_message()` does.
pub(crate) fn write(path: &Path, content: &[u8]) -> Result<()> {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    let lock = PathBuf::from(lock);
    let mut file = create_lock_file(&lock).or_raise(|| message!("could not lock '{}'", path.display()))?;
    file.write_all(content)
        .or_raise(|| message!("could not write to '{}'", lock.display()))?;
    file.persist(path)
        .map_err(|err| err.error)
        .or_raise(|| message!("failed to finalize '{}'", path.display()))?;
    Ok(())
}

/// Append `content` to the file at `path`, creating it if it doesn't exist, as Git appends with `O_APPEND`.
pub(crate) fn append(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .or_raise(|| message!("could not open '{}' for writing", path.display()))?;
    file.write_all(content)
        .or_raise(|| message!("could not write to '{}'", path.display()))
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

/// Remove the file at `path` if it exists.
pub(crate) fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(()),
        Err(err) => Err(err.and_raise(message!("could not remove '{}'", path.display()))),
    }
}

/// Remove the directory at `path` with everything in it if it exists, as `remove_dir_recursively()` does.
pub(crate) fn remove_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => Ok(()),
        Err(err) => Err(err.and_raise(message!("could not remove '{}'", path.display()))),
    }
}

/// Create the directory at `path` and the directories leading to it if they don't exist.
pub(crate) fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).or_raise(|| message!("could not create directory '{}'", path.display()))
}

/// Parse the object id of `object_hash` at the start of `hex`, ignoring what follows it, as `get_oid_hex()` does.
pub(crate) fn id_at_start(hex: &[u8], object_hash: gix_hash::Kind) -> Option<ObjectId> {
    hex.get(..object_hash.len_in_hex())
        .and_then(|hex| ObjectId::from_hex(hex).ok())
        .filter(|id| id.kind() == object_hash)
}

/// Parse `hex` as a whole as the object id of `object_hash`, as `parse_oid_hex()` does when it checks that nothing follows.
pub(crate) fn id(hex: &[u8], object_hash: gix_hash::Kind) -> Option<ObjectId> {
    (hex.len() == object_hash.len_in_hex())
        .then(|| id_at_start(hex, object_hash))
        .flatten()
}

/// Append the hexadecimal form of `id` to `out`.
pub(crate) fn push_hex(out: &mut BString, id: &gix_hash::oid) {
    let mut buf = gix_hash::Kind::hex_buf();
    out.extend_from_slice(id.hex_to_buf(&mut buf).as_bytes());
}
