//! Launch commands very similarly to `Command`, but with `git` specific capabilities and adjustments.
//!
//! ## Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let output = gix_command::prepare("git")
//!     .arg("--version")
//!     .spawn()?
//!     .wait_with_output()?;
//!
//! assert!(output.status.success());
//! assert!(String::from_utf8(output.stdout)?.starts_with("git version "));
//! # Ok(()) }
//! ```
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use bstr::BString;

pub mod mingw;
///
pub mod parse;

mod prepare;

/// A structure to keep settings to use when invoking a command via [`spawn()`][Prepare::spawn()],
/// after creating it with [`prepare()`].
///
/// Convert it to [`std::process::Command`] with [`TryFrom::try_from()`] to finish preparation
/// without spawning. Preparation reports an error when shell quoting or namespace conversion
/// requires an encoding the platform cannot represent.
pub struct Prepare {
    /// The command to invoke, either directly or with a shell depending on `use_shell`.
    pub command: OsString,
    /// Additional information to be passed to the spawned command.
    pub context: Option<Context>,
    /// The way standard input is configured.
    pub stdin: std::process::Stdio,
    /// The way standard output is configured.
    pub stdout: std::process::Stdio,
    /// The way standard error is configured.
    pub stderr: std::process::Stdio,
    /// The arguments to pass to the process being spawned.
    pub args: Vec<OsString>,
    /// Environment variables to set for the spawned process.
    pub env: Vec<(OsString, OsString)>,
    /// If `true`, we will use `shell_program` or `sh` to execute the `command`.
    pub use_shell: bool,
    /// If `true`, `command` is assumed to be a command or path to the program to execute, and it
    /// will be shell-quoted to assure it will be executed as is and without splitting across
    /// whitespace.
    pub quote_command: bool,
    /// The name or path to the shell program to use instead of `sh`.
    pub shell_program: Option<OsString>,
    /// When to run `command` without a shell after splitting its arguments by hand, if that can be done safely.
    ///
    /// Only effective if `use_shell` is `true` as well. The shell is still used if the command-line contains
    /// 'scripting' that can't be split by hand.
    pub manual_argument_splitting: ManualArgumentSplitting,
}

/// When [`Prepare`] runs a command that may be a shell script without a shell, after splitting its arguments by hand.
///
/// A shell runs such commands as Git does, with shell builtins and expansions. Splitting by hand saves starting a
/// shell, but runs the program directly, so neither builtins nor expansions other than quoting are available.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ManualArgumentSplitting {
    /// Always use a shell.
    Never,
    /// Use the shell, and split arguments by hand only if the default shell can't be started, so commands without
    /// shell syntax still run.
    ///
    /// The default shell can only be missing on Windows without Git for Windows, if `PATH` lacks `sh.exe` as well.
    #[default]
    IfNoShell,
    /// Split arguments by hand whenever that can be done safely, even if a shell can be started.
    Always,
}

/// Additional information that is relevant to spawned processes, which typically receive
/// a wealth of contextual information when spawned from `git`.
///
/// Repository paths must be absolute, resolved against the caller's captured working directory.
/// Consumers may change the child process's working directory, so relative paths are not reliable.
///
/// See [the git source code](https://github.com/git/git/blob/cfb8a6e9a93adbe81efca66e6110c9b4d2e57169/git.c#L191)
/// for details.
#[derive(Debug, Default, Clone)]
pub struct Context {
    /// The absolute path to the `.git` directory that contains the repository.
    ///
    /// If set, it will be used to set the `GIT_DIR` environment variable.
    pub git_dir: Option<PathBuf>,
    /// Set the `GIT_WORK_TREE` environment variable with the given absolute path.
    pub worktree_dir: Option<PathBuf>,
    /// If `true`, set `GIT_NO_REPLACE_OBJECTS` to `1`, which turns off object replacements, or `0` otherwise.
    /// If `None`, the variable won't be set.
    pub no_replace_objects: Option<bool>,
    /// Set the `GIT_NAMESPACE` variable with the given value, effectively namespacing all
    /// operations on references.
    pub ref_namespace: Option<BString>,
    /// If `true`, set `GIT_LITERAL_PATHSPECS` to `1`, which makes globs literal and prefixes as well, or `0` otherwise.
    /// If `None`, the variable won't be set.
    pub literal_pathspecs: Option<bool>,
    /// If `true`, set `GIT_GLOB_PATHSPECS` to `1`, which lets wildcards not match the `/` character, and equals the `:(glob)` prefix.
    /// If `false`, set `GIT_NOGLOB_PATHSPECS` to `1` which lets globs match only themselves.
    /// If `None`, the variable won't be set.
    pub glob_pathspecs: Option<bool>,
    /// If `true`, set `GIT_ICASE_PATHSPECS` to `1`, to let patterns match case-insensitively, or `0` otherwise.
    /// If `None`, the variable won't be set.
    pub icase_pathspecs: Option<bool>,
    /// If `true`, inherit `stderr` just like it's the default when spawning processes.
    /// If `false`, suppress all stderr output.
    /// If not `None`, this will override any value set with [`Prepare::stderr()`].
    pub stderr: Option<bool>,
}

/// Split a joined `PATH` value according to platform conventions, omitting empty entries.
///
/// Git's Windows lookup skips empty entries instead of treating them as the current directory. This also prevents an
/// explicitly empty `PATH` from finding a command there.
fn split_paths(joined_paths: &OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(joined_paths).filter(|path| !path.as_os_str().is_empty())
}

/// Return whether `command` is a single path component eligible for `PATH` lookup.
fn is_bare_command(command: &Path) -> bool {
    command.components().take(2).count() == 1
}

/// Try to find `command` in `joined_paths` using [`split_paths()`] with [`mingw::path_lookup()`].
/// Note that just like Git, no lookup is performed if a slash or backslash is in `command`.
fn win_path_lookup(command: &Path, joined_paths: &OsStr) -> Option<PathBuf> {
    if !is_bare_command(command) {
        return None;
    }
    mingw::path_lookup(command.as_os_str(), split_paths(joined_paths), false)
}

/// Prepare `cmd` for [spawning][std::process::Command::spawn()] by configuring it with various builder methods.
///
/// Note that the default IO is configured for typical API usage, that is
///
/// - `stdin` is null to prevent blocking unexpectedly on consumption of stdin
/// - `stdout` is captured for consumption by the caller
/// - `stderr` is inherited to allow the command to provide context to the user
///
/// On Windows, terminal Windows will be suppressed automatically. The command also receives the
/// [directories of Git's programs](gix_path::env::git_program_dirs()) in front of its `PATH`, as programs started
/// by Git do, and a program given by name is looked up there first. Neither applies if `PATH` is set with
/// [`Prepare::env()`].
///
/// ### Warning
///
/// When using this method, be sure that the invoked program doesn't rely on the current working dir and/or
/// environment variables to know its context. If so, call instead [`Prepare::with_context()`] to provide
/// additional information.
pub fn prepare(cmd: impl Into<OsString>) -> Prepare {
    Prepare {
        command: cmd.into(),
        shell_program: None,
        context: None,
        stdin: std::process::Stdio::null(),
        stdout: std::process::Stdio::piped(),
        stderr: std::process::Stdio::inherit(),
        args: Vec::new(),
        env: Vec::new(),
        use_shell: false,
        quote_command: false,
        manual_argument_splitting: ManualArgumentSplitting::IfNoShell,
    }
}

#[cfg(test)]
mod tests;
