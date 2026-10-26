//! Git's check of submodule URLs in `.gitmodules`, `check_submodule_url()`, with the URL normalization and decoding
//! it relies on.
//!
//! URLs are read as Git reads them, as text that ends at a NUL byte, so reading past the end of a URL yields NUL.

const UPPERCASE_HEX: &[u8; 16] = b"0123456789ABCDEF";

/// The characters that a percent-escape keeps escaped in a normalized URL, as they delimit its parts.
const URL_RESERVED: &[u8] = b":/?#[]@!$&'()*+,;=";

/// Whether Git allows `url` as the URL of a submodule.
///
/// It refuses URLs that could be taken for a command-line option, relative URLs and `git://` URLs that decode to a
/// newline or reach above their root to a host or scheme with `..`, and URLs that Git's HTTP and FTP transport would
/// receive and that don't normalize or decode to a newline.
pub(super) fn is_allowed_submodule_url(url: &[u8]) -> bool {
    if url.first() == Some(&b'-') {
        return false;
    }
    if starts_with_dot_slash(url) || starts_with_dot_dot_slash(url) || url.starts_with(b"git://") {
        if decode(url).contains(&b'\n') {
            return false;
        }
        let mut rest = url;
        let mut dot_dots = 0;
        loop {
            if starts_with_dot_dot_slash(rest) {
                dot_dots += 1;
                rest = &rest[3..];
            } else if starts_with_dot_slash(rest) {
                rest = &rest[2..];
            } else {
                break;
            }
        }
        return !(dot_dots > 0 && matches!(rest.first(), Some(b':' | b'/')));
    }
    let curl_url = [b"http::".as_slice(), b"https::", b"ftp::", b"ftps::"]
        .iter()
        .find_map(|prefix| url.strip_prefix(*prefix))
        .or_else(|| {
            [b"http://".as_slice(), b"https://", b"ftp://", b"ftps://"]
                .iter()
                .any(|prefix| url.starts_with(prefix))
                .then_some(url)
        });
    match curl_url {
        Some(curl_url) => normalize(curl_url).is_some_and(|normalized| !decode(&normalized).contains(&b'\n')),
        None => true,
    }
}

fn starts_with_dot_slash(path: &[u8]) -> bool {
    matches!(path, [b'.', b'/' | b'\\', ..])
}

fn starts_with_dot_dot_slash(path: &[u8]) -> bool {
    matches!(path, [b'.', b'.', b'/' | b'\\', ..])
}

/// `url` with percent-escapes decoded after its scheme, as Git's `url_decode()` does: escapes that are invalid or
/// encode NUL stay as they are.
fn decode(url: &[u8]) -> Vec<u8> {
    let (scheme, rest) = match url.iter().position(|&b| b == b':') {
        Some(colon) if colon > 0 => url.split_at(colon),
        _ => (&[][..], url),
    };
    let mut out = scheme.to_vec();
    let mut i = 0;
    while i < rest.len() {
        let c = rest[i];
        if c == b'%'
            && let Some(value) = rest.get(i + 1..i + 3).and_then(|hex| hex_byte(hex[0], hex[1]))
            && value > 0
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The byte the hexadecimal digits `high` and `low` encode, in either case.
fn hex_byte(high: u8, low: u8) -> Option<u8> {
    let digit = |c: u8| (c as char).to_digit(16);
    Some((digit(high)? << 4 | digit(low)?) as u8)
}

/// `url` normalized as Git's `url_normalize()` does, or `None` if Git considers it invalid: a lowercase scheme
/// followed by `://`, user information with normalized escapes, a lowercase host without escapes that only `file:`
/// URLs may omit, a port in the range 1 to 65535 without leading zeros, which is left out if it is the default port of
/// HTTP or HTTPS, and a path whose `.` and `..` segments are resolved, followed by the query and fragment.
fn normalize(url: &[u8]) -> Option<Vec<u8>> {
    let at = |i: usize| url.get(i).copied().unwrap_or(0);
    let is_scheme_char = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-');
    let scheme_len = url.iter().take_while(|c| is_scheme_char(c)).count();
    if scheme_len == 0 || !at(0).is_ascii_alphabetic() || !url[scheme_len..].starts_with(b"://") {
        return None;
    }
    let mut norm: Vec<u8> = url[..scheme_len + 3].to_ascii_lowercase();
    let mut pos = scheme_len + 3;

    let path_delimiter = |from: usize| {
        url[from..]
            .iter()
            .position(|c| matches!(c, b'/' | b'?' | b'#'))
            .map_or(url.len(), |offset| from + offset)
    };
    let slash = path_delimiter(pos);
    if let Some(user_end) = url[pos..].iter().position(|&c| c == b'@').map(|offset| pos + offset)
        && user_end < slash
    {
        if !append_normalized_escapes(&mut norm, &url[pos..user_end]) {
            return None;
        }
        norm.push(b'@');
        pos = user_end + 1;
    }

    let has_host = pos < url.len() && !matches!(at(pos), b':' | b'/' | b'?' | b'#');
    if !has_host && !norm.starts_with(b"file:") {
        return None;
    }
    // The last `:` before the path that doesn't belong to an IPv6 literal begins the port.
    let mut colon = slash - 1;
    while colon > pos && !matches!(at(colon), b':' | b']') {
        colon -= 1;
    }
    if at(colon) != b':' {
        colon = slash;
    } else if !has_host && colon < slash && colon + 1 != slash {
        return None;
    }
    let is_host_char = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b'[' | b':' | b']');
    if url[pos..].iter().take_while(|c| is_host_char(c)).count() < colon - pos {
        return None;
    }
    norm.extend(url[pos..colon].iter().map(u8::to_ascii_lowercase));
    pos = colon;

    if colon < slash {
        pos += 1;
        pos += url[pos..slash].iter().take_while(|&&c| c == b'0').count();
        if pos == slash && url[pos - 1] == b'0' {
            pos -= 1;
        }
        let port = &url[pos..slash];
        let is_default = port.is_empty()
            || (port == b"80" && norm.starts_with(b"http:"))
            || (port == b"443" && norm.starts_with(b"https:"));
        if !is_default {
            if !port.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let number = if port.len() <= 5 {
                port.iter()
                    .fold(0u32, |number, digit| number * 10 + u32::from(digit - b'0'))
            } else {
                0
            };
            if number == 0 || number > 65535 {
                return None;
            }
            norm.push(b':');
            norm.extend_from_slice(port);
        }
        pos = slash;
    }

    let path_start = norm.len();
    norm.push(b'/');
    if at(pos) == b'/' {
        pos += 1;
    }
    loop {
        let segment_start = norm.len();
        let next = path_delimiter(pos);
        let mut skip_slash = false;
        if !append_normalized_escapes(&mut norm, &url[pos..next]) {
            return None;
        }
        match &norm[segment_start..] {
            b"." => {
                if segment_start == path_start + 1 {
                    norm.truncate(norm.len() - 1);
                    skip_slash = true;
                } else {
                    norm.truncate(norm.len() - 2);
                }
            }
            b".." => {
                let mut previous_slash = norm.len() - 3;
                if previous_slash == path_start {
                    return None;
                }
                previous_slash -= 1;
                while norm[previous_slash] != b'/' {
                    previous_slash -= 1;
                }
                if previous_slash == path_start {
                    norm.truncate(previous_slash + 1);
                    skip_slash = true;
                } else {
                    norm.truncate(previous_slash);
                }
            }
            _ => {}
        }
        pos = next;
        if at(pos) != b'/' {
            break;
        }
        pos += 1;
        if !skip_slash {
            norm.push(b'/');
        }
    }

    if !append_normalized_escapes(&mut norm, &url[pos..]) {
        return None;
    }
    Some(norm)
}

/// Append `from` to `out`, decoding percent-escapes that don't need to be escaped and escaping the characters that
/// need to be, which are control characters, non-ASCII bytes, ` <>"%{}|\^` and the backtick, and those of
/// [`URL_RESERVED`] that were escaped. Escapes are written in uppercase. Return `false` at an invalid escape, as Git's
/// `append_normalized_escapes()` does.
fn append_normalized_escapes(out: &mut Vec<u8>, from: &[u8]) -> bool {
    let mut i = 0;
    while i < from.len() {
        let mut c = from[i];
        i += 1;
        let mut was_escaped = false;
        if c == b'%' {
            let Some(value) = from.get(i..i + 2).and_then(|hex| hex_byte(hex[0], hex[1])) else {
                return false;
            };
            c = value;
            i += 2;
            was_escaped = true;
        }
        if c <= 0x1f || c >= 0x7f || b" <>\"%{}|\\^`".contains(&c) || (was_escaped && URL_RESERVED.contains(&c)) {
            out.extend_from_slice(&[
                b'%',
                UPPERCASE_HEX[usize::from(c >> 4)],
                UPPERCASE_HEX[usize::from(c & 0xf)],
            ]);
        } else {
            out.push(c);
        }
    }
    true
}
