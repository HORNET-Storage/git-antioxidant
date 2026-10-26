//! Git's parser of configuration in memory, as `git_config_from_mem()` runs it on `.gitmodules` blobs.
//!
//! Git reads each byte as a `char` and passes it on as an `int`. Where `char` is signed, as on x86, x86-64 and the
//! Arm targets of Apple and Windows, bytes from `0x80` on are negative: `0xFF` reads as the end of the content, and a
//! UTF-8 byte order mark isn't skipped. [`c_char`] reproduces this on each target.

use std::ffi::c_char;

const EOF: i32 = -1;
const NL: i32 = b'\n' as i32;

/// Parse `content` as Git's `git_config_from_mem()` does, calling `entry` with the name and the value of each
/// variable in turn, and return `false` where Git's parser fails, after the variables before the failure.
///
/// A name holds the lowercase section, the subsection as written and the lowercase key, separated by periods. A
/// variable without `=` has no value.
pub(super) fn parse(content: &[u8], mut entry: impl FnMut(&[u8], Option<&[u8]>)) -> bool {
    const BOM: [u8; 3] = [0xef, 0xbb, 0xbf];
    let mut source = Source {
        content,
        pos: 0,
        eof: false,
        len: 0,
    };
    let mut comment = false;
    let mut section_len = 0;
    let mut name = Vec::new();
    // How many bytes of the byte order mark were skipped, until content other than the mark begins.
    let mut bom = Some(0);
    loop {
        let c = source.next_char();
        if let Some(skipped) = bom.filter(|&skipped| skipped < BOM.len()) {
            if c == i32::from(BOM[skipped]) {
                bom = Some(skipped + 1);
                continue;
            }
            if skipped != 0 {
                return false;
            }
            bom = None;
        }
        if c == NL {
            if source.eof {
                return true;
            }
            comment = false;
            continue;
        }
        if comment || is_space(c) {
            continue;
        }
        if c == i32::from(b'#') || c == i32::from(b';') {
            comment = true;
            continue;
        }
        if c == i32::from(b'[') {
            name.clear();
            if !section(&mut source, &mut name) || name.is_empty() {
                return false;
            }
            name.push(b'.');
            section_len = name.len();
            continue;
        }
        if !(c as u8).is_ascii_alphabetic() {
            return false;
        }
        name.truncate(section_len);
        name.push((c as u8).to_ascii_lowercase());
        if !variable(&mut source, &mut name, &mut entry) {
            return false;
        }
    }
}

/// The configuration being read, as Git's `struct config_source` holds it.
struct Source<'a> {
    content: &'a [u8],
    pos: usize,
    /// Whether the end was reached, which Git doesn't reset when it reads on past a `0xFF` byte.
    eof: bool,
    /// The number of characters returned, which Git limits to `INT_MAX`.
    len: usize,
}

impl Source<'_> {
    /// The next byte as Git's `config_buf_fgetc()` returns it, through a `char`, or [`EOF`].
    fn getc(&mut self) -> i32 {
        match self.content.get(self.pos) {
            Some(&byte) => {
                self.pos += 1;
                i32::from(byte as c_char)
            }
            None => EOF,
        }
    }

    /// The next character as Git's `get_next_char()` returns it: `\r\n` as `\n`, and the end of the content as `\n`
    /// after which [`Source::eof`] is set.
    fn next_char(&mut self) -> i32 {
        let mut c = self.getc();
        if c == i32::from(b'\r') {
            c = self.getc();
            if c != NL {
                if c != EOF {
                    self.pos -= 1;
                }
                c = i32::from(b'\r');
            }
        }
        if c != EOF {
            self.len += 1;
            if self.len > i32::MAX as usize {
                self.eof = true;
                return 0;
            }
        }
        if c == EOF {
            self.eof = true;
            c = NL;
        }
        c
    }
}

/// Read the section name after `[` into `name`, lowercase, and the subsection as written after a period, as Git's
/// `get_base_var()` does.
fn section(source: &mut Source<'_>, name: &mut Vec<u8>) -> bool {
    loop {
        let c = source.next_char();
        if source.eof {
            return false;
        }
        if c == i32::from(b']') {
            return true;
        }
        if is_space(c) {
            return subsection(source, name, c);
        }
        if !is_key_char(c) && c != i32::from(b'.') {
            return false;
        }
        name.push((c as u8).to_ascii_lowercase());
    }
}

/// Read the quoted subsection that follows the white space `c` into `name`, after a period, as Git's
/// `get_extended_base_var()` does.
fn subsection(source: &mut Source<'_>, name: &mut Vec<u8>, mut c: i32) -> bool {
    loop {
        if c == NL {
            return false;
        }
        c = source.next_char();
        if !is_space(c) {
            break;
        }
    }
    if c != i32::from(b'"') {
        return false;
    }
    name.push(b'.');
    loop {
        let mut c = source.next_char();
        if c == NL {
            return false;
        }
        if c == i32::from(b'"') {
            break;
        }
        if c == i32::from(b'\\') {
            c = source.next_char();
            if c == NL {
                return false;
            }
        }
        name.push(c as u8);
    }
    source.next_char() == i32::from(b']')
}

/// Read the rest of a variable's key into `name`, lowercase, and its value, and pass both to `entry`, as Git's
/// `get_value()` does.
fn variable(source: &mut Source<'_>, name: &mut Vec<u8>, entry: &mut impl FnMut(&[u8], Option<&[u8]>)) -> bool {
    let mut c;
    loop {
        c = source.next_char();
        if source.eof || !is_key_char(c) {
            break;
        }
        name.push((c as u8).to_ascii_lowercase());
    }
    while c == i32::from(b' ') || c == i32::from(b'\t') {
        c = source.next_char();
    }
    let mut value = None;
    if c != NL {
        if c != i32::from(b'=') {
            return false;
        }
        let Some(parsed) = parse_value(source) else {
            return false;
        };
        value = Some(parsed);
    }
    entry(name, value.as_deref());
    true
}

/// Read a value up to the end of its line as Git's `parse_value()` does: without surrounding white space and
/// comments, with quotes removed and escapes resolved, or `None` if it has an unknown escape or an unclosed quote.
fn parse_value(source: &mut Source<'_>) -> Option<Vec<u8>> {
    let mut quote = false;
    let mut comment = false;
    // The length of the value without the white space that followed it, or 0 if no white space did.
    let mut trim_len = 0;
    let mut value = Vec::new();
    loop {
        let mut c = source.next_char();
        if c == NL {
            if quote {
                return None;
            }
            if trim_len != 0 {
                value.truncate(trim_len);
            }
            return Some(value);
        }
        if comment {
            continue;
        }
        if is_space(c) && !quote {
            if trim_len == 0 {
                trim_len = value.len();
            }
            if !value.is_empty() {
                value.push(c as u8);
            }
            continue;
        }
        if !quote && (c == i32::from(b';') || c == i32::from(b'#')) {
            comment = true;
            continue;
        }
        trim_len = 0;
        if c == i32::from(b'\\') {
            c = source.next_char();
            c = match u8::try_from(c) {
                Ok(b'\n') => continue,
                Ok(b't') => i32::from(b'\t'),
                Ok(b'b') => 0x08,
                Ok(b'n') => NL,
                Ok(b'\\' | b'"') => c,
                _ => return None,
            };
            value.push(c as u8);
            continue;
        }
        if c == i32::from(b'"') {
            quote = !quote;
            continue;
        }
        value.push(c as u8);
    }
}

/// Whether Git counts `c` as white space, which it decides for the byte `c` was read from.
fn is_space(c: i32) -> bool {
    matches!(c as u8, b'\t' | b'\n' | b'\r' | b' ')
}

/// Whether `c` may be part of a key, as Git's `iskeychar()` decides.
fn is_key_char(c: i32) -> bool {
    (c as u8).is_ascii_alphanumeric() || c == i32::from(b'-')
}
