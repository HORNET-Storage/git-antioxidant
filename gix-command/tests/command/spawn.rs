use crate::TestResult;
use bstr::ByteSlice;

#[test]
fn environment_variables_are_passed_one_by_one() -> TestResult {
    let out = gix_command::prepare("echo $FIRST $SECOND")
        .env("FIRST", "first")
        .env("SECOND", "second")
        .command_may_be_shell_script()
        .spawn()?
        .wait_with_output()?;
    assert_eq!(out.stdout.as_bstr(), "first second\n");
    Ok(())
}

#[test]
fn disallow_shell() -> TestResult {
    let out = gix_command::prepare("PATH= echo hi")
        .command_may_be_shell_script_disallow_manual_argument_splitting()
        .spawn()?
        .wait_with_output()?;
    assert_eq!(out.stdout.as_bstr(), "hi\n");

    let mut cmd: std::process::Command = gix_command::prepare("echo hi")
        .command_may_be_shell_script()
        .without_shell()
        .try_into()?;
    assert!(
        cmd.env_remove("PATH").spawn().is_err(),
        "no command named 'echo hi' exists"
    );
    Ok(())
}

#[test]
fn script_with_dollar_at() -> TestResult {
    let out = std::process::Command::try_from(
        gix_command::prepare(r#"echo "$@""#)
            .command_may_be_shell_script()
            .arg("arg"),
    )?
    .spawn()?
    .wait_with_output()?;
    assert_eq!(
        out.stdout.to_str_lossy().trim(),
        "arg arg",
        "as in Git, the arguments are added to the command whether or not it mentions them"
    );
    Ok(())
}

#[test]
fn direct_command_execution_searches_in_path() -> TestResult {
    assert!(
        gix_command::prepare(if cfg!(unix) { "ls" } else { "attrib.exe" })
            .spawn()?
            .wait()?
            .success()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn direct_command_with_absolute_command_path() -> TestResult {
    assert!(gix_command::prepare("/usr/bin/env").spawn()?.wait()?.success());
    Ok(())
}

mod with_shell {
    use crate::TestResult;
    use gix_testtools::bstr::ByteSlice;

    #[test]
    fn command_in_path_with_args() -> TestResult {
        // `ls` is occasionaly a builtin, as in busybox ash, but it is usually external.
        // Git's shell on Windows turns arguments like `/d` into Windows paths, so `attrib` gets a path.
        assert!(
            gix_command::prepare(if cfg!(unix) { "ls -l" } else { "attrib.exe ." })
                .command_may_be_shell_script()
                .spawn()?
                .wait()?
                .success()
        );
        Ok(())
    }

    #[test]
    fn shell_builtin_or_command_in_path() -> TestResult {
        let out = gix_command::prepare("echo")
            .command_may_be_shell_script()
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "\n");
        Ok(())
    }

    #[test]
    fn shell_builtin_or_command_in_path_with_single_extra_arg() -> TestResult {
        let out = gix_command::prepare("printf")
            .command_may_be_shell_script()
            .arg("1")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "1");
        Ok(())
    }

    #[test]
    fn shell_builtin_or_command_in_path_with_multiple_extra_args() -> TestResult {
        let out = gix_command::prepare("printf")
            .command_may_be_shell_script()
            .arg("%s")
            .arg("arg")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "arg");
        Ok(())
    }

    #[test]
    fn force_shell_builtin() -> TestResult {
        let out = gix_command::prepare("echo").with_shell().spawn()?.wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "\n");
        Ok(())
    }

    #[test]
    fn force_shell_builtin_with_single_extra_arg() -> TestResult {
        let out = gix_command::prepare("printf")
            .with_shell()
            .arg("1")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "1");
        Ok(())
    }

    #[test]
    fn force_shell_builtin_with_multiple_extra_args() -> TestResult {
        let out = gix_command::prepare("printf")
            .with_shell()
            .arg("%s")
            .arg("arg")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "arg");
        Ok(())
    }

    #[test]
    fn sh_shell_specific_script_code() -> TestResult {
        assert!(
            gix_command::prepare(":;:;:")
                .command_may_be_shell_script()
                .spawn()?
                .wait()?
                .success()
        );
        Ok(())
    }

    #[test]
    fn sh_shell_specific_script_code_with_single_extra_arg() -> TestResult {
        let out = gix_command::prepare(":;printf")
            .command_may_be_shell_script()
            .arg("1")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "1");
        Ok(())
    }

    #[test]
    fn sh_shell_specific_script_code_with_multiple_extra_args() -> TestResult {
        let out = gix_command::prepare(":;printf")
            .command_may_be_shell_script()
            .arg("%s")
            .arg("arg")
            .spawn()?
            .wait_with_output()?;
        assert!(out.status.success());
        assert_eq!(out.stdout.as_bstr(), "arg");
        Ok(())
    }

    #[test]
    fn commands_see_the_parameters_they_see_in_git() -> TestResult {
        let dir = gix_testtools::tempfile::tempdir()?;
        for command in [
            r#"printf '[%s]\n' "$0" "$#" "$@""#,
            r#"f() { printf '[%s]\n' "$0" "$#" "$@"; }; f"#,
            r#"printf '[%s]\n' "$1""#,
            "printf",
        ] {
            let git = gix_testtools::git_command(dir.path())
                .args(["-c", &format!("alias.run=!{command}"), "run", "[%s]", "b c"])
                .output()?;
            assert!(git.status.success(), "Git ran {command:?}: {}", git.stderr.as_bstr());
            let ours = gix_command::prepare(command)
                .command_may_be_shell_script()
                .args(["[%s]", "b c"])
                .spawn()?
                .wait_with_output()?;
            assert!(ours.status.success(), "{command:?}");
            assert_eq!(ours.stdout.as_bstr(), git.stdout.as_bstr(), "{command:?}");
        }
        Ok(())
    }
}
