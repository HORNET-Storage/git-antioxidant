//! Test support for Git signing without using the user's identities or configuration.
//!
//! The bundled, passwordless SSH, OpenPGP, and X.509 identities are copied or imported
//! into disposable directories with suitably restrictive permissions. Callers are
//! responsible for checking that the required signing [`crate::signature::program_available()`] and must keep the
//! returned [`tempfile::TempDir`] alive while using it.
//!
//! These public test identities provide no security and must never be used outside tests.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::Result;

/// The identity associated with all signing fixtures.
pub const IDENTITY: &str = "signing@example.com";

/// Return the path to a signing fixture named `name`.
pub fn fixture(name: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/signature/fixtures")
        .join(name)
}

/// Return `path` in a form understood by `program` as gix runs it.
///
/// GnuPG and OpenSSH programs built for MSYS2, which Git for Windows provides, expect MSYS paths like `/c/Users/name`
/// instead of native paths like `C:\\Users\\name` or `C:/Users/name`, which other Windows programs expect.
pub fn path_for_command(program: impl AsRef<OsStr>, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if is_msys_program(program.as_ref()) {
        PathBuf::from(msys_path(
            path.to_str().expect("signing test fixture paths must be valid UTF-8"),
        ))
    } else {
        path.to_owned()
    }
}

/// Return whether `program`, as gix finds it, is built for MSYS2, whose runtime library is installed beside it.
fn is_msys_program(program: &OsStr) -> bool {
    cfg!(windows)
        && command(program).is_ok_and(|command| {
            Path::new(command.get_program())
                .parent()
                .is_some_and(|dir| dir.join("msys-2.0.dll").is_file())
        })
}

fn msys_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        format!(
            "/{drive}{rest}",
            drive = (bytes[0] as char).to_ascii_lowercase(),
            rest = &path[2..]
        )
    } else {
        path
    }
}

/// Return whether signing `program` can be launched as gix runs it.
pub fn program_available(program: impl AsRef<OsStr>) -> bool {
    command(program).is_ok_and(|mut command| {
        command
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    })
}

/// Prepare `program` to run as gix runs programs, which on Windows looks in Git's program directories before `PATH`.
fn command(program: impl AsRef<OsStr>) -> Result<Command> {
    Ok(Command::try_from(gix_command::prepare(program.as_ref()))?)
}

/// Create an isolated signer home with suitably restrictive permissions.
pub fn isolated_home() -> Result<crate::tempfile::TempDir> {
    #[cfg(unix)]
    let home = crate::tempfile::Builder::new().prefix("gix-sign-").tempdir_in("/tmp")?;
    #[cfg(not(unix))]
    let home = crate::tempfile::TempDir::new()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(home)
}

/// Copy the passwordless SSH signing key pair to temporary files with suitably restrictive permissions.
///
/// The returned path names the private key; its public key is available at the same path with the `.pub` extension.
/// Keeping both files mirrors `ssh-keygen` key generation and is required by implementations which cannot derive the
/// public key from the private key while signing.
pub fn ssh_private_key() -> Result<(crate::tempfile::TempDir, PathBuf)> {
    let home = crate::tempfile::TempDir::new()?;
    let key = home.path().join("key");
    std::fs::copy(fixture("ssh-private"), &key)?;
    std::fs::copy(fixture("ssh-private.pub"), key.with_extension("pub"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok((home, key))
}

/// Import the passwordless OpenPGP signing identity into a temporary home.
///
/// Passing this directory to `gpg --homedir` keeps the test's keys, configuration,
/// and trust state separate from the user's GnuPG home and removes them with the directory.
pub fn openpgp_home() -> Result<crate::tempfile::TempDir> {
    let home = isolated_home()?;
    run(command("gpg")?
        .args(["--batch", "--homedir"])
        .arg(path_for_command("gpg", home.path()))
        .args(["--import"])
        .arg(path_for_command("gpg", fixture("openpgp-secret.asc"))))?;
    Ok(home)
}

/// Import and trust the passwordless X.509 signing identity in a temporary home.
///
/// Passing this directory to `gpgsm --homedir` keeps the test's keys, configuration,
/// and trust list separate from the user's GnuPG home and removes them with the directory.
pub fn x509_home() -> Result<crate::tempfile::TempDir> {
    let home = isolated_home()?;
    run(command("gpgsm")?
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--homedir",
        ])
        .arg(path_for_command("gpgsm", home.path()))
        .arg("--import")
        .arg(path_for_command("gpgsm", fixture("x509-identity.p12"))))?;
    let keys = command("gpgsm")?
        .args(["--batch", "--homedir"])
        .arg(path_for_command("gpgsm", home.path()))
        .args(["-K", "--with-colons"])
        .output()?;
    assert!(keys.status.success(), "the imported X.509 key can be listed");
    let keys = String::from_utf8_lossy(&keys.stdout);
    let fingerprint = keys
        .lines()
        .find_map(|line| {
            line.strip_prefix("fpr:::::::::")
                .and_then(|line| line.split(':').next())
        })
        .expect("gpgsm reports a fingerprint for the imported key");
    std::fs::write(home.path().join("trustlist.txt"), format!("{fingerprint} S relax\n"))?;
    let _ = command("gpgconf")?
        .args(["--homedir"])
        .arg(path_for_command("gpgconf", home.path()))
        .args(["--reload", "all"])
        .status();
    Ok(home)
}

fn run(command: &mut Command) -> Result {
    let output = command.output()?;
    assert!(
        output.status.success(),
        "signature fixture setup succeeds: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::msys_path;

    #[test]
    fn windows_paths_for_unix_derived_commands_are_msys_paths() {
        assert_eq!(msys_path(r"C:\Users\name\key"), "/c/Users/name/key");
        assert_eq!(msys_path("D:/a/project/key"), "/d/a/project/key");
        assert_eq!(msys_path(r"relative\key"), "relative/key");
        assert_eq!(msys_path(r"\\server\share\key"), "//server/share/key");
    }
}
