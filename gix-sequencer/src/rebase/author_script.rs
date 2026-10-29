//! The author of the commit that a rebase works on, in `author-script`.

use bstr::{BStr, BString, ByteSlice};
use gix_error::{Result, bail, message};

/// The author of the commit that a rebase works on, which `author-script` holds for the commit that Git makes from it,
/// with the variables `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` and `GIT_AUTHOR_DATE`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthorScript {
    /// The name of the author.
    pub name: BString,
    /// The email address of the author, without angle brackets.
    pub email: BString,
    /// The date of the commit as Git passes it on: `@`, the seconds since the epoch and the offset from UTC, like
    /// `@946684800 +0000`.
    pub date: BString,
}

impl AuthorScript {
    /// Take the author from the value of the `author` header of a commit, like `A U Thor <author@example.com> 1112912053
    /// -0700`, as `write_author_script()` does: the name ends before ` <`, the email address before `> `, and the date is
    /// the rest of the line.
    pub fn from_author_header(author: &BStr) -> Self {
        let line = author
            .find_byteset(b"\r\n")
            .map_or(author.as_bytes(), |end| &author[..end]);
        let (name, rest) = split_at_separator(line, b" <");
        let (email, date) = split_at_separator(rest, b"> ");
        let mut at_date = BString::from("@");
        at_date.extend_from_slice(date);
        AuthorScript {
            name: name.into(),
            email: email.into(),
            date: at_date,
        }
    }

    /// Parse `content`, the content of `author-script`, as `read_author_script()` does: each line sets one of the three
    /// variables to a value in single quotes, as a shell would read it.
    pub fn from_bytes(content: &[u8]) -> Result<Self> {
        let (mut name, mut email, mut date) = (None, None, None);
        for line in content.lines() {
            let Some(equals) = line.find_byte(b'=') else {
                bail!(message!("no key present in '{}'", line.as_bstr()).corrupted());
            };
            let (key, quoted) = (&line[..equals], &line[equals + 1..]);
            let Some(value) = sq_dequote(quoted) else {
                bail!(message!("unable to dequote value of '{}'", key.as_bstr()).corrupted());
            };
            let slot = match key {
                b"GIT_AUTHOR_NAME" => &mut name,
                b"GIT_AUTHOR_EMAIL" => &mut email,
                b"GIT_AUTHOR_DATE" => &mut date,
                _ => bail!(message!("unknown variable '{}'", key.as_bstr()).corrupted()),
            };
            if slot.replace(value).is_some() {
                bail!(message!("'{}' already given", key.as_bstr()).corrupted());
            }
        }
        Ok(AuthorScript {
            name: name.ok_or_else(|| message("missing 'GIT_AUTHOR_NAME'").corrupted())?,
            email: email.ok_or_else(|| message("missing 'GIT_AUTHOR_EMAIL'").corrupted())?,
            date: date.ok_or_else(|| message("missing 'GIT_AUTHOR_DATE'").corrupted())?,
        })
    }

    /// Return the content of `author-script` for this author, as `write_author_script()` writes it: a line for each
    /// variable, with its value in single quotes, and each single quote of a value written as `'\''`.
    pub fn to_bytes(&self) -> BString {
        let mut out = BString::default();
        for (key, value) in [
            ("GIT_AUTHOR_NAME", &self.name),
            ("GIT_AUTHOR_EMAIL", &self.email),
            ("GIT_AUTHOR_DATE", &self.date),
        ] {
            out.extend_from_slice(key.as_bytes());
            out.extend_from_slice(b"='");
            for byte in value.iter() {
                if *byte == b'\'' {
                    out.extend_from_slice(b"'\\''");
                } else {
                    out.push(*byte);
                }
            }
            out.extend_from_slice(b"'\n");
        }
        out
    }
}

/// Split `line` at the first `separator`, or return all of it before an empty rest if there is none.
fn split_at_separator<'a>(line: &'a [u8], separator: &[u8]) -> (&'a [u8], &'a [u8]) {
    match line.find(separator) {
        Some(start) => (&line[..start], &line[start + separator.len()..]),
        None => (line, &[]),
    }
}

/// Return the value of `quoted`, a value in single quotes in which a single quote or an exclamation mark may follow
/// the closed quote after a backslash before the quote opens again, as `sq_dequote()` reads it, or `None` if it isn't
/// quoted this way.
fn sq_dequote(quoted: &[u8]) -> Option<BString> {
    if quoted.first() != Some(&b'\'') {
        return None;
    }
    let mut value = BString::default();
    let mut index = 0;
    loop {
        index += 1;
        let byte = *quoted.get(index)?;
        if byte != b'\'' {
            value.push(byte);
            continue;
        }
        index += 1;
        match quoted.get(index) {
            None => return Some(value),
            Some(b'\\') => match (quoted.get(index + 1), quoted.get(index + 2)) {
                (Some(&escaped @ (b'\'' | b'!')), Some(b'\'')) => {
                    value.push(escaped);
                    index += 2;
                }
                _ => return None,
            },
            Some(_) => return None,
        }
    }
}
