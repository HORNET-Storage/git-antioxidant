//! The checks of blobs that trees name `.gitmodules` or `.gitattributes`, as Git's `fsck_blob()` performs them.

use bstr::{BString, ByteSlice};
use gix_hash::oid;
use gix_object::Kind;

use super::{MessageId, Queue, Reports, config, until_nul, url};

/// The largest `.gitattributes` blob Git reads.
const MAX_ATTRIBUTES_SIZE: usize = 100 * 1024 * 1024;
/// The length from which Git doesn't read a line of `.gitattributes`.
const MAX_ATTRIBUTES_LINE_LENGTH: usize = 2048;

/// Check the blob `id` with the content `data`, or `None` if it is too large to load, as Git's `fsck_blob()` does,
/// if a tree queued it in `gitmodules` or `gitattributes`, and return `true` if it fails.
///
/// Like Git, a blob that is too large for `.gitattributes` fails, or not, by that finding alone.
pub(super) fn check(
    reports: &mut Reports<'_>,
    id: &oid,
    data: Option<&[u8]>,
    gitmodules: &mut Queue,
    gitattributes: &mut Queue,
) -> bool {
    if reports.options.is_skipped(id) {
        return false;
    }
    let mut failed = false;
    if gitmodules.found.contains(id) {
        gitmodules.done.insert(id.to_owned());
        let Some(data) = data else {
            return reports.report(
                id,
                Kind::Blob,
                MessageId::GitmodulesLarge,
                ".gitmodules too large to parse",
            );
        };
        let parsed = config::parse(data, |name, value| {
            failed |= submodule_variable(reports, id, name, value);
        });
        if !parsed {
            failed |= reports.report(
                id,
                Kind::Blob,
                MessageId::GitmodulesParse,
                "could not parse gitmodules blob",
            );
        }
    }
    if gitattributes.found.contains(id) {
        gitattributes.done.insert(id.to_owned());
        let Some(data) = data.filter(|data| data.len() <= MAX_ATTRIBUTES_SIZE) else {
            return reports.report(
                id,
                Kind::Blob,
                MessageId::GitattributesLarge,
                ".gitattributes too large to parse",
            );
        };
        // Git reads the blob as text that ends at its first NUL byte.
        if until_nul(data)
            .split(|&b| b == b'\n')
            .any(|line| line.len() >= MAX_ATTRIBUTES_LINE_LENGTH)
        {
            failed |= reports.report(
                id,
                Kind::Blob,
                MessageId::GitattributesLineLength,
                ".gitattributes has too long lines to parse",
            );
        }
    }
    failed
}

/// Check the variable `name` with `value` from the `.gitmodules` blob `id`, as Git's `fsck_gitmodules_fn()` does, and
/// return `true` if the blob fails.
///
/// Variables of the form `submodule.<name>.<key>` need a valid name, and `url`, `path` and `update` keys need
/// values that are safe to pass to commands. Like Git, `name` and `value` are read up to a NUL byte.
fn submodule_variable(reports: &mut Reports<'_>, id: &oid, name: &[u8], value: Option<&[u8]>) -> bool {
    let name = until_nul(name);
    let value = value.map(until_nul);
    let Some(rest) = name.strip_prefix(b"submodule.") else {
        return false;
    };
    // The key follows the last period; without a period before it there is no subsection.
    let Some(dot) = rest.rfind_byte(b'.') else {
        return false;
    };
    let (submodule, key) = (&rest[..dot], &rest[dot + 1..]);
    let mut report = |message_id, message: &str, subject: &[u8]| {
        let mut message = BString::from(message);
        message.extend_from_slice(subject);
        reports.report(id, Kind::Blob, message_id, message)
    };
    let mut failed = false;
    if gix_validate::submodule::name(submodule.as_bstr()).is_err() {
        failed |= report(MessageId::GitmodulesName, "disallowed submodule name: ", submodule);
    }
    let Some(value) = value else {
        return failed;
    };
    match key {
        b"url" if !url::is_allowed_submodule_url(value) => {
            failed |= report(MessageId::GitmodulesUrl, "disallowed submodule url: ", value);
        }
        b"path" if value.first() == Some(&b'-') => {
            failed |= report(MessageId::GitmodulesPath, "disallowed submodule path: ", value);
        }
        // An `update` setting that starts with `!` names a command to run.
        b"update" if value.first() == Some(&b'!') => {
            failed |= report(
                MessageId::GitmodulesUpdate,
                "disallowed submodule update setting: ",
                value,
            );
        }
        _ => {}
    }
    failed
}
