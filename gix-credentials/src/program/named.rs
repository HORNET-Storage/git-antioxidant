//! Helpers named in the configuration, like `manager`, which Git runs as `git credential-<name>` through the shell.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;

use crate::program::store;

/// How a helper named in the configuration runs.
pub(crate) enum Resolution {
    /// As Git's `credential-store`, in this process.
    Store(store::Options),
    /// As the program that implements it, prepared for the operation with the arguments configured for it.
    Program(gix_command::Prepare),
    /// As `git credential-<name>`, which needs `git`.
    Git,
}

/// Decide how `name_and_args`, a helper named in the configuration with its arguments, runs.
///
/// Git runs it as `git credential-<name> <args>` through the shell (`credential_do()` in `credential.c`), and `git` runs
/// its builtin `credential-store` or `credential-cache`, or else the program `git-credential-<name>` with the arguments
/// (`execv_dashed_external()` in `git.c`), found in its exec path and then in `PATH`. Here `store` runs in this process,
/// and the program runs where it's found without starting Git. The others run as `git credential-<name>`: `cache`,
/// which talks to Git's daemon, a name that the shell would change or that is a path, and a helper whose program
/// isn't found, as Git may find it in its exec path.
///
/// `operation` is what the helper is asked to do, which Git adds to the text of the command.
pub(crate) fn resolve(name_and_args: &BStr, operation: &str) -> Result<Resolution> {
    // The name ends where the shell ends the word, and a newline, which would end the command, leaves it to Git.
    let (name, args) = match name_and_args.find_byteset(b" \t") {
        Some(end) => (&name_and_args.as_bytes()[..end], name_and_args[end + 1..].as_bstr()),
        None => (name_and_args.as_bytes(), b"".as_bstr()),
    };
    let is_plain = |byte: &u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+');
    if name.is_empty() || !name.iter().all(is_plain) {
        return Ok(Resolution::Git);
    }
    match name {
        b"store" => {
            return Ok(store_words(args)
                .and_then(|words| store::Options::parse(&words))
                .map_or(Resolution::Git, Resolution::Store));
        }
        b"cache" | b"cache--daemon" => return Ok(Resolution::Git),
        _ => {}
    }
    let mut file_name = String::from("git-credential-");
    file_name.extend(name.iter().copied().map(char::from));
    Ok(match find(OsStr::new(&file_name), &search_path()) {
        Some(found) => Resolution::Program(found.into_command(args, operation)?),
        None => Resolution::Git,
    })
}

/// Split `args`, the arguments configured for `store`, into words as the shell splits them, or return `None` if they
/// need the shell for more than quotes, escapes and a `~` that starts a word, which then run as `git credential-store`.
fn store_words(args: &BStr) -> Option<Vec<BString>> {
    // A comment would also hide the operation that Git appends.
    if args.find_byteset(b"|&;<>()$`*?[{#\n").is_some() {
        return None;
    }
    let mut line = BString::from("credential-store ");
    line.extend_from_slice(&expand_tildes(args)?);
    let words = gix_command::parse::command_line(line.as_bstr()).ok()?.args;
    words
        .into_iter()
        .map(|word| gix_path::os_string_into_bstring(word).ok())
        .collect()
}

/// Replace each unquoted `~` that starts a word and is followed by `/` or ends it with the quoted home directory, as the
/// shell's tilde expansion does, or return `None` if such a `~` starts a longer name, like `~user`, or the home
/// directory is unknown.
fn expand_tildes(args: &BStr) -> Option<BString> {
    let mut out = BString::default();
    let mut quote = None;
    let mut starts_word = true;
    let mut bytes = args.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        match quote {
            Some(end) => {
                out.push(byte);
                if byte == end {
                    quote = None;
                } else if end == b'"'
                    && byte == b'\\'
                    && let Some(escaped) = bytes.next()
                {
                    out.push(escaped);
                }
            }
            None if byte == b'~' && starts_word => {
                if !matches!(bytes.peek(), None | Some(b' ' | b'\t' | b'/')) {
                    return None;
                }
                let home = gix_path::env::home_dir()?;
                out.extend_from_slice(&gix_quote::single(gix_path::into_bstr(home).ok()?.as_ref()));
                starts_word = false;
            }
            None => {
                starts_word = matches!(byte, b' ' | b'\t');
                out.push(byte);
                match byte {
                    b'\'' | b'"' => quote = Some(byte),
                    b'\\' => {
                        if let Some(escaped) = bytes.next() {
                            out.push(escaped);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Some(out)
}

/// Return the directories in which Git finds the program of a named helper, in its order, as far as they are known
/// without starting Git: `GIT_EXEC_PATH` if it's set, the [directories of Git's
/// programs](gix_path::env::git_program_dirs()), and those of `PATH`, as Git's `setup_path()` puts its exec path in
/// front of `PATH`. On platforms other than Windows, the exec path that Git was built with isn't among them.
fn search_path() -> Vec<PathBuf> {
    let exec_path = std::env::var_os("GIT_EXEC_PATH").filter(|dir| !dir.is_empty());
    let path = std::env::var_os("PATH");
    exec_path
        .map(PathBuf::from)
        .into_iter()
        .chain(gix_path::env::git_program_dirs().iter().cloned())
        .chain(path.iter().flat_map(std::env::split_paths))
        .collect()
}

/// The program of a named helper, to start as Git starts it.
struct Found {
    /// The program to start.
    program: PathBuf,
    /// The script that `program` interprets, as Git for Windows starts a script with the interpreter its `#!` line names.
    script: Option<PathBuf>,
}

impl Found {
    /// Prepare the command that starts the program for `operation` with `args`, the arguments configured for the helper,
    /// through the shell only if they need one.
    fn into_command(self, args: &BStr, operation: &str) -> Result<gix_command::Prepare> {
        let args = args.trim_start_with(|c| matches!(c, ' ' | '\t'));
        let script = self.script.as_deref().map(gix_command::mingw::script_argument);
        if args.is_empty() {
            return Ok(gix_command::prepare(self.program).args(script).arg(operation));
        }
        let mut command = quote(&self.program)?;
        if let Some(script) = &script {
            command.push(b' ');
            command.extend_from_slice(&quote(Path::new(script))?);
        }
        command.push(b' ');
        command.extend_from_slice(args);
        command.push(b' ');
        command.extend_from_slice(operation.as_bytes());
        Ok(
            gix_command::prepare(gix_path::from_bstr(command.as_bstr())?.into_owned())
                .command_may_be_shell_script_allow_manual_argument_splitting(),
        )
    }
}

/// Quote `path` as one word for the shell, with slashes on Windows, where the shell would read backslashes as escapes.
fn quote(path: &Path) -> Result<BString> {
    Ok(gix_quote::single(
        gix_path::to_unix_separators_on_windows(gix_path::into_bstr(path)?).as_ref(),
    ))
}

/// Find `file_name` in `search_path` as Git's `locate_in_PATH()` finds a program: the first regular file that its owner
/// may execute, where an empty entry stands for the current directory.
#[cfg(unix)]
fn find(file_name: &OsStr, search_path: &[PathBuf]) -> Option<Found> {
    use std::os::unix::fs::PermissionsExt;

    search_path
        .iter()
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                Path::new(".").join(file_name)
            } else {
                dir.join(file_name)
            }
        })
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o100 != 0)
        })
        .map(|program| Found { program, script: None })
}

/// Find `file_name` in `search_path` as Git for Windows' `mingw_spawnvpe()` finds the program to start, with the
/// interpreter that its `#!` line names if it's a script, or return `None` if Git for Windows couldn't start it.
#[cfg(not(unix))]
fn find(file_name: &OsStr, search_path: &[PathBuf]) -> Option<Found> {
    use gix_command::mingw;

    let program = mingw::path_lookup(file_name, search_path.iter().cloned(), false)?;
    match mingw::interpreter(&program) {
        Some(interpreter) => Some(Found {
            program: mingw::path_lookup(&interpreter, search_path.iter().cloned(), true)?,
            script: Some(program),
        }),
        None => mingw::is_exe(program.as_os_str()).then_some(Found { program, script: None }),
    }
}
