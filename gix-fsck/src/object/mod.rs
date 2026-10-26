//! Check objects for the problems that Git's `fsck` finds in them, as Git's `fsck.c` does.
//!
//! [`Checker::check()`] checks the format of a commit, tag or tree, and the content of the `.gitmodules` and
//! `.gitattributes` blobs that checked trees name. Such a blob may be checked before or after the trees that name it,
//! or not at all if it wasn't among the objects checked; [`Checker::finish()`] then reads and checks the blobs
//! that are still due, as Git's `fsck_finish()` does. Each finding is a [`Report`] with Git's message, and
//! [`Outcome::failed`] tells whether Git would refuse the object.
//!
//! [`Options`] select the severity of each finding as Git's `fsck.<id>` configuration does, and objects to skip as
//! `fsck.skipList` does. Git's `git fsck` checks with the default options, while `git index-pack`, and thereby
//! fetches and pushes with `transfer.fsckObjects`, check in strict mode.
//!
//! ### Deviations
//!
//! * Git's choices that depend on the platform are made as Git makes them on the platform this runs on: backslashes
//!   separate directories in names that HFS+ may take for `.gitmodules` and `.gitattributes` only on Windows, and
//!   `.gitmodules` blobs are parsed with Git's signedness of `char`. Dates from 2^63 on overflow, as in Git where
//!   `time_t` has 64 bits, while Git with a 32-bit `time_t` refuses dates from 2^31 on.
//! * Where the `tree` line of a commit, a `parent` line or the `object` line of a tag holds no object ID and the
//!   finding is configured not to fail the object, the check continues on the next line. Git's checks then read an
//!   uninitialized pointer, or read the same `parent` line again without end.
//! * [`Checker::finish()`] checks blobs in the order of their IDs, where Git's order depends on its hash table, and
//!   reports missing blobs that Git skips if they belong to a promisor remote.
//! * The longest name of tree entries is part of the [`Options`], where Git keeps one limit per process.

use bstr::BString;
use gix_error::{Result, ResultExt, message};
use gix_hash::{ObjectId, oid};
use gix_hashtable::HashSet;
use gix_object::Kind;

mod blob;
mod config;
mod headers;
mod message;
mod name;
mod options;
mod tree;
mod url;

pub use message::MessageId;
pub use options::{Options, Severity};

/// Whether a finding is reported as an error or as a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// The finding fails the object.
    Error,
    /// The finding doesn't fail the object.
    Warning,
}

/// A finding about an object, as Git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The check that made the finding.
    pub id: MessageId,
    /// How the finding is reported.
    pub level: Level,
    /// The object the finding is about.
    pub object: ObjectId,
    /// The kind of [`object`](Self::object).
    pub kind: Kind,
    /// The finding as Git words it, without the name of the check.
    pub message: BString,
}

impl std::fmt::Display for Report {
    /// The finding as Git prints it: the name of the check and the message, separated by `: `.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.id, self.message)
    }
}

/// The findings of one call to a [`Checker`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The findings in the order Git reports them.
    pub reports: Vec<Report>,
    /// Whether Git would refuse the checked objects, which happens if a finding that is reported as an error
    /// stops a check or remains its result.
    ///
    /// It is nearly always the same as the presence of an error among the reports. Like Git, a tag passes though if
    /// a finding about its `tagger` line is followed by one about further headers that doesn't fail it, as
    /// [`ExtraHeaderEntry`](MessageId::ExtraHeaderEntry) doesn't by default, and a blob that is too large for
    /// `.gitattributes` passes if that finding doesn't fail it, whatever its checks as `.gitmodules` found.
    pub failed: bool,
}

/// Check objects as Git's `fsck` does, keeping track of the `.gitmodules` and `.gitattributes` blobs that checked
/// trees name until they are checked.
#[derive(Debug, Clone)]
pub struct Checker {
    options: Options,
    object_hash: gix_hash::Kind,
    gitmodules: Queue,
    gitattributes: Queue,
}

/// Blobs that trees name `.gitmodules` or `.gitattributes`, and which of them were checked.
#[derive(Debug, Clone, Default)]
struct Queue {
    found: HashSet<ObjectId>,
    done: HashSet<ObjectId>,
}

impl Queue {
    fn pending(&self) -> Vec<ObjectId> {
        let mut pending: Vec<_> = self.found.difference(&self.done).copied().collect();
        pending.sort();
        pending
    }
}

impl Checker {
    /// Create a checker for objects with IDs of `object_hash`, configured by `options`.
    pub fn new(options: Options, object_hash: gix_hash::Kind) -> Self {
        Checker {
            options,
            object_hash,
            gitmodules: Queue::default(),
            gitattributes: Queue::default(),
        }
    }

    /// The options of this checker.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Check the object `id` of `kind` with the content `data`, as Git's `fsck_object()` does.
    ///
    /// Trees queue the `.gitmodules` and `.gitattributes` blobs they name, which are checked when they are passed
    /// to this method later, or by [`finish()`](Self::finish()). Blobs are only checked if a tree queued them before.
    pub fn check(&mut self, id: &oid, kind: Kind, data: &[u8]) -> Outcome {
        let mut reports = Reports::new(&self.options);
        let failed = match kind {
            // Nothing is reported about skipped commits and tags, so they aren't read.
            Kind::Commit => !self.options.is_skipped(id) && headers::commit(&mut reports, id, data, self.object_hash),
            Kind::Tag => !self.options.is_skipped(id) && headers::tag(&mut reports, id, data, self.object_hash),
            Kind::Tree => tree::check(
                &mut reports,
                id,
                data,
                self.object_hash,
                &mut self.gitmodules.found,
                &mut self.gitattributes.found,
            ),
            Kind::Blob => blob::check(
                &mut reports,
                id,
                Some(data),
                &mut self.gitmodules,
                &mut self.gitattributes,
            ),
        };
        reports.outcome(failed)
    }

    /// Check the blob `id` whose content is too large to load, which fails it if a tree queued it as `.gitmodules`
    /// or `.gitattributes`, as Git's `fsck_object()` does for blobs it streams.
    pub fn check_large_blob(&mut self, id: &oid) -> Outcome {
        let mut reports = Reports::new(&self.options);
        let failed = blob::check(&mut reports, id, None, &mut self.gitmodules, &mut self.gitattributes);
        reports.outcome(failed)
    }

    /// Whether [`finish()`](Self::finish()) has blobs to check, as Git's `fsck_has_queued_checks()` tells.
    pub fn has_queued_checks(&self) -> bool {
        self.gitmodules.found != self.gitmodules.done || self.gitattributes.found != self.gitattributes.done
    }

    /// Read the queued `.gitmodules` and `.gitattributes` blobs that weren't checked yet from `objects` and check
    /// them, as Git's `fsck_finish()` does, reporting those that are missing or aren't blobs. This empties the queues.
    pub fn finish(&mut self, objects: &impl gix_object::Find) -> Result<Outcome> {
        let mut reports = Reports::new(&self.options);
        let mut failed = false;
        let mut buf = Vec::new();
        for gitmodules in [true, false] {
            let (pending, file, missing, non_blob) = if gitmodules {
                let pending = self.gitmodules.pending();
                (
                    pending,
                    ".gitmodules",
                    MessageId::GitmodulesMissing,
                    MessageId::GitmodulesBlob,
                )
            } else {
                let pending = self.gitattributes.pending();
                (
                    pending,
                    ".gitattributes",
                    MessageId::GitattributesMissing,
                    MessageId::GitattributesBlob,
                )
            };
            for id in pending {
                let object = objects
                    .try_find(&id, &mut buf)
                    .or_raise(|| message!("Could not read the blob {id} that a tree names {file}"))?;
                failed |= match object {
                    None => reports.report(&id, Kind::Blob, missing, format!("unable to read {file} blob")),
                    Some(object) if object.kind == Kind::Blob => blob::check(
                        &mut reports,
                        &id,
                        Some(object.data),
                        &mut self.gitmodules,
                        &mut self.gitattributes,
                    ),
                    Some(object) => reports.report(&id, object.kind, non_blob, format!("non-blob found at {file}")),
                };
            }
            let queue = if gitmodules {
                &mut self.gitmodules
            } else {
                &mut self.gitattributes
            };
            *queue = Queue::default();
        }
        Ok(reports.outcome(failed))
    }
}

/// The findings of one call, collected as Git's `report()` passes them to its callback.
struct Reports<'a> {
    options: &'a Options,
    list: Vec<Report>,
}

impl<'a> Reports<'a> {
    fn new(options: &'a Options) -> Self {
        Reports {
            options,
            list: Vec::new(),
        }
    }

    /// Record the finding `id` about `object` of `kind` with `message`, unless the object is skipped or the finding
    /// is ignored, and return `true` if it fails the object, as Git's `report()` does with its default callback.
    fn report(&mut self, object: &oid, kind: Kind, id: MessageId, message: impl Into<BString>) -> bool {
        if self.options.is_skipped(object) {
            return false;
        }
        let level = match self.options.severity(id) {
            Severity::Ignore => return false,
            Severity::Fatal | Severity::Error => Level::Error,
            Severity::Warn | Severity::Info => Level::Warning,
        };
        self.list.push(Report {
            id,
            level,
            object: object.to_owned(),
            kind,
            message: message.into(),
        });
        level == Level::Error
    }

    fn outcome(self, failed: bool) -> Outcome {
        Outcome {
            reports: self.list,
            failed,
        }
    }
}

/// The bytes before the first NUL byte of `bytes`, which is where C code that reads them as a string stops.
fn until_nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}

/// The object ID of `kind` that the hexadecimal digits, in either case, at the start of `hex` encode, as Git's
/// `get_oid_hex_algop()` reads it.
fn parse_hex_oid(hex: &[u8], kind: gix_hash::Kind) -> Option<ObjectId> {
    let hex = hex.get(..kind.len_in_hex())?;
    let digit = |c: u8| (c as char).to_digit(16);
    let mut id = ObjectId::null(kind);
    for (byte, pair) in id.as_mut_slice().iter_mut().zip(hex.chunks_exact(2)) {
        *byte = (digit(pair[0])? << 4 | digit(pair[1])?) as u8;
    }
    Some(id)
}
