use std::io::{BufRead, Read};

use bstr::{BString, ByteSlice};
use gix_error::{Result, ResultExt, bail, message};
use gix_hash::ObjectId;

use crate::{Header, Prerequisite, Reference, Version};

/// The first line of a v2 bundle.
pub(crate) const V2_SIGNATURE: &[u8] = b"# v2 git bundle\n";
/// The first line of a v3 bundle.
pub(crate) const V3_SIGNATURE: &[u8] = b"# v3 git bundle\n";

impl Header {
    /// Read a bundle header from `read`, consuming exactly its bytes so that the bundle's pack is what remains to be read.
    ///
    /// Each line is checked as `git bundle` checks it, except for these differences, which only refuse more:
    ///
    /// * The header must end with its empty line, while git ends a header at the end of the input as well.
    /// * A v2 bundle uses SHA-1, the only object hash git writes it with, while git reads it with the hash of the
    ///   repository that reads it.
    /// * A v3 bundle names its object hash with the `object-format` capability, which git always writes.
    /// * Capabilities precede all prerequisites and references, as the format specifies, and each appears at most once.
    ///
    /// Lines are read whole. To bound the memory used for untrusted input, limit what `read` provides,
    /// for example with [`Read::take()`].
    pub fn from_read(read: &mut dyn BufRead) -> Result<Header> {
        let mut line = Vec::new();
        Read::take(&mut *read, V2_SIGNATURE.len() as u64)
            .read_until(b'\n', &mut line)
            .or_raise(|| message("Could not read the signature of the bundle"))?;
        let version = match line.as_slice() {
            V2_SIGNATURE => Version::V2,
            V3_SIGNATURE => Version::V3,
            _ => bail!(
                message("The input doesn't start with the signature of a v2 or v3 git bundle")
                    .corrupted()
                    .with_input(line.as_slice())
            ),
        };

        let mut object_hash = match version {
            Version::V2 => Some(hash_by_name(b"sha1").ok_or_else(|| {
                message("A v2 bundle uses SHA-1, whose support isn't compiled in").unsupported_error()
            })?),
            Version::V3 => None,
        };
        let mut filter = None;
        let mut prerequisites = Vec::new();
        let mut references = Vec::new();
        let mut line_number = 1;
        loop {
            line_number += 1;
            line.clear();
            read.read_until(b'\n', &mut line)
                .or_raise(|| message!("Could not read line {line_number} of the bundle header"))?;
            if line.last() != Some(&b'\n') {
                bail!("The bundle header ends in line {line_number}, before the empty line that ends it".corrupted());
            }
            if line.len() == 1 {
                break;
            }

            let content = trim_end(&line);
            if let Some(capability) = content.strip_prefix(b"@") {
                if version == Version::V2 {
                    bail!(
                        "Line {line_number} of the bundle header is a capability, which v2 bundles don't have"
                            .corrupted()
                            .with_input(content)
                    );
                }
                if !prerequisites.is_empty() || !references.is_empty() {
                    bail!(
                        "Line {line_number} of the bundle header is a capability, but capabilities precede prerequisites and references"
                            .corrupted()
                            .with_input(content)
                    );
                }
                parse_capability(capability, &mut object_hash, &mut filter)
                    .or_raise(|| message!("Line {line_number} of the bundle header is an invalid capability"))?;
                continue;
            }

            let Some(hash) = object_hash else {
                bail!("A v3 bundle names its object hash with the object-format capability, but line {line_number} precedes it".corrupted());
            };
            let (is_prerequisite, rest) = match content.strip_prefix(b"-") {
                Some(rest) => (true, rest),
                None => (false, content),
            };
            let Some((hex, after_id)) = rest.split_at_checked(hash.len_in_hex()) else {
                bail!(
                    "Line {line_number} of the bundle header doesn't start with an object id"
                        .corrupted()
                        .with_input(content)
                );
            };
            let id = ObjectId::from_hex(hex).or_raise(|| {
                message!("Line {line_number} of the bundle header doesn't start with an object id")
                    .corrupted()
                    .with_input(content)
            })?;
            let text = match after_id.split_first() {
                None => None,
                Some((&separator, text)) if is_git_space(separator) => Some(text),
                Some(_) => bail!(
                    "Line {line_number} of the bundle header continues its object id without whitespace"
                        .corrupted()
                        .with_input(content)
                ),
            };
            if is_prerequisite {
                prerequisites.push(Prerequisite {
                    id,
                    comment: text.unwrap_or_default().into(),
                });
            } else {
                let Some(name) = text.filter(|name| !name.is_empty()) else {
                    bail!(
                        "Line {line_number} of the bundle header names no reference after its object id"
                            .corrupted()
                            .with_input(content)
                    );
                };
                references.push(Reference { id, name: name.into() });
            }
        }

        let Some(object_hash) = object_hash else {
            bail!(
                message("A v3 bundle names its object hash with the object-format capability, which is missing")
                    .corrupted()
            );
        };
        Ok(Header {
            version,
            object_hash,
            filter,
            prerequisites,
            references,
        })
    }
}

/// Apply `capability`, a capability line without its leading `@`.
fn parse_capability(
    capability: &[u8],
    object_hash: &mut Option<gix_hash::Kind>,
    filter: &mut Option<BString>,
) -> Result {
    if let Some(name) = capability.strip_prefix(b"object-format=") {
        if object_hash.is_some() {
            bail!(message("The object-format capability appears more than once").corrupted());
        }
        let Some(kind) = hash_by_name(name) else {
            bail!("The object hash {:?} isn't supported".unsupported(), name.as_bstr());
        };
        *object_hash = Some(kind);
    } else if let Some(spec) = capability.strip_prefix(b"filter=") {
        if filter.is_some() {
            bail!(message("The filter capability appears more than once").corrupted());
        }
        if spec.is_empty() {
            bail!(message("The filter capability names no filter").corrupted());
        }
        *filter = Some(spec.into());
    } else {
        bail!(
            "The capability {:?} isn't supported".unsupported(),
            capability.as_bstr()
        );
    }
    Ok(())
}

/// The object hash that git calls `name`, as in the `object-format` capability, if its support is compiled in.
pub(crate) fn hash_by_name(name: &[u8]) -> Option<gix_hash::Kind> {
    gix_hash::Kind::all()
        .iter()
        .copied()
        .find(|kind| kind.to_string().as_bytes() == name)
}

/// Whether `byte` is whitespace to git's `isspace()`, which unlike [`u8::is_ascii_whitespace()`] excludes form feeds.
pub(crate) fn is_git_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// `line` without the trailing whitespace that git's `strbuf_rtrim()` removes.
pub(crate) fn trim_end(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|&byte| !is_git_space(byte))
        .map_or(0, |last| last + 1);
    &line[..end]
}
