use std::{
    path::Path,
    process::{Command, Stdio},
};

use bstr::{BString, ByteSlice, ByteVec};
use gix_error::Result;

use crate::{Program, helper};

/// Correctly quotes `git_progarm`, while assuming `name_and_args` are good to be put into a script.
fn external_name_command(
    git_program: &Path,
    name_and_args: &bstr::BStr,
    action: &helper::Action,
) -> Result<std::process::Command> {
    let git_program = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(git_program)?);
    let mut args = gix_quote::single(git_program.as_ref());
    args.push_str(" credential-");
    args.push_str(name_and_args);
    gix_command::prepare(gix_path::from_bstr(args.as_bstr())?.into_owned())
        .arg(action.as_arg(true))
        .command_may_be_shell_script_allow_manual_argument_splitting()
        .try_into()
}

/// The error code with which Unix systems refuse to execute a file that isn't a program, like a script without `#!` line.
const ENOEXEC: i32 = 8;

/// Connect the standard streams of `cmd` as the helper needs them for `action`, and inherit `stderr` only if `stderr`
/// is `true`.
fn configure_io(cmd: &mut Command, action: &helper::Action, stderr: bool) {
    cmd.stdin(Stdio::piped())
        .stdout(if action.expects_output() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if stderr { Stdio::inherit() } else { Stdio::null() });
}

/// Return a command that runs the program of `cmd` as a script of the shell, with the arguments, environment and working
/// directory of `cmd`, as Git's `start_command()` does if the system can't execute the program.
fn with_shell(cmd: &Command) -> Result<Command> {
    let mut with_shell = Command::try_from(
        gix_command::prepare(gix_path::env::shell())
            .arg(cmd.get_program())
            .args(cmd.get_args()),
    )?;
    for (name, value) in cmd.get_envs() {
        match value {
            Some(value) => with_shell.env(name, value),
            None => with_shell.env_remove(name),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        with_shell.current_dir(dir);
    }
    Ok(with_shell)
}

/// The kind of helper program to use.
///
/// Of these, [`Kind::Builtin`] needs `git`, and so do the helpers of [`Kind::ExternalName`] that run as
/// `git credential-<name>`, as listed there.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Kind {
    /// The built-in `git credential` helper program, part of any `git` distribution, which runs as `git credential`.
    Builtin,
    /// A custom credentials helper, as identified just by the name with optional arguments, like `manager` or
    /// `store --file=<path>`.
    ///
    /// Git runs it as `git credential-<name> [args]` through the shell, and `git` then runs its builtin
    /// `credential-store` or `credential-cache`, or the program `git-credential-<name>` with the arguments. Here it
    /// runs without `git` where that's possible:
    ///
    /// - `store` runs Git's builtin `credential-store` in this process, with the file of `--file=<path>` or the
    ///   default files `~/.git-credentials` and `$XDG_CONFIG_HOME/git/credentials`, or `~/.config/git/credentials`
    ///   if `XDG_CONFIG_HOME` isn't set. It waits a second for the lock of a file it changes, the default of
    ///   `credentialStore.lockTimeoutMS`, which isn't read.
    /// - Other names run the program `git-credential-<name> [args]`, through a shell only if the arguments need one.
    ///   Like Git, it's found in `GIT_EXEC_PATH` if that's set, in the
    ///   [directories of Git's programs](gix_path::env::git_program_dirs()), and in `PATH`.
    ///
    /// These need `git`, and run as `git credential-<name> [args]`:
    ///
    /// - `cache`, Git's builtin `credential-cache`, which talks to Git's own daemon.
    /// - `store` with arguments that need a shell for more than quotes, escapes and a `~` that starts a word, or with
    ///   options other than `--file`, which Git's `credential-store` parses.
    /// - A name whose program isn't found there, as Git may find it in its exec path, which isn't known without
    ///   starting Git on platforms other than Windows. On Windows, this includes a program that is neither an `.exe`
    ///   file nor a script with a `#!` line.
    /// - A name with other characters than ASCII letters, digits, `-`, `_`, `.` and `+`, which the shell may change.
    ExternalName {
        /// The name like `foo` along with optional args, like `foo --arg --bar="a b"`, with arguments using `sh` shell quoting rules.
        /// The program executed will be `git-credential-foo [args]` if `name_and_args` starts with `foo [args]`.
        /// Note that a shell is only used if it's needed.
        name_and_args: BString,
    },
    /// A custom credentials helper, as identified just by the absolute path to the program and optional arguments. The program is executed through a shell.
    ExternalPath {
        /// The absolute path to the executable, like `/path/to/exe` along with optional args, like `/path/to/exe --arg --bar="a b"`, with arguments using `sh`
        /// shell quoting rules.
        path_and_args: BString,
    },
    /// A script to execute with `sh`.
    ExternalShellScript(BString),
}

/// How a [`Program`] runs to perform an action.
pub(crate) enum Run {
    /// As this command, ready to be started.
    Command(Command),
    /// As Git's `credential-store`, in this process, with these options.
    Store(store::Options),
}

/// Initialization
impl Program {
    /// Create a new program of the given `kind`.
    pub fn from_kind(kind: Kind) -> Self {
        Program {
            kind,
            child: None,
            stderr: true,
        }
    }

    /// Parse the given input as per the custom helper definition, supporting `!<script>`, `name` and `/absolute/name`, the latter two
    /// also support arguments which are ignored here.
    ///
    /// Returns an error if the helper name or path cannot be represented by the platform.
    pub fn from_custom_definition(input: impl Into<BString>) -> Result<Self> {
        fn from_custom_definition_inner(mut input: BString) -> Result<Program> {
            let kind = if input.starts_with(b"!") {
                input.remove(0);
                Kind::ExternalShellScript(input)
            } else {
                let path = gix_path::from_bstr(
                    input
                        .find_byte(b' ')
                        .map_or(input.as_slice(), |pos| &input[..pos])
                        .as_bstr(),
                )?;
                if gix_path::is_absolute(path) {
                    Kind::ExternalPath { path_and_args: input }
                } else {
                    Kind::ExternalName { name_and_args: input }
                }
            };
            Ok(Program {
                kind,
                child: None,
                stderr: true,
            })
        }
        from_custom_definition_inner(input.into())
    }

    /// Convert the program into the respective command, suitable to invoke `action`, or return `None` for `store`, Git's
    /// builtin `credential-store`, which runs in this process instead (see [`Kind::ExternalName`]).
    ///
    /// Returns an error if helper or executable paths cannot be represented in the required encoding.
    pub fn to_command(&self, action: &helper::Action) -> Result<Option<std::process::Command>> {
        Ok(match self.to_run(action)? {
            Run::Command(cmd) => Some(cmd),
            Run::Store(_) => None,
        })
    }

    /// Decide how the program runs to invoke `action`, as a command or in this process.
    pub(crate) fn to_run(&self, action: &helper::Action) -> Result<Run> {
        let mut cmd = match &self.kind {
            Kind::Builtin => {
                let mut cmd = Command::try_from(gix_command::prepare(gix_path::env::exe_invocation()))?;
                cmd.arg("credential").arg(action.as_arg(false));
                cmd
            }
            Kind::ExternalName { name_and_args } => match named::resolve(name_and_args.as_bstr())? {
                named::Resolution::Store(options) => return Ok(Run::Store(options)),
                named::Resolution::Program(program) => Command::try_from(program.arg(action.as_arg(true)))?,
                named::Resolution::Git => {
                    external_name_command(gix_path::env::exe_invocation(), name_and_args.as_bstr(), action)?
                }
            },
            Kind::ExternalShellScript(for_shell)
            | Kind::ExternalPath {
                path_and_args: for_shell,
            } => gix_command::prepare(gix_path::from_bstr(for_shell.as_bstr())?.as_ref())
                .command_may_be_shell_script()
                .arg(action.as_arg(true))
                .try_into()?,
        };
        configure_io(&mut cmd, action, self.stderr);
        Ok(Run::Command(cmd))
    }
}

/// Builder
impl Program {
    /// By default `stderr` of programs is inherited and typically displayed in the terminal.
    pub fn suppress_stderr(mut self) -> Self {
        self.stderr = false;
        self
    }
}

impl Program {
    pub(crate) fn start(
        &mut self,
        mut cmd: Command,
        action: &helper::Action,
    ) -> std::io::Result<(std::process::ChildStdin, Option<std::process::ChildStdout>)> {
        assert!(self.child.is_none(), "BUG: must not call `start()` twice");
        gix_trace::debug!(cmd = ?cmd, "launching credential helper");
        let mut child = match cmd.spawn() {
            // Git's `start_command()` runs a program that the system can't execute with the shell, as a script.
            Err(err) if cfg!(unix) && err.raw_os_error() == Some(ENOEXEC) => {
                let mut cmd = with_shell(&cmd).map_err(std::io::Error::other)?;
                configure_io(&mut cmd, action, self.stderr);
                gix_trace::debug!(cmd = ?cmd, "launching credential helper as a script of the shell");
                cmd.spawn()?
            }
            result => result?,
        };
        let stdin = child.stdin.take().expect("stdin to be configured");
        let stdout = child.stdout.take();

        self.child = child.into();
        Ok((stdin, stdout))
    }

    pub(crate) fn finish(&mut self) -> std::io::Result<()> {
        let mut child = self.child.take().expect("Call `start()` before calling finish()");
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            let failure = gix_error::message("Credentials helper program failed").with_exit_status(status);
            Err(std::io::Error::other(failure))
        }
    }
}

///
pub mod main;
pub use main::function::main;

mod named;
pub(crate) mod store;

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    use crate::helper;

    #[test]
    fn git_program_with_spaces_is_quoted_in_external_name_shell_scripts() -> gix_testtools::TestResult {
        let cmd = super::external_name_command(
            Path::new(r"C:\Program Files\Git\mingw64\bin\git.exe"),
            "manager --config=~/credentials".into(),
            &helper::Action::Get(Default::default()),
        )?;
        let script = cmd
            .get_args()
            .skip_while(|arg| *arg != OsStr::new("-c"))
            .nth(1)
            .expect("a shell invocation has its script after the '-c' argument");

        assert_eq!(
            script,
            OsStr::new(if cfg!(windows) {
                r#"'C:/Program Files/Git/mingw64/bin/git.exe' credential-manager --config=~/credentials "$@""#
            } else {
                r#"'C:\Program Files\Git\mingw64\bin\git.exe' credential-manager --config=~/credentials "$@""#
            }),
            "the Git executable is a single shell token even when its path contains spaces"
        );
        Ok(())
    }
}
