//! The checks of commits and tags, which Git writes as header lines followed by a message.

use bstr::{BString, ByteSlice};
use gix_hash::oid;
use gix_object::Kind;

use super::{MessageId, Reports, parse_hex_oid};

/// Check the commit `id` with the content `data`, as Git's `fsck_commit()` does, and return `true` if it fails.
pub(super) fn commit(reports: &mut Reports<'_>, id: &oid, data: &[u8], object_hash: gix_hash::Kind) -> bool {
    if verify(reports, id, Kind::Commit, data) {
        return true;
    }
    let report =
        |reports: &mut Reports<'_>, message_id, message: &str| reports.report(id, Kind::Commit, message_id, message);
    let Some(mut pos) = skip_prefix(data, 0, b"tree ") else {
        return report(reports, MessageId::MissingTree, "invalid format - expected 'tree' line");
    };
    match id_line(data, pos, object_hash) {
        Ok(next) => pos = next,
        Err(next) => {
            if report(reports, MessageId::BadTreeSha1, "invalid 'tree' line format - bad sha1") {
                return true;
            }
            pos = next;
        }
    }
    while let Some(after) = skip_prefix(data, pos, b"parent ") {
        match id_line(data, after, object_hash) {
            Ok(next) => pos = next,
            Err(next) => {
                if report(
                    reports,
                    MessageId::BadParentSha1,
                    "invalid 'parent' line format - bad sha1",
                ) {
                    return true;
                }
                pos = next;
            }
        }
    }
    let mut authors = 0;
    while let Some(after) = skip_prefix(data, pos, b"author ") {
        authors += 1;
        pos = after;
        if ident(reports, id, Kind::Commit, data, &mut pos) {
            return true;
        }
    }
    let failed = match authors {
        0 => report(
            reports,
            MessageId::MissingAuthor,
            "invalid format - expected 'author' line",
        ),
        1 => false,
        _ => report(
            reports,
            MessageId::MultipleAuthors,
            "invalid format - multiple 'author' lines",
        ),
    };
    if failed {
        return true;
    }
    let Some(after) = skip_prefix(data, pos, b"committer ") else {
        return report(
            reports,
            MessageId::MissingCommitter,
            "invalid format - expected 'committer' line",
        );
    };
    pos = after;
    if ident(reports, id, Kind::Commit, data, &mut pos) {
        return true;
    }
    data.contains(&0) && report(reports, MessageId::NulInCommit, "NUL byte in the commit object body")
}

/// Check the tag `id` with the content `data`, as Git's `fsck_tag()` does, and return `true` if it fails.
///
/// Like Git, a tag passes if a finding about its `tagger` line is followed by headers after that line and the
/// finding about those, [`ExtraHeaderEntry`](MessageId::ExtraHeaderEntry), doesn't fail it, as it is ignored by
/// default.
pub(super) fn tag(reports: &mut Reports<'_>, id: &oid, data: &[u8], object_hash: gix_hash::Kind) -> bool {
    if verify(reports, id, Kind::Tag, data) {
        return true;
    }
    let report =
        |reports: &mut Reports<'_>, message_id, message: &str| reports.report(id, Kind::Tag, message_id, message);
    let Some(mut pos) = skip_prefix(data, 0, b"object ") else {
        return report(
            reports,
            MessageId::MissingObject,
            "invalid format - expected 'object' line",
        );
    };
    match id_line(data, pos, object_hash) {
        Ok(next) => pos = next,
        Err(next) => {
            if report(
                reports,
                MessageId::BadObjectSha1,
                "invalid 'object' line format - bad sha1",
            ) {
                return true;
            }
            pos = next;
        }
    }
    let Some(after) = skip_prefix(data, pos, b"type ") else {
        return report(
            reports,
            MessageId::MissingTypeEntry,
            "invalid format - expected 'type' line",
        );
    };
    pos = after;
    let Some(eol) = line_end(data, pos) else {
        return report(
            reports,
            MessageId::MissingType,
            "invalid format - unexpected end after 'type' line",
        );
    };
    if !matches!(&data[pos..eol], b"commit" | b"tree" | b"blob" | b"tag")
        && report(reports, MessageId::BadType, "invalid 'type' value")
    {
        return true;
    }
    pos = eol + 1;
    let Some(after) = skip_prefix(data, pos, b"tag ") else {
        return report(
            reports,
            MessageId::MissingTagEntry,
            "invalid format - expected 'tag' line",
        );
    };
    pos = after;
    let Some(eol) = line_end(data, pos) else {
        // Git's message names the `type` line here too.
        return report(
            reports,
            MessageId::MissingTag,
            "invalid format - unexpected end after 'type' line",
        );
    };
    let name = &data[pos..eol];
    let mut reference = BString::from("refs/tags/");
    reference.extend_from_slice(name);
    if gix_validate::reference::name(reference.as_bstr()).is_err() {
        let mut message = BString::from("invalid 'tag' name: ");
        message.extend_from_slice(name);
        if reports.report(id, Kind::Tag, MessageId::BadTagName, message) {
            return true;
        }
    }
    pos = eol + 1;
    let mut failed = match skip_prefix(data, pos, b"tagger ") {
        Some(after) => {
            pos = after;
            ident(reports, id, Kind::Tag, data, &mut pos)
        }
        None => {
            if report(
                reports,
                MessageId::MissingTaggerEntry,
                "invalid format - expected 'tagger' line",
            ) {
                return true;
            }
            false
        }
    };
    if let Some(after) = skip_prefix(data, pos, b"gpgsig ").or_else(|| skip_prefix(data, pos, b"gpgsig-sha256 ")) {
        let Some(eol) = line_end(data, after) else {
            return report(
                reports,
                MessageId::BadGpgsig,
                "invalid format - unexpected end after 'gpgsig' or 'gpgsig-sha256' line",
            );
        };
        pos = eol + 1;
        while data.get(pos) == Some(&b' ') {
            let Some(eol) = line_end(data, pos) else {
                return report(
                    reports,
                    MessageId::BadHeaderContinuation,
                    "invalid format - unexpected end in 'gpgsig' or 'gpgsig-sha256' continuation line",
                );
            };
            pos = eol + 1;
        }
    }
    if data.get(pos).is_some_and(|&b| b != b'\n') {
        failed = report(
            reports,
            MessageId::ExtraHeaderEntry,
            "invalid format - extra header(s) after 'tagger'",
        );
    }
    failed
}

/// Check that the headers of a commit or tag end with a newline before the end of `data` or an empty line, without
/// a NUL byte before, as Git's `verify_headers()` does, and return `true` if the object fails.
///
/// Callers rely on it to find a newline after each header line they read.
fn verify(reports: &mut Reports<'_>, id: &oid, kind: Kind, data: &[u8]) -> bool {
    for (offset, &byte) in data.iter().enumerate() {
        match byte {
            0 => {
                let message = format!("unterminated header: NUL at offset {offset}");
                return reports.report(id, kind, MessageId::NulInHeader, message);
            }
            b'\n' if data.get(offset + 1) == Some(&b'\n') => return false,
            _ => {}
        }
    }
    data.last() != Some(&b'\n') && reports.report(id, kind, MessageId::UnterminatedHeader, "unterminated header")
}

/// Check the author, committer or tagger line that starts at `*pos` with the name, as Git's `fsck_ident()` does, and
/// return `true` if the object fails. `*pos` moves to the next line, whatever the outcome.
fn ident(reports: &mut Reports<'_>, id: &oid, kind: Kind, data: &[u8], pos: &mut usize) -> bool {
    let end = data.len();
    let mut p = *pos;
    *pos = line_end(data, p).map_or(end, |eol| eol + 1);
    // Git reads its NUL-terminated copy of the object here, so past the end there is a NUL byte.
    let at = |i: usize| data.get(i).copied().unwrap_or(0);
    let mut report = |message_id, problem: &str| {
        let message = format!("invalid author/committer line - {problem}");
        reports.report(id, kind, message_id, message)
    };
    if at(p) == b'<' {
        return report(MessageId::MissingNameBeforeEmail, "missing space before email");
    }
    loop {
        match at(p) {
            _ if p >= end => return report(MessageId::MissingEmail, "missing email"),
            b'\n' => return report(MessageId::MissingEmail, "missing email"),
            b'>' => return report(MessageId::BadName, "bad name"),
            b'<' => break,
            _ => p += 1,
        }
    }
    if at(p - 1) != b' ' {
        return report(MessageId::MissingSpaceBeforeEmail, "missing space before email");
    }
    p += 1;
    loop {
        match at(p) {
            _ if p >= end => return report(MessageId::BadEmail, "bad email"),
            b'<' | b'\n' => return report(MessageId::BadEmail, "bad email"),
            b'>' => break,
            _ => p += 1,
        }
    }
    p += 1;
    if at(p) != b' ' {
        return report(MessageId::MissingSpaceBeforeDate, "missing space before date");
    }
    p += 1;
    while matches!(at(p), b' ' | b'\t') {
        p += 1;
    }
    if !at(p).is_ascii_digit() {
        return report(MessageId::BadDate, "bad date");
    }
    if at(p) == b'0' && at(p + 1) != b' ' {
        return report(MessageId::ZeroPaddedDate, "zero-padded date");
    }
    let digits = data[p..].iter().take_while(|b| b.is_ascii_digit()).count();
    // Git reads at most 23 digits and treats more as the largest timestamp. Values from 2^63 on overflow its
    // 64-bit `time_t`.
    let overflows = digits > 23
        || data[p..p + digits]
            .iter()
            .try_fold(0u64, |value, b| value.checked_mul(10)?.checked_add(u64::from(b - b'0')))
            .is_none_or(|value| value >= 1 << 63);
    if overflows {
        return report(MessageId::BadDateOverflow, "date causes integer overflow");
    }
    p += digits;
    if at(p) != b' ' {
        return report(MessageId::BadDate, "bad date");
    }
    p += 1;
    if !matches!(at(p), b'+' | b'-') || !(1..5).all(|offset| at(p + offset).is_ascii_digit()) || at(p + 5) != b'\n' {
        return report(MessageId::BadTimezone, "bad time zone");
    }
    false
}

/// The position after `prefix` if `data` continues with it at `pos`.
fn skip_prefix(data: &[u8], pos: usize, prefix: &[u8]) -> Option<usize> {
    data.get(pos..)?.starts_with(prefix).then_some(pos + prefix.len())
}

/// The position of the newline that ends the line in which `pos` lies.
fn line_end(data: &[u8], pos: usize) -> Option<usize> {
    data.get(pos..)?.find_byte(b'\n').map(|offset| pos + offset)
}

/// Read the object ID at `pos` that ends a line, returning the position of the next line on success. Otherwise, return
/// the position at which Git continues: right after the object ID if it is followed by something else than a
/// newline, or after the end of the line if there is no object ID.
///
/// Without an object ID, Git continues after the previous object ID it read, which brings it back to the same
/// `parent` line without end, or at an uninitialized position on the `tree` and `object` lines.
fn id_line(data: &[u8], pos: usize, object_hash: gix_hash::Kind) -> Result<usize, usize> {
    let rest = data.get(pos..).unwrap_or_default();
    if parse_hex_oid(rest, object_hash).is_none() {
        return Err(line_end(data, pos).map_or(data.len(), |eol| eol + 1));
    }
    let after = pos + object_hash.len_in_hex();
    if data.get(after) == Some(&b'\n') {
        Ok(after + 1)
    } else {
        Err(after + 1)
    }
}
