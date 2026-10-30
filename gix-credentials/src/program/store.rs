//! Git's `credential-store`, which runs in this process as `cmd_credential_store()` in `builtin/credential-store.c` runs,
//! with the same files, lines and locks.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use bstr::{BString, ByteSlice};
use gix_error::{OptionExt, Result, ResultExt, bail, message};

/// How long to wait for the lock of a credentials file, the default of Git's `credentialStore.lockTimeoutMS`.
const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

/// The arguments of Git's `credential-store` in front of its operation, as `cmd_credential_store()` parses them.
#[derive(Debug)]
pub(crate) struct Options {
    /// The file that `--file=<path>` names, or `None` to use the default files.
    file: Option<PathBuf>,
}

impl Options {
    /// Parse `words`, the arguments configured for `store` as the shell splits them, as `parse_options()` parses them
    /// in front of the operation that Git appends.
    ///
    /// Return `None` for arguments other than `--file=<path>`, `--file <path>` and `--no-file`, which are left to
    /// `git credential-store` to parse.
    pub(crate) fn parse(words: &[BString]) -> Option<Self> {
        let mut file = None;
        let mut words = words.iter();
        while let Some(word) = words.next() {
            if word == "--no-file" {
                file = None;
                continue;
            }
            let value = match word.strip_prefix(b"--file=") {
                Some(value) => value,
                // Without a value of its own, `--file` takes the operation as its value.
                None if word == "--file" => words.next()?.as_slice(),
                None => return None,
            };
            if value.is_empty() {
                return None;
            }
            file = Some(gix_path::from_byte_slice(value).ok()?.to_owned());
        }
        Some(Options { file })
    }
}

/// Perform `operation` with `options` on the credential that `input`, the request a helper reads, describes, and return
/// what Git's `credential-store` writes to its standard output, as `cmd_credential_store()` does.
pub(crate) fn run(options: &Options, operation: &str, input: &[u8]) -> Result<Vec<u8>> {
    let files = match &options.file {
        Some(file) => vec![file.clone()],
        None => default_files(),
    };
    if files.is_empty() {
        bail!(message("unable to set up default path; use --file"));
    }
    let credential = Credential::read(input)?;
    let mut output = Vec::new();
    match operation {
        "get" => lookup(&files, &credential, &mut output)?,
        "erase" => remove(&files, &credential)?,
        "store" => store(&files, &credential)?,
        // Git ignores unknown operations.
        _ => {}
    }
    Ok(output)
}

/// Return the files that Git's `credential-store` uses without `--file`, in its order: `~/.git-credentials`, then
/// `$XDG_CONFIG_HOME/git/credentials`, or `~/.config/git/credentials` if `XDG_CONFIG_HOME` is unset or empty.
fn default_files() -> Vec<PathBuf> {
    let home = gix_path::env::home_dir();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    home.map(|home| home.join(".git-credentials"))
        .into_iter()
        .chain(config_home.map(|dir| dir.join("git").join("credentials")))
        .collect()
}

/// The fields of a credential that Git's `credential-store` uses, each `None` if it's unset, as in Git's
/// `struct credential`.
#[derive(Debug, Default)]
struct Credential {
    protocol: Option<BString>,
    host: Option<BString>,
    path: Option<BString>,
    username: Option<BString>,
    password: Option<BString>,
    /// A pre-encoded credential, which no entry of a credentials file has.
    credential: Option<BString>,
}

impl Credential {
    /// Read the credential that `input` describes, as `credential_read()` reads the request of a helper.
    fn read(input: &[u8]) -> Result<Self> {
        let mut credential = Credential::default();
        for line in input.lines_with_terminator() {
            let line = line
                .strip_suffix(b"\n")
                .map_or(line, |line| line.strip_suffix(b"\r").unwrap_or(line));
            if line.is_empty() {
                break;
            }
            let Some((key, raw_value)) = until_nul(line).split_once_str("=") else {
                bail!(message("unable to read credential: a line isn't a key=value pair"));
            };
            let value = Some(BString::from(raw_value));
            match key {
                b"protocol" => credential.protocol = value,
                b"host" => credential.host = value,
                b"path" => credential.path = value,
                b"username" => credential.username = value,
                b"password" => credential.password = value,
                b"credential" => credential.credential = value,
                // Like Git, the parts of the URL replace everything read before it.
                b"url" => {
                    credential = Credential::from_url(raw_value)
                        .ok_or_raise(|| message("unable to read credential: its url can't be parsed"))?;
                }
                _ => {}
            }
        }
        Ok(credential)
    }

    /// Parse `url` into its parts, as `credential_from_url_gently()` parses a complete URL, or return `None` if it has
    /// no protocol or a part with a newline.
    fn from_url(url: &[u8]) -> Option<Self> {
        let protocol_end = url.find(b"://").filter(|end| *end > 0)?;
        let rest = &url[protocol_end + 3..];
        let slash = rest.find_byteset(b"/?#").unwrap_or(rest.len());
        let mut credential = Credential {
            protocol: Some(url[..protocol_end].into()),
            ..Default::default()
        };
        let host_start = match rest.find_byte(b'@') {
            Some(at) if at < slash => {
                match rest.find_byte(b':') {
                    Some(colon) if colon < at => {
                        credential.username = Some(gix_url::decode(&rest[..colon]));
                        credential.password = Some(gix_url::decode(&rest[colon + 1..at]));
                    }
                    _ => credential.username = Some(gix_url::decode(&rest[..at])),
                }
                at + 1
            }
            _ => 0,
        };
        credential.host = Some(gix_url::decode(&rest[host_start..slash]));
        let mut path = &rest[slash..];
        while let [b'/', tail @ ..] = path {
            path = tail;
        }
        if !path.is_empty() {
            let mut path = gix_url::decode(path);
            while path.len() > 1 && path.ends_with(b"/") {
                path.pop();
            }
            credential.path = Some(path);
        }
        let parts = [
            &credential.protocol,
            &credential.host,
            &credential.path,
            &credential.username,
            &credential.password,
        ];
        parts
            .iter()
            .all(|part| part.as_ref().is_none_or(|part| !part.contains(&b'\n')))
            .then_some(credential)
    }

    /// Return `true` if `have` has each field that this credential has, with the same value, as `credential_match()`
    /// decides, comparing the password and the pre-encoded credential only if `match_password` is `true`.
    fn matches(&self, have: &Credential, match_password: bool) -> bool {
        fn check(want: Option<&BString>, have: Option<&BString>) -> bool {
            want.is_none_or(|want| have == Some(want))
        }
        check(self.protocol.as_ref(), have.protocol.as_ref())
            && check(self.host.as_ref(), have.host.as_ref())
            && check(self.path.as_ref(), have.path.as_ref())
            && check(self.username.as_ref(), have.username.as_ref())
            && (!match_password
                || (check(self.password.as_ref(), have.password.as_ref())
                    && check(self.credential.as_ref(), have.credential.as_ref())))
    }
}

/// Parse `line` of a credentials file into the credential it stores if it's a complete URL with a username and a
/// password, as `parse_credential_file()` accepts its lines.
fn entry(line: &[u8]) -> Option<Credential> {
    Credential::from_url(until_nul(line)).filter(|entry| entry.username.is_some() && entry.password.is_some())
}

/// Return the lines of `content` as `strbuf_getline_lf()` reads them, without their line feed.
fn lines(content: &[u8]) -> impl Iterator<Item = &[u8]> {
    content
        .lines_with_terminator()
        .map(|line| line.strip_suffix(b"\n").unwrap_or(line))
}

/// Read the credentials file at `path`, or return `None` if it doesn't exist or can't be accessed, as
/// `parse_credential_file()` skips such files. As there, a failure to read it ends its content.
fn read_file(path: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::ErrorKind::{NotFound, PermissionDenied};
    match std::fs::File::open(path) {
        Ok(mut file) => {
            let mut content = Vec::new();
            file.read_to_end(&mut content).ok();
            Ok(Some(content))
        }
        Err(err) if matches!(err.kind(), NotFound | PermissionDenied) => Ok(None),
        Err(err) => Err(err).or_raise(|| message!("unable to open {}", path.display())),
    }
}

/// Write the username and password of the first entry matching `credential` in the first of `files` that has one to
/// `output`, as `lookup_credential()` does.
fn lookup(files: &[PathBuf], credential: &Credential, output: &mut Vec<u8>) -> Result {
    for file in files {
        let Some(content) = read_file(file)? else {
            continue;
        };
        if let Some(Credential {
            username: Some(username),
            password: Some(password),
            ..
        }) = lines(&content)
            .filter_map(entry)
            .find(|entry| credential.matches(entry, false))
        {
            output.extend_from_slice(b"username=");
            output.extend_from_slice(&username);
            output.extend_from_slice(b"\npassword=");
            output.extend_from_slice(&password);
            output.push(b'\n');
            return Ok(());
        }
    }
    Ok(())
}

/// Store `credential` in the first of `files` that exists, or else in the first file, as `store_credential()` does,
/// unless it lacks the protocol, both host and path, the username or the password.
fn store(files: &[PathBuf], credential: &Credential) -> Result {
    let (Some(protocol), Some(username), Some(password)) =
        (&credential.protocol, &credential.username, &credential.password)
    else {
        return Ok(());
    };
    if credential.host.is_none() && credential.path.is_none() {
        return Ok(());
    }
    let mut line = protocol.clone();
    line.extend_from_slice(b"://");
    url_encode(&mut line, username, is_unreserved);
    line.push(b':');
    url_encode(&mut line, password, is_unreserved);
    line.push(b'@');
    if let Some(host) = &credential.host {
        url_encode(&mut line, host, is_unreserved);
    }
    if let Some(path) = &credential.path {
        line.push(b'/');
        url_encode(&mut line, path, is_reserved_or_unreserved);
    }
    match files.iter().find(|file| file.exists()).or_else(|| files.first()) {
        Some(file) => rewrite(file, credential, Some(&line), false),
        None => Ok(()),
    }
}

/// Remove the entries matching `credential` from each of `files` that exists, as `remove_credential()` does, unless
/// it has neither protocol, host, path nor username.
fn remove(files: &[PathBuf], credential: &Credential) -> Result {
    let Credential {
        protocol,
        host,
        path,
        username,
        ..
    } = credential;
    if protocol.is_none() && host.is_none() && path.is_none() && username.is_none() {
        return Ok(());
    }
    for file in files.iter().filter(|file| file.exists()) {
        rewrite(file, credential, None, true)?;
    }
    Ok(())
}

/// Replace `file` with `first_line`, if given, followed by its lines that aren't entries matching `credential`, as
/// `rewrite_credential_file()` does.
///
/// Like Git, this writes a lock file next to the file, or next to the target if the file is a symbolic link, and
/// renames it to the file. Only its owner may read and write it, as Git's `credential-store` sets its umask to `077`.
fn rewrite(file: &Path, credential: &Credential, first_line: Option<&[u8]>, match_password: bool) -> Result {
    let mut lock = gix_lock::File::acquire(
        file,
        gix_lock::acquire::Fail::AfterDurationWithBackoff(LOCK_TIMEOUT),
        None,
        -0o600,
        Some(&gix_lock::acquire::resolve_symlink),
    )
    .or_raise(|| {
        message!(
            "unable to get credential storage lock in {} ms",
            LOCK_TIMEOUT.as_millis()
        )
    })?;
    let write_error = || message("unable to write credential store");
    if let Some(line) = first_line {
        lock.write_all(line)
            .and_then(|()| lock.write_all(b"\n"))
            .or_raise(write_error)?;
    }
    if let Some(content) = read_file(file)? {
        for line in
            lines(&content).filter(|line| entry(line).is_none_or(|entry| !credential.matches(&entry, match_password)))
        {
            lock.write_all(line)
                .and_then(|()| lock.write_all(b"\n"))
                .or_raise(write_error)?;
        }
    }
    lock.commit().map_err(|err| err.error).or_raise(write_error)?;
    Ok(())
}

/// Append `value` to `out`, writing each byte that `is_allowed` rejects as `%xx`, as `strbuf_add_urlencode()` does.
fn url_encode(out: &mut BString, value: &[u8], is_allowed: fn(u8) -> bool) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in value {
        if is_allowed(byte) {
            out.push(byte);
        } else {
            out.extend_from_slice(&[b'%', HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
        }
    }
}

/// Return `true` for the bytes that RFC 3986 calls unreserved, as Git's `is_rfc3986_unreserved()` does.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// Return `true` for the bytes that RFC 3986 calls reserved or unreserved, as Git's
/// `is_rfc3986_reserved_or_unreserved()` does.
fn is_reserved_or_unreserved(byte: u8) -> bool {
    is_unreserved(byte)
        || matches!(
            byte,
            b'!' | b'*'
                | b'\''
                | b'('
                | b')'
                | b';'
                | b':'
                | b'@'
                | b'&'
                | b'='
                | b'+'
                | b'$'
                | b','
                | b'/'
                | b'?'
                | b'#'
                | b'['
                | b']'
        )
}

/// Return `bytes` up to their first NUL, as Git ends them there as C strings.
fn until_nul(bytes: &[u8]) -> &[u8] {
    bytes.find_byte(0).map_or(bytes, |nul| &bytes[..nul])
}
