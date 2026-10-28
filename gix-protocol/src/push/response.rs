use bstr::{BString, ByteSlice};
use gix_error::{Result, bail};
use gix_hash::ObjectId;

/// Whether the remote unpacked the objects it received, as reported with `unpack <status>`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Unpack {
    /// The remote reported `unpack ok`.
    Ok,
    /// The remote couldn't unpack the objects, so it didn't update any reference.
    Failed {
        /// What the remote reported instead of `ok`.
        error: BString,
    },
}

/// What happened to a reference to be pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Status {
    /// The remote reported with `ok` that it updated the reference, or the reference is assumed to be updated as the
    /// remote doesn't report.
    Ok,
    /// The remote reported with `ng` that it didn't update the reference.
    Rejected {
        /// Why the reference wasn't updated, as reported by the remote.
        reason: BString,
    },
    /// The reference already points to the new object, so no command was sent for it.
    UpToDate,
    /// No command was sent to delete the reference as the remote doesn't support `delete-refs`.
    DeleteUnsupported,
    /// No command was sent as the push is atomic and another reference can't be pushed.
    AtomicPushFailed,
    /// The remote didn't report about the reference although it was asked to.
    NotReported,
}

/// What the remote did with a reference instead of what its command asked for, as reported with the `option` lines of
/// `report-status-v2`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Report {
    /// The name of the reference that was updated instead, from `option refname`.
    pub name: Option<BString>,
    /// The object the reference pointed to before, from `option old-oid`.
    pub old_id: Option<ObjectId>,
    /// The object the reference points to now, from `option new-oid`.
    pub new_id: Option<ObjectId>,
    /// If `true`, the reference was updated by force, as indicated by `option forced-update`.
    pub forced_update: bool,
}

/// The status of the reference of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RefStatus {
    /// The full name of the reference on the remote, as in the command.
    pub name: BString,
    /// What happened to the reference.
    pub status: Status,
    /// What the remote did instead of what the command asked for, as reported with `report-status-v2`, once for each
    /// `ok` line followed by `option` lines.
    pub reports: Vec<Report>,
}

/// The outcome of [`push()`](crate::push()).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Outcome {
    /// Whether the remote unpacked the objects it received, or `None` if no command was sent or the remote doesn't
    /// report.
    pub unpack: Option<Unpack>,
    /// The status of the reference of each command, in the order of the commands.
    pub refs: Vec<RefStatus>,
}

impl Outcome {
    /// Return `true` if the remote unpacked the objects it received and all references are updated or up to date,
    /// which is when `send_pack()` in Git's `send-pack.c` succeeds.
    pub fn is_success(&self) -> bool {
        !matches!(self.unpack, Some(Unpack::Failed { .. }))
            && self
                .refs
                .iter()
                .all(|r| matches!(r.status, Status::Ok | Status::UpToDate))
    }
}

/// Update `refs` with the status report `lines` the remote sent, without their trailing newlines, and return whether
/// the remote unpacked the objects, as `receive_status()` in Git's `send-pack.c` does.
///
/// Only references that expect a report or were reported before are updated. Reports about other references are
/// ignored, as are unknown `option` lines and object ids that aren't of `object_hash`.
pub fn parse_report(lines: &[BString], refs: &mut [RefStatus], object_hash: gix_hash::Kind) -> Result<Unpack> {
    let mut lines = lines.iter();
    let unpack = match lines.next() {
        None => bail!("unexpected flush packet while reading remote unpack status".corrupted()),
        Some(line) => match line.strip_prefix(b"unpack ") {
            None => bail!("unable to parse remote unpack status: {line}".corrupted()),
            Some(b"ok") => Unpack::Ok,
            Some(error) => Unpack::Failed { error: error.into() },
        },
    };

    // The reference reported last, which `option` lines refer to.
    let mut hint = None::<usize>;
    // The report `option` lines add to, which is created by the first `option` line after `ok`.
    let mut report = None::<usize>;
    let mut ok_without_report = false;
    for line in lines {
        let Some((head, rest)) = line.split_once_str(" ") else {
            bail!("invalid status line from remote: {line}".corrupted());
        };
        if head == b"option" {
            let Some(index) = hint.filter(|_| report.is_some() || ok_without_report) else {
                bail!("'option' without a matching 'ok/ng' directive".corrupted());
            };
            let reports = &mut refs[index].reports;
            if ok_without_report {
                reports.push(Report::default());
                report = Some(reports.len() - 1);
                ok_without_report = false;
            }
            let Some(report) = report.and_then(|report| reports.get_mut(report)) else {
                bail!("'option' without a matching 'ok/ng' directive".corrupted());
            };
            let (key, value) = match rest.split_once_str(" ") {
                Some((key, value)) => (key, Some(value)),
                None => (rest, None),
            };
            match key {
                b"refname" => report.name = value.map(Into::into),
                b"old-oid" => {
                    if let Some(id) = value.and_then(|value| parse_id(value, object_hash)) {
                        report.old_id = Some(id);
                    }
                }
                b"new-oid" => {
                    if let Some(id) = value.and_then(|value| parse_id(value, object_hash)) {
                        report.new_id = Some(id);
                    }
                }
                b"forced-update" => report.forced_update = true,
                _ => {}
            }
            continue;
        }

        report = None;
        ok_without_report = false;
        if head != b"ok" && head != b"ng" {
            bail!("invalid ref status from remote: {}".corrupted(), head.as_bstr());
        }
        let (name, reason) = match rest.split_once_str(" ") {
            Some((name, reason)) => (name, Some(reason)),
            None => (rest, None),
        };
        // Look for the reference after the one reported before first, as the remote reports in the order of the commands.
        hint = hint
            .and_then(|start| refs[start..].iter().position(|r| r.name == name).map(|pos| start + pos))
            .or_else(|| refs.iter().position(|r| r.name == name));
        let Some(index) = hint else {
            continue;
        };
        let r = &mut refs[index];
        if !matches!(r.status, Status::NotReported | Status::Ok | Status::Rejected { .. }) {
            continue;
        }
        if head == b"ng" {
            r.status = Status::Rejected {
                reason: reason.unwrap_or(b"failed").into(),
            };
        } else {
            r.status = Status::Ok;
            ok_without_report = true;
        }
    }
    Ok(unpack)
}

/// Parse the object id of kind `object_hash` at the start of `hex`, as `parse_oid_hex_algop()` in Git does.
fn parse_id(hex: &[u8], object_hash: gix_hash::Kind) -> Option<ObjectId> {
    ObjectId::from_hex(hex.get(..object_hash.len_in_hex())?).ok()
}
