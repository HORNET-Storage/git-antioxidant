use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use bstr::ByteSlice;
use gix_error::Result;

use crate::{
    Context, ManualArgumentSplitting, Prepare, extract_interpreter, is_bare_command, split_paths, win_path_lookup,
};

/// Builder
impl Prepare {
    /// If called, the command will be checked for characters that are typical for shell
    /// scripts, and if found will use `sh` to execute it or whatever is set as
    /// [`with_shell_program()`](Self::with_shell_program()).
    ///
    /// Commands are inspected using their native encoded bytes, including commands that cannot
    /// be represented as UTF-8.
    ///
    /// If a shell is used, then arguments given here with [arg()](Self::arg) or
    /// [args()](Self::args) will be substituted via `"$@"` if it's not already present in the
    /// command.
    ///
    ///
    /// The [`command_may_be_shell_script_allow_manual_argument_splitting()`](Self::command_may_be_shell_script_allow_manual_argument_splitting())
    /// and [`command_may_be_shell_script_disallow_manual_argument_splitting()`](Self::command_may_be_shell_script_disallow_manual_argument_splitting())
    /// methods also call this method.
    ///
    /// If neither this method nor [`with_shell()`](Self::with_shell()) is called, commands are
    /// always executed verbatim and directly, without the use of a shell.
    pub fn command_may_be_shell_script(mut self) -> Self {
        self.use_shell = self
            .command
            .as_encoded_bytes()
            .find_byteset(b"|&;<>()$`\\\"' \t\n*?[#~=%")
            .is_some();
        self
    }

    /// If called, unconditionally use a shell to execute the command and its arguments.
    ///
    /// This uses `sh` to execute it, or whatever is set as
    /// [`with_shell_program()`](Self::with_shell_program()).
    ///
    /// Arguments given here with [arg()](Self::arg) or [args()](Self::args) will be
    /// substituted via `"$@"` if it's not already present in the command.
    ///
    /// If neither this method nor
    /// [`command_may_be_shell_script()`](Self::command_may_be_shell_script()) is called,
    /// commands are always executed verbatim and directly, without the use of a shell. (But
    /// see [`command_may_be_shell_script()`](Self::command_may_be_shell_script()) on other
    /// methods that call that method.)
    ///
    /// We also disallow manual argument splitting
    /// (see [`command_may_be_shell_script_disallow_manual_argument_splitting`](Self::command_may_be_shell_script_disallow_manual_argument_splitting()))
    /// to assure a shell is indeed used, no matter what.
    pub fn with_shell(mut self) -> Self {
        self.use_shell = true;
        self.manual_argument_splitting = ManualArgumentSplitting::Never;
        self
    }

    /// Quote the command if it is run in a shell, so its path is left intact.
    ///
    /// This is only meaningful if the command has been arranged to run in a shell, either
    /// unconditionally with [`with_shell()`](Self::with_shell()), or conditionally with
    /// [`command_may_be_shell_script()`](Self::command_may_be_shell_script()).
    ///
    /// Note that this should not be used if the command is a script - quoting is only the
    /// right choice if it's known to be a program path.
    ///
    /// Note also that this does not affect arguments passed with [arg()](Self::arg) or
    /// [args()](Self::args), which do not have to be quoted by the *caller* because they are
    /// passed as `"$@"` positional parameters (`"$1"`, `"$2"`, and so on).
    pub fn with_quoted_command(mut self) -> Self {
        self.quote_command = true;
        self
    }

    /// Set the name or path to the shell `program` to use if a shell is to be used, to avoid
    /// using the default shell which is `sh`.
    ///
    /// Note that shells that are not Bourne-style cannot be expected to work correctly,
    /// because POSIX shell syntax is assumed when searching for and conditionally adding
    /// `"$@"` to receive arguments, where applicable (and in the behaviour of
    /// [`with_quoted_command()`](Self::with_quoted_command()), if called).
    pub fn with_shell_program(mut self, program: impl Into<OsString>) -> Self {
        self.shell_program = Some(program.into());
        self
    }

    /// Unconditionally turn off using the shell when spawning the command.
    ///
    /// Note that not using the shell is the default. So an effective use of this method
    /// is some time after [`command_may_be_shell_script()`](Self::command_may_be_shell_script())
    /// or [`with_shell()`](Self::with_shell()) was called.
    pub fn without_shell(mut self) -> Self {
        self.use_shell = false;
        self
    }

    /// Set additional `ctx` to be used when spawning the process.
    ///
    /// Note that this is a must for most kind of commands that `git` usually spawns, as at
    /// least they need to know the correct Git repository to function.
    pub fn with_context(mut self, ctx: Context) -> Self {
        self.context = Some(ctx);
        self
    }

    /// Like [`command_may_be_shell_script()`](Self::command_may_be_shell_script()), but try to
    /// split arguments by hand if this can be safely done without a shell.
    ///
    /// This is useful on platforms where spawning processes is slow, or where many processes
    /// have to be spawned in a row which should be sped up. By default, arguments are split by
    /// hand only if no shell can be started, see [`ManualArgumentSplitting::IfNoShell`].
    ///
    /// Note that this does *not* check for the use of possible shell builtins. Commands may
    /// fail or behave differently if they are available as shell builtins and no corresponding
    /// external command exists, or the external command behaves differently.
    /// Leading shell assignment words are applied to the environment when followed by a command.
    pub fn command_may_be_shell_script_allow_manual_argument_splitting(mut self) -> Self {
        self.manual_argument_splitting = ManualArgumentSplitting::Always;
        self.command_may_be_shell_script()
    }

    /// Like [`command_may_be_shell_script()`](Self::command_may_be_shell_script()), but don't
    /// allow to bypass the shell even if manual argument splitting can be performed safely.
    pub fn command_may_be_shell_script_disallow_manual_argument_splitting(mut self) -> Self {
        self.manual_argument_splitting = ManualArgumentSplitting::Never;
        self.command_may_be_shell_script()
    }

    /// Configure the process to use `stdio` for _stdin_.
    pub fn stdin(mut self, stdio: Stdio) -> Self {
        self.stdin = stdio;
        self
    }
    /// Configure the process to use `stdio` for _stdout_.
    pub fn stdout(mut self, stdio: Stdio) -> Self {
        self.stdout = stdio;
        self
    }
    /// Configure the process to use `stdio` for _stderr_.
    pub fn stderr(mut self, stdio: Stdio) -> Self {
        self.stderr = stdio;
        self
    }

    /// Add `arg` to the list of arguments to call the command with.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add `args` to the list of arguments to call the command with.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args
            .append(&mut args.into_iter().map(Into::into).collect::<Vec<_>>());
        self
    }

    /// Add `key` with `value` to the environment of the spawned command.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Finalization
impl Prepare {
    /// Spawn the command as configured.
    ///
    /// Encoding errors during preparation are returned with [`std::io::ErrorKind::InvalidInput`].
    pub fn spawn(self) -> std::io::Result<std::process::Child> {
        let mut cmd =
            Command::try_from(self).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        gix_trace::debug!(cmd = ?cmd);
        cmd.spawn()
    }
}

impl TryFrom<Prepare> for Command {
    type Error = gix_error::Error;

    fn try_from(mut prep: Prepare) -> Result<Self> {
        let mut inline_env = Vec::new();
        let mut cmd = if prep.use_shell {
            let split_args = may_split_arguments(&prep)
                .then(|| {
                    let command = gix_path::os_str_into_bstr(&prep.command).ok()?;
                    if command.find_byteset(b"\\|&;<>()$`\n*?[#~%").is_none() {
                        crate::parse::command_line(command).ok()
                    } else {
                        None
                    }
                })
                .flatten();
            match split_args {
                Some(parsed) => {
                    let mut cmd = if cfg!(windows) {
                        windows_command(parsed.command, &prep.env, &parsed.env)
                    } else {
                        Command::new(parsed.command)
                    };
                    cmd.args(parsed.args);
                    inline_env = parsed.env;
                    cmd
                }
                None => {
                    let mut cmd = match prep.shell_program {
                        Some(shell) => Command::new(shell),
                        None => gix_path::env::shell_command(),
                    };
                    if explicit_path(&prep.env, &[]).is_none()
                        && let Some(path) = path_with_git_program_dirs()
                    {
                        cmd.env("PATH", path);
                    }
                    // Passed as `command_name` after `-c <script>`; the shell uses it
                    // as `$0`, which prefixes its own diagnostic messages. If the
                    // shell path has no extractable basename — reachable only via
                    // degenerate input like `""` or `/` — fall back to `_`, the
                    // conventional placeholder for an unused `$0`, rather than
                    // making a false claim about which shell is running.
                    let arg0 = std::path::Path::new(cmd.get_program())
                        .file_name()
                        .unwrap_or(std::ffi::OsStr::new("_"))
                        .to_os_string();
                    cmd.arg("-c");
                    if !prep.args.is_empty() {
                        if !prep.command.as_encoded_bytes().contains_str("$@") {
                            if prep.quote_command {
                                let command = gix_path::os_str_into_bstr(&prep.command)?;
                                prep.command = gix_path::from_bstring(gix_quote::single(command))?.into();
                            }
                            prep.command.push(r#" "$@""#);
                        } else {
                            gix_trace::debug!(
                                r#"Will not add '"$@"' to '{:?}' as it seems to contain '$@' already"#,
                                prep.command
                            );
                        }
                    }
                    cmd.arg(prep.command);
                    cmd.arg(arg0);
                    cmd
                }
            }
        } else if cfg!(windows) {
            windows_command(prep.command, &prep.env, &[])
        } else {
            Command::new(prep.command)
        };
        // We never want to have terminals pop-up on Windows if this runs from a GUI application.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.stdin(prep.stdin)
            .stdout(prep.stdout)
            .stderr(prep.stderr)
            .envs(prep.env)
            .args(prep.args);
        if let Some(ctx) = prep.context {
            if let Some(git_dir) = ctx.git_dir {
                cmd.env("GIT_DIR", &git_dir);
            }
            if let Some(worktree_dir) = ctx.worktree_dir {
                cmd.env("GIT_WORK_TREE", worktree_dir);
            }
            if let Some(value) = ctx.no_replace_objects {
                cmd.env("GIT_NO_REPLACE_OBJECTS", usize::from(value).to_string());
            }
            if let Some(namespace) = ctx.ref_namespace {
                cmd.env("GIT_NAMESPACE", gix_path::from_bstring(namespace)?);
            }
            if let Some(value) = ctx.literal_pathspecs {
                cmd.env("GIT_LITERAL_PATHSPECS", usize::from(value).to_string());
            }
            if let Some(value) = ctx.glob_pathspecs {
                cmd.env(
                    if value {
                        "GIT_GLOB_PATHSPECS"
                    } else {
                        "GIT_NOGLOB_PATHSPECS"
                    },
                    "1",
                );
            }
            if let Some(value) = ctx.icase_pathspecs {
                cmd.env("GIT_ICASE_PATHSPECS", usize::from(value).to_string());
            }
            if let Some(stderr) = ctx.stderr {
                cmd.stderr(if stderr { Stdio::inherit() } else { Stdio::null() });
            }
        }
        cmd.envs(inline_env);
        Ok(cmd)
    }
}

/// Return whether `prep` may run its command without a shell, if its arguments can be split safely by hand.
fn may_split_arguments(prep: &Prepare) -> bool {
    match prep.manual_argument_splitting {
        ManualArgumentSplitting::Never => false,
        ManualArgumentSplitting::IfNoShell => prep.shell_program.is_none() && !default_shell_exists(&prep.env),
        ManualArgumentSplitting::Always => true,
    }
}

/// Return whether the [default shell](gix_path::env::shell()) can be found, using the `PATH` of `env` if it sets one.
///
/// The default shell is an absolute path unless no Git for Windows installation is found, and then `sh.exe` has to be
/// in `PATH`.
fn default_shell_exists(env: &[(OsString, OsString)]) -> bool {
    let shell = Path::new(gix_path::env::shell());
    shell.is_absolute()
        || explicit_path(env, &[])
            .map(Cow::Borrowed)
            .or_else(|| std::env::var_os("PATH").map(Cow::Owned))
            .is_some_and(|joined_paths| win_path_lookup(shell, &joined_paths).is_some())
}

/// Create a Windows command using Git-compatible `PATH` lookup and shebang dispatch.
///
/// The last `PATH` in `inline_env` overrides the last one in `env`. Without either, the inherited `PATH` (of this
/// process) is used with the [directories of Git's programs](gix_path::env::git_program_dirs()) in front, and the
/// command receives this `PATH` as well, as programs started by Git do. The selected `PATH` is searched in order.
/// A resolved shebang script is launched through its interpreter, ignoring shebang arguments. If `PATH` does not
/// resolve a bare command, the missing program remains anchored in its first entry so Rust's broader Windows lookup
/// cannot find it elsewhere.
fn windows_command(command: OsString, env: &[(OsString, OsString)], inline_env: &[(String, OsString)]) -> Command {
    let explicit_joined_paths = explicit_path(env, inline_env);
    let git_joined_paths = match explicit_joined_paths {
        Some(_) => None,
        None => path_with_git_program_dirs(),
    };
    let joined_paths = explicit_joined_paths
        .or(git_joined_paths.as_deref())
        .map(Cow::Borrowed)
        .or_else(|| std::env::var_os("PATH").map(Cow::Owned));
    let looked_up = joined_paths
        .as_deref()
        .and_then(|joined_paths| win_path_lookup(command.as_ref(), joined_paths));
    let mut cmd = match looked_up {
        // Use the manually resolved path.
        Some(program) => program_command(&program),
        // A bare PATH miss must neither probe a worktree file nor use Rust's broader Windows search.
        None if is_bare_command(Path::new(&command)) => Command::new(prevent_further_path_lookup(
            command.as_ref(),
            joined_paths.as_deref().unwrap_or_else(|| OsStr::new("")),
        )),
        None => program_command(command.as_ref()),
    };
    if let Some(joined_paths) = &git_joined_paths {
        cmd.env("PATH", joined_paths);
    }
    cmd
}

/// Create a command to run `program`, through its interpreter if it's a script with a shebang.
fn program_command(program: &Path) -> Command {
    match extract_interpreter(program) {
        Some(shebang) => {
            let mut cmd = Command::new(shebang.interpreter);
            // Git for Windows ignores shebang arguments and passes only the script path.
            cmd.arg(program);
            cmd
        }
        None => Command::new(program),
    }
}

/// Return the `PATH` that the last assignment in `inline_env` sets, or else the last one in `env`, matching the name
/// without regard to case as Windows does.
fn explicit_path<'a>(env: &'a [(OsString, OsString)], inline_env: &'a [(String, OsString)]) -> Option<&'a OsStr> {
    inline_env
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.as_os_str())
        .or_else(|| {
            env.iter()
                .rev()
                .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                .map(|(_, value)| value.as_os_str())
        })
}

/// Return the inherited `PATH` with the [directories of Git's programs](gix_path::env::git_program_dirs()) in front, as
/// Git sets it for the programs it starts, or `None` if there are no such directories.
fn path_with_git_program_dirs() -> Option<OsString> {
    let dirs = gix_path::env::git_program_dirs();
    if dirs.is_empty() {
        return None;
    }
    let mut joined_paths = std::env::join_paths(dirs).ok()?;
    if let Some(inherited) = std::env::var_os("PATH").filter(|inherited| !inherited.is_empty()) {
        // Only Windows has these directories, and it separates `PATH` entries with `;`.
        joined_paths.push(";");
        joined_paths.push(inherited);
    }
    Some(joined_paths)
}

/// Represent the failed lookup of `command` in `PATH` without permitting another search.
///
/// `joined_paths` is the complete value of `PATH`, not one of its entries. The first non-empty entry is
/// joined with `command`, producing a path that Rust's Windows resolver will not look up elsewhere. If there is no such
/// entry, a trailing separator makes `command` invalid instead.
fn prevent_further_path_lookup(command: &Path, joined_paths: &OsStr) -> PathBuf {
    if let Some(mut root) = split_paths(joined_paths).next() {
        root.push(command);
        root
    } else {
        let mut missing = command.to_owned();
        // A trailing separator makes the program invalid without allowing Rust to search ambient locations.
        missing.push("");
        missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_path_is_used_for_both_probing_and_execution() -> gix_testtools::TestResult {
        if gix_testtools::run_in_isolated_process()? {
            return Ok(());
        }
        let git_dirs = gix_path::env::git_program_dirs();
        let root = gix_testtools::tempfile::tempdir()?;
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin)?;
        let _cwd = gix_testtools::set_current_dir(root.path())?;
        let program = "gix-command-missing-program";
        std::fs::write(program, b"#!/untrusted/interpreter\n")?;

        for path in [Some(bin.to_str().expect("temporary path is UTF-8")), Some(""), None] {
            let environment = gix_testtools::Env::new();
            let _environment = match path {
                Some(path) => environment.set("PATH", path),
                None => environment.unset("PATH"),
            };
            let mut cmd = windows_command(program.into(), &[], &[]);
            let expected = match (git_dirs.first(), path) {
                (Some(git_dir), _) => git_dir.join(program),
                (None, Some(path)) if !path.is_empty() => bin.join(program),
                (None, _) => Path::new(program).join(""),
            };
            assert_eq!(
                cmd.get_program(),
                expected,
                "a PATH miss cannot probe a worktree file or leave a bare command for another lookup"
            );
            assert!(cmd.get_args().next().is_none(), "the planted shebang is never used");
            assert!(cmd.spawn().is_err(), "a missing program fails to spawn");
        }

        let _environment = gix_testtools::Env::new().set("PATH", bin.to_str().expect("temporary path is UTF-8"));
        let executable = bin.join(format!("{program}.exe"));
        std::fs::write(&executable, b"executable placeholder")?;
        assert_eq!(
            windows_command(program.into(), &[], &[]).get_program(),
            executable,
            "the executable selected from PATH is also the one passed to the process launcher"
        );
        let script = format!("./{program}");
        let explicit_script = windows_command(script.as_str().into(), &[], &[]);
        assert_eq!(
            explicit_script.get_program(),
            Path::new("/untrusted/interpreter"),
            "an explicitly requested script retains shebang support"
        );
        assert_eq!(
            explicit_script.get_args().collect::<Vec<_>>(),
            [OsStr::new(&script)],
            "the interpreter receives the explicitly requested script"
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(not(windows), ignore = "only Git for Windows has program directories")]
    fn programs_distributed_with_git_for_windows_come_first() -> gix_testtools::TestResult {
        if gix_testtools::run_in_isolated_process()? {
            return Ok(());
        }
        let git_dirs = gix_path::env::git_program_dirs();
        let shell = gix_path::env::shell();
        let git_sed = git_dirs
            .iter()
            .map(|dir| dir.join("sed.exe"))
            .find(|sed| sed.is_file())
            .expect("Git for Windows distributes `sed`");
        let root = gix_testtools::tempfile::tempdir()?;
        let other_sed = root.path().join("sed.exe");
        std::fs::write(&other_sed, b"executable placeholder")?;
        let _environment =
            gix_testtools::Env::new().set("PATH", root.path().to_str().expect("temporary path is UTF-8"));
        let mut git_path = std::env::join_paths(git_dirs)?;
        git_path.push(";");
        git_path.push(root.path());

        let cmd = windows_command("sed".into(), &[], &[]);
        assert_eq!(cmd.get_program(), git_sed, "Git's `sed` precedes the one in `PATH`");
        assert_eq!(
            path_variable(&cmd),
            Some(git_path.as_os_str()),
            "the program finds Git's programs first as well"
        );

        let cmd = Command::try_from(crate::prepare("sed -n p").command_may_be_shell_script())?;
        assert_eq!(cmd.get_program(), shell, "a command with arguments runs in Git's shell");
        assert_eq!(
            path_variable(&cmd),
            Some(git_path.as_os_str()),
            "the shell finds Git's programs first"
        );

        let cmd = windows_command("sed".into(), &[("PATH".into(), root.path().into())], &[]);
        assert_eq!(cmd.get_program(), other_sed, "an explicit `PATH` is searched as it is");
        assert_eq!(path_variable(&cmd), None, "the explicit `PATH` isn't replaced");
        Ok(())
    }

    /// Return the value that `cmd` sets for `PATH`.
    fn path_variable(cmd: &Command) -> Option<&OsStr> {
        cmd.get_envs()
            .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
            .and_then(|(_, value)| value)
    }

    #[test]
    fn explicit_path_lookup_failure_stays_within_that_path() -> gix_testtools::TestResult {
        let joined_paths = std::env::join_paths(["", "not/a/real/path", "also/not/real"])?;
        let cmd = windows_command("missing.exe".into(), &[], &[("PATH".into(), joined_paths)]);
        assert_eq!(
            cmd.get_program(),
            std::path::Path::new("not/a/real/path/missing.exe"),
            "an inline PATH miss must not leave a bare program for Rust to resolve elsewhere"
        );
        Ok(())
    }
}
