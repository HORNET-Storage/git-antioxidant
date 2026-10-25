//! [Read](read()) and [write](write()) shallow files, while performing typical operations on them.
//!
//! ## Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let first = gix_hash::ObjectId::from_hex(b"1111111111111111111111111111111111111111")?;
//! let second = gix_hash::ObjectId::from_hex(b"2222222222222222222222222222222222222222")?;
//! # let dir = tempfile::tempdir()?;
//! # let shallow_file = dir.path().join("shallow");
//! # std::fs::write(&shallow_file, format!("{first}\n"))?;
//!
//! let shallow = gix_shallow::read(&shallow_file)
//!     ?
//!     .expect("a shallow boundary");
//! let lock = gix_lock::File::acquire(
//!     &shallow_file,
//!     gix_lock::acquire::Fail::Immediately,
//!     None,
//!     0,
//!     Some(&gix_lock::acquire::resolve_symlink),
//! )
//! ?;
//! gix_shallow::write(lock, &shallow_file, Some(shallow), &[gix_shallow::Update::Shallow(second)])
//!     ?;
//!
//! let ids = gix_shallow::read(&shallow_file)
//!     ?
//!     .expect("a shallow boundary")
//!     .into_iter()
//!     .collect::<Vec<_>>();
//! assert_eq!(ids, vec![first, second]);
//! # Ok(()) }
//! ```
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use gix_error::Result;
/// An instruction on how to
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Update {
    /// Shallow the given `id`.
    Shallow(gix_hash::ObjectId),
    /// Don't shallow the given `id` anymore.
    Unshallow(gix_hash::ObjectId),
}

/// Return a list of shallow commits as unconditionally read from `shallow_file`.
///
/// The list of shallow commits represents the shallow boundary, beyond which we are lacking all (parent) commits.
/// Note that the list is never empty, as `Ok(None)` is returned in that case indicating the repository
/// isn't a shallow clone.
pub fn read(shallow_file: &std::path::Path) -> Result<Option<nonempty::NonEmpty<gix_hash::ObjectId>>> {
    use bstr::ByteSlice;
    use gix_error::{ErrorExt, ResultExt, bail, message};
    let buf = match std::fs::read(shallow_file) {
        Ok(buf) => buf,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            bail!(err.and_raise(message("Could not open shallow file for reading")));
        }
    };

    let mut commits = buf
        .lines()
        .map(gix_hash::ObjectId::from_hex)
        .collect::<std::result::Result<Vec<_>, _>>()
        .or_raise(|| gix_error::corruption("Could not decode a line in shallow file as hex-encoded object hash"))?;

    commits.sort();
    Ok(nonempty::NonEmpty::from_vec(commits))
}

///
pub mod write {
    pub(crate) mod function {
        use gix_error::Result;
        use std::io::Write;

        use gix_error::{ErrorExt, ResultExt, bail, message};

        use crate::Update;

        /// Write the [previously obtained](crate::read()) (possibly non-existing) `shallow_commits` after applying all
        /// `updates`, committing them through `file`, the lock taken for `shallow_file`.
        ///
        /// If this leaves the list of shallow commits empty, `shallow_file` is removed instead and the lock released.
        /// As in Git, a symbolic link at `shallow_file` is then removed itself and its target is left alone, while a
        /// non-empty list replaces the link's target if `file` was acquired with
        /// [`gix_lock::acquire::resolve_symlink()`].
        ///
        /// ### Deviation
        ///
        /// Git also prunes the set of shallow commits while writing, we don't until we support some sort of pruning.
        pub fn write(
            mut file: gix_lock::File,
            shallow_file: &std::path::Path,
            shallow_commits: Option<nonempty::NonEmpty<gix_hash::ObjectId>>,
            updates: &[Update],
        ) -> Result {
            let mut shallow_commits = shallow_commits.map(Vec::from).unwrap_or_default();
            for update in updates {
                match update {
                    Update::Shallow(id) => {
                        shallow_commits.push(*id);
                    }
                    Update::Unshallow(id) => shallow_commits.retain(|oid| oid != id),
                }
            }
            if shallow_commits.is_empty() {
                if let Err(err) = std::fs::remove_file(shallow_file)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    bail!(err.and_raise(message("Could not remove an empty shallow file")));
                }
                drop(file);
                return Ok(());
            }
            shallow_commits.sort();
            let mut buf = Vec::<u8>::new();
            for commit in shallow_commits {
                commit
                    .write_hex_to(&mut buf)
                    .or_raise(|| message("Failed to write object id to shallow file"))?;
                buf.push(b'\n');
            }
            file.write_all(&buf)
                .or_raise(|| message("Failed to write object id to shallow file"))?;
            file.flush()
                .or_raise(|| message("Failed to write object id to shallow file"))?;
            file.commit().or_raise(|| message("Could not commit shallow file"))?;
            Ok(())
        }
    }
}
pub use write::function::write;
