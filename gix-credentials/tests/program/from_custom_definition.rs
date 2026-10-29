use gix_credentials::{Program, helper, program::Kind};
use gix_testtools::TestResult;

static GIT: std::sync::LazyLock<&'static str> = std::sync::LazyLock::new(|| {
    gix_path::env::exe_invocation()
        .to_str()
        .expect("some `from_custom_definition` tests must be run where 'git' path is valid Unicode")
});

// The basename of the default shell, used as the `command_name` operand after
// `-c <script>` and observable inside the shell as `$0`. The default shell is
// `gix_path::env::shell()`, documented as `/bin/sh` on Unix and a path ending
// in `sh.exe` on Windows.
const SH_BASENAME: &str = if cfg!(windows) { "sh.exe" } else { "sh" };

/// Return the command that runs `prog` to store a credential, which all helpers but `store` have.
fn store_command(prog: &Program) -> gix_error::Result<std::process::Command> {
    Ok(prog
        .to_command(&helper::Action::Store("egal".into()))?
        .expect("only `store` runs without a command"))
}

/// Return the program that runs Git.
fn git_program() -> gix_error::Result<std::path::PathBuf> {
    Ok(
        std::process::Command::try_from(gix_command::prepare(gix_path::env::exe_invocation()))?
            .get_program()
            .into(),
    )
}

#[test]
fn empty() -> TestResult {
    let prog = Program::from_custom_definition("")?;
    assert!(matches!(&prog.kind, Kind::ExternalName { name_and_args } if name_and_args.is_empty()));
    let cmd = store_command(&prog)?;
    assert_eq!(
        std::path::Path::new(cmd.get_program()),
        std::path::Path::new(
            std::process::Command::try_from(gix_command::prepare(gix_path::env::exe_invocation()))?.get_program()
        ),
        "Git uses the platform's command resolution"
    );
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["credential-", "store"],
        "not useful, but allowed, would have to be caught elsewhere"
    );
    Ok(())
}

#[test]
fn simple_script_in_path() -> TestResult {
    let prog = Program::from_custom_definition("!exe")?;
    assert!(matches!(&prog.kind, Kind::ExternalShellScript(script) if script == "exe"));
    let cmd = store_command(&prog)?;
    assert_eq!(
        cmd.get_program(),
        std::process::Command::try_from(gix_command::prepare("exe"))?.get_program(),
        "it didn't detect anything shell-scripty, and thus doesn't use a shell"
    );
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["store"],
        "the helper receives its action"
    );
    Ok(())
}

#[test]
fn name_with_args() -> TestResult {
    let input = "name --arg --bar=\"a b\"";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalName{name_and_args} if name_and_args == input));
    let cmd = store_command(&prog)?;
    assert_eq!(
        std::path::Path::new(cmd.get_program()),
        std::path::Path::new(
            std::process::Command::try_from(gix_command::prepare(gix_path::env::exe_invocation()))?.get_program()
        ),
        "Git uses the platform's command resolution"
    );
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["credential-name", "--arg", "--bar=a b", "store"],
        "quoted arguments are split without losing their contents"
    );
    Ok(())
}

#[test]
fn name_with_special_args() -> TestResult {
    let input = "name --arg --bar=~/folder/in/home";
    let prog = Program::from_custom_definition(input)?;
    let sh = gix_path::env::shell_command();
    let git = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(std::path::Path::new(*GIT))?);
    let quoted_git = gix_quote::single(git.as_ref());
    assert!(matches!(&prog.kind, Kind::ExternalName{name_and_args} if name_and_args == input));
    assert_eq!(
        format!("{:?}", store_command(&prog)?),
        format!(
            r#"{sh:?} "-c" "{quoted_git} credential-name --arg --bar=~/folder/in/home \"$@\"" "{SH_BASENAME}" "store""#
        )
    );
    Ok(())
}

#[test]
fn name() -> TestResult {
    let input = "name";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalName{name_and_args} if name_and_args == input));
    let cmd = store_command(&prog)?;
    assert_eq!(
        std::path::Path::new(cmd.get_program()),
        std::path::Path::new(
            std::process::Command::try_from(gix_command::prepare(gix_path::env::exe_invocation()))?.get_program()
        ),
        "we detect that this can run without shell, which is also more portable on windows"
    );
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["credential-name", "store"],
        "Git receives the helper name and action"
    );
    Ok(())
}

#[test]
fn path_with_args_that_definitely_need_shell() -> TestResult {
    let input = "/abs/name --arg --bar=\"a b\"";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalPath{path_and_args} if path_and_args == input));
    let sh = gix_path::env::shell_command();
    assert_eq!(
        format!("{:?}", store_command(&prog)?),
        format!(r#"{sh:?} "-c" "/abs/name --arg --bar=\"a b\" \"$@\"" "{SH_BASENAME}" "store""#)
    );
    Ok(())
}

#[test]
fn path_without_args() -> TestResult {
    let input = "/abs/name";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalPath{path_and_args} if path_and_args == input));
    assert_eq!(
        format!("{:?}", store_command(&prog)?),
        r#""/abs/name" "store""#,
        "no shell is used"
    );
    Ok(())
}

#[test]
fn path_with_simple_args() -> TestResult {
    let input = "/abs/name a b";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalPath{path_and_args} if path_and_args == input));
    let sh = gix_path::env::shell_command();
    assert_eq!(
        format!("{:?}", store_command(&prog)?),
        format!(r#"{sh:?} "-c" "/abs/name a b \"$@\"" "{SH_BASENAME}" "store""#),
        "a shell is used as there are arguments, as Git does"
    );
    Ok(())
}

#[test]
fn cache_runs_as_git_credential_cache() -> TestResult {
    let input = "cache --timeout=10";
    let prog = Program::from_custom_definition(input)?;
    assert!(matches!(&prog.kind, Kind::ExternalName{name_and_args} if name_and_args == input));
    let cmd = store_command(&prog)?;
    assert_eq!(
        std::path::Path::new(cmd.get_program()),
        git_program()?,
        "Git's builtin `credential-cache` talks to Git's own daemon, which needs Git"
    );
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["credential-cache", "--timeout=10", "store"],
        "Git receives the helper name with its arguments and the action"
    );
    Ok(())
}

#[test]
fn store_runs_in_this_process() -> TestResult {
    for input in [
        "store",
        "store --file=creds",
        "store --file creds",
        "store --file='with space' --no-file",
        "store --file=\"with space\"",
        "store\t--file=with\\ space",
    ] {
        let prog = Program::from_custom_definition(input)?;
        assert!(
            prog.to_command(&helper::Action::Store("egal".into()))?.is_none(),
            "{input}: Git's builtin `credential-store` runs in this process, without a command"
        );
    }
    Ok(())
}

#[test]
fn store_with_arguments_for_git_runs_as_git_credential_store() -> TestResult {
    for input in [
        "store --file=$HOME/creds",
        "store --file ~other/creds",
        "store --fi=creds",
        "store --file",
        "store --file=",
        "store creds",
        "store --file=creds # comment",
        "store --file=a;b",
    ] {
        let prog = Program::from_custom_definition(input)?;
        assert!(
            format!("{:?}", store_command(&prog)?).contains("credential-store"),
            "{input}: Git's `credential-store` parses these arguments, or the shell interprets them"
        );
    }
    Ok(())
}

#[test]
fn names_the_shell_would_change_run_as_git_credential_name() -> TestResult {
    for input in ["na$me", "name;other", "na\"me\"", "name\nother"] {
        let prog = Program::from_custom_definition(input)?;
        assert!(
            format!("{:?}", store_command(&prog)?).contains("credential-na"),
            "{input:?}: the shell interprets the name, so Git runs the helper"
        );
    }
    Ok(())
}
