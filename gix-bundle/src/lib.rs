//! Read, write, verify and unbundle git bundles.
//!
//! A bundle carries a pack together with a header that names the references the pack provides and the prerequisite
//! objects a repository must already have to use it. This crate implements the v2 and v3 formats described in
//! [gitformat-bundle(5)](https://git-scm.com/docs/gitformat-bundle):
//!
//! * [`Header::from_read()`] reads a header and leaves the pack to be read from the same input.
//! * [`Header::write_to()`] writes a header, which the pack follows.
//! * [`create()`] writes a bundle that provides references to repositories having certain commits, choosing the
//!   prerequisites and the objects of its pack as `git bundle create` does.
//! * [`Header::verify_prerequisites()`] checks that a repository has every prerequisite along with all objects
//!   reachable from it, as `git bundle verify` does.
//! * [`Header::unbundle()`] also writes the pack into the pack directory of a repository, adding the objects that a thin
//!   pack lacks, and checks that every reference is complete before the pack appears in the directory, as fetching from
//!   a bundle with `git fetch` does.
#![deny(unsafe_code, missing_docs)]

use bstr::BString;
use gix_hash::ObjectId;

/// The version of the bundle format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Version {
    /// The format whose object ids are SHA-1 hashes and which has no capabilities.
    V2,
    /// The format that names its object hash in the `object-format` capability and may carry an object filter.
    V3,
}

/// An object that a repository must have, along with all objects reachable from it, to use a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Prerequisite {
    /// The id of the object.
    pub id: ObjectId,
    /// The text after the id, typically the subject of a commit. It has no meaning, and readers ignore it.
    pub comment: BString,
}

/// A reference that a bundle provides.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Reference {
    /// The object the reference points to.
    pub id: ObjectId,
    /// The name of the reference, like `refs/heads/main`.
    ///
    /// As git does, this crate doesn't check it as a reference name. That is left to whoever creates a reference from it.
    pub name: BString,
}

/// The header of a bundle, which the bundle's pack follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The version of the format.
    pub version: Version,
    /// The hash of all object ids in the bundle, which is SHA-1 for [`Version::V2`].
    pub object_hash: gix_hash::Kind,
    /// The object filter the pack was created with, in the form `git rev-list --filter` takes it, if any.
    ///
    /// Only [`Version::V3`] can carry a filter. The pack of a filtered bundle lacks objects on purpose.
    pub filter: Option<BString>,
    /// The objects a repository must have to use the bundle.
    pub prerequisites: Vec<Prerequisite>,
    /// The references the bundle provides.
    pub references: Vec<Reference>,
}

mod connectivity;
/// The options and outcome of [`create()`].
pub mod create;
pub use create::function::create;
mod decode;
mod encode;
mod unbundle;
mod verify;
