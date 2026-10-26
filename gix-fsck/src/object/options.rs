use bstr::{BStr, ByteSlice};
use gix_error::{Result, bail, message};
use gix_hash::{ObjectId, oid};

use super::{MessageId, message::COUNT, parse_hex_oid, until_nul};

/// How a check's finding is treated, from the most to the least serious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The finding is reported as an error and fails the object. Checks with this default severity can't be
    /// configured to anything but an error, as the rest of the object can't be read safely after them.
    Fatal,
    /// The finding is reported as an error and fails the object.
    Error,
    /// The finding is reported as a warning. In [strict](Options::strict()) mode, checks with this default
    /// severity that aren't configured otherwise report errors instead.
    Warn,
    /// The finding is reported as a warning, also in [strict](Options::strict()) mode.
    Info,
    /// The finding isn't reported.
    Ignore,
}

impl Severity {
    /// The severity that `value` configures, which is one of Git's `error`, `warn` and `ignore`.
    pub fn from_config_value(value: &BStr) -> Option<Severity> {
        match value.as_bytes() {
            b"error" => Some(Severity::Error),
            b"warn" => Some(Severity::Warn),
            b"ignore" => Some(Severity::Ignore),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Severity::Fatal => "fatal",
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Info => "info",
            Severity::Ignore => "ignore",
        }
    }
}

/// What the object checks report and which objects they skip, configured as `git fsck` and the transfer of objects
/// with `transfer.fsckObjects` configure them.
///
/// Git's `git fsck` checks with the [default](Options::default()) options, while `git index-pack`, and thereby
/// fetches and pushes that check objects, check in [strict](Options::strict()) mode.
#[derive(Debug, Clone)]
pub struct Options {
    strict: bool,
    severities: [Option<Severity>; COUNT],
    skip: gix_hashtable::HashSet<ObjectId>,
    max_path_component_len: i64,
}

impl Default for Options {
    /// Options in which each check has its default severity, and tree entries may have names of up to 4096 bytes.
    fn default() -> Self {
        Options {
            strict: false,
            severities: [None; COUNT],
            skip: Default::default(),
            max_path_component_len: 4096,
        }
    }
}

impl Options {
    /// Options in which checks that warn by default report errors instead, unless they are configured otherwise,
    /// and tree entries with mode `100664` are reported as [`BadFilemode`](MessageId::BadFilemode).
    pub fn strict() -> Self {
        Options {
            strict: true,
            ..Default::default()
        }
    }

    /// Whether these are [strict](Options::strict()) options.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// The severity of `id`: the configured one, or otherwise its default, which strict mode raises from
    /// [`Warn`](Severity::Warn) to [`Error`](Severity::Error).
    pub fn severity(&self, id: MessageId) -> Severity {
        self.severities[id as usize].unwrap_or_else(|| match id.default_severity() {
            Severity::Warn if self.strict => Severity::Error,
            severity => severity,
        })
    }

    /// Configure the severity of `id`.
    ///
    /// Checks whose default severity is [`Fatal`](Severity::Fatal) can only be set to [`Error`](Severity::Error) or
    /// [`Fatal`](Severity::Fatal), as Git refuses to demote them.
    pub fn set_severity(&mut self, id: MessageId, severity: Severity) -> Result<&mut Self> {
        if refuses_demotion(id, severity) {
            bail!(message!("Cannot demote {} to {}", id, severity.name()).validation());
        }
        self.severities[id as usize] = Some(severity);
        Ok(self)
    }

    /// Apply the configuration `fsck.<name> = <value>` as Git does, where `name` names a check and `value` is
    /// `error`, `warn` or `ignore`.
    ///
    /// For [`LargePathname`](MessageId::LargePathname), `value` may be followed by a colon and the longest name in
    /// bytes that tree entries may have, as in `warn:255`, with an optional `k`, `m` or `g` unit. It sets
    /// [`max_path_component_len()`](Self::max_path_component_len()).
    ///
    /// Nothing changes if an error is returned. Git's `git fsck` stops at a `name` it doesn't know, while fetches and
    /// pushes warn and skip it, which callers can do with [`MessageId::from_name()`]. Error messages name the check
    /// in lower case, as Git reads configuration keys.
    pub fn set_severity_from_config(&mut self, name: &BStr, value: &BStr) -> Result<&mut Self> {
        let name = name.to_ascii_lowercase();
        let name = name.as_bstr();
        let Some(id) = MessageId::from_name(name) else {
            bail!(message!("Unhandled message id: {name}").validation());
        };
        let (value, limit) = match value.find_byte(b':').filter(|_| id == MessageId::LargePathname) {
            Some(colon) => {
                let limit = value[colon + 1..].as_bstr();
                let Some(limit) = parse_signed_with_unit(limit) else {
                    bail!(message!("unable to parse max tree entry len: {limit}").validation());
                };
                (value[..colon].as_bstr(), Some(limit))
            }
            None => (value, None),
        };
        let Some(severity) = Severity::from_config_value(value) else {
            bail!(message!("Unknown fsck message type: '{value}'").validation());
        };
        if refuses_demotion(id, severity) {
            bail!(message!("Cannot demote {name} to {value}").validation());
        }
        self.severities[id as usize] = Some(severity);
        if let Some(limit) = limit {
            self.max_path_component_len = limit;
        }
        Ok(self)
    }

    /// The longest name, in bytes, that tree entries may have without a [`LargePathname`](MessageId::LargePathname)
    /// finding.
    pub fn max_path_component_len(&self) -> i64 {
        self.max_path_component_len
    }

    /// Set the longest name, in bytes, that tree entries may have without a
    /// [`LargePathname`](MessageId::LargePathname) finding. A negative length reports every entry.
    pub fn set_max_path_component_len(&mut self, len: i64) -> &mut Self {
        self.max_path_component_len = len;
        self
    }

    /// Report nothing about the object `id`, as Git's `fsck.skipList` does.
    ///
    /// The checks of trees that are skipped still note their `.gitmodules` and `.gitattributes` blobs, which are
    /// checked unless they are skipped themselves.
    pub fn skip(&mut self, id: ObjectId) -> &mut Self {
        self.skip.insert(id);
        self
    }

    /// Skip the objects that `list` names, in the format of Git's `fsck.skipList` file: one object ID of
    /// `object_hash` per line, with empty lines, surrounding white space and comments from `#` to the end of a
    /// line allowed.
    pub fn add_skip_list(&mut self, list: &[u8], object_hash: gix_hash::Kind) -> Result<&mut Self> {
        let mut ids = Vec::new();
        for line in list.lines() {
            // Git reads each line as a C string, so a NUL byte hides what follows it from the search for `#`.
            let line = match line.find_byte(b'#') {
                Some(comment) if !line[..comment].contains(&0) => &line[..comment],
                _ => line,
            };
            let line = trim_git_space(line);
            if line.is_empty() {
                continue;
            }
            let hex_len = object_hash.len_in_hex();
            let id = parse_hex_oid(line, object_hash).filter(|_| line.get(hex_len).is_none_or(|&next| next == 0));
            let Some(id) = id else {
                bail!(message!("invalid object name: {}", until_nul(line).as_bstr()).validation());
            };
            ids.push(id);
        }
        self.skip.extend(ids);
        Ok(self)
    }

    /// Whether nothing is reported about the object `id`.
    pub fn is_skipped(&self, id: &oid) -> bool {
        self.skip.contains(id)
    }
}

/// Parse `value` as Git's `git_parse_ssize_t()` does: a decimal, octal (with a leading `0`) or hexadecimal (with a
/// leading `0x`) integer, which may be preceded by white space and a sign and followed by `k`, `m` or `g`, and which
/// has to fit into an `isize`, Git's `ssize_t`, once multiplied by its unit.
fn parse_signed_with_unit(value: &BStr) -> Option<i64> {
    let bytes = value.as_bytes();
    let mut pos = bytes
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .unwrap_or(bytes.len());
    let negative = match bytes.get(pos) {
        Some(b'-') => {
            pos += 1;
            true
        }
        Some(b'+') => {
            pos += 1;
            false
        }
        _ => false,
    };
    let radix = match bytes.get(pos..pos + 2) {
        Some([b'0', b'x' | b'X']) if bytes.get(pos + 2).is_some_and(u8::is_ascii_hexdigit) => {
            pos += 2;
            16
        }
        _ if bytes.get(pos) == Some(&b'0') => 8,
        _ => 10,
    };
    let digits = bytes[pos..]
        .iter()
        .take_while(|&&b| (b as char).is_digit(radix))
        .count();
    if digits == 0 {
        return None;
    }
    let magnitude = bytes[pos..pos + digits].iter().try_fold(0i128, |value, &b| {
        let digit = i128::from((b as char).to_digit(radix)?);
        let value = value * i128::from(radix) + digit;
        (value <= i128::from(i64::MAX) + 1).then_some(value)
    })?;
    let value = if negative { -magnitude } else { magnitude };
    let value = i64::try_from(value).ok()?;
    let factor: i64 = match bytes[pos + digits..].to_ascii_lowercase().as_slice() {
        b"" => 1,
        b"k" => 1024,
        b"m" => 1024 * 1024,
        b"g" => 1024 * 1024 * 1024,
        _ => return None,
    };
    value
        .checked_mul(factor)
        .filter(|value| isize::try_from(*value).is_ok())
}

/// Whether Git refuses to set `id` to `severity`: checks whose default severity is [`Fatal`](Severity::Fatal) can't be
/// set to anything but an error.
fn refuses_demotion(id: MessageId, severity: Severity) -> bool {
    id.default_severity() == Severity::Fatal && !matches!(severity, Severity::Error | Severity::Fatal)
}

/// `bytes` without the leading and trailing bytes that Git counts as white space.
fn trim_git_space(bytes: &[u8]) -> &[u8] {
    let is_space = |b: &u8| matches!(b, b'\t' | b'\n' | b'\r' | b' ');
    let start = bytes.iter().position(|b| !is_space(b)).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !is_space(b)).map_or(start, |last| last + 1);
    &bytes[start..end]
}
