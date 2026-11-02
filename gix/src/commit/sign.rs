use std::{ffi::OsString, process::Stdio};

use crate::error::{ResultExt, bail, message};
use crate::{
    Error, Result,
    bstr::{BStr, BString, ByteSlice},
    config::tree::{Gpg, User, gpg},
};

use gix_object::signature::sign::is_literal_ssh_key;
pub use gix_object::signature::{Format, sign::Options};

pub(crate) fn sign<'repo>(commit: &crate::Commit<'repo>) -> Result<crate::Commit<'repo>> {
    let options = commit.repo.commit_signing_options()?;
    let signed = commit
        .decode()?
        .sign(options)
        .or_raise(|| message("Could not sign the commit"))?;
    let id = commit.repo.write_object(&signed)?;
    commit.repo.find_commit(id)
}

/// The options to sign a commit with `key`, as `sign_commit_to_strbuf()` in Git's `commit.c` signs with it. An empty
/// `key` stands for that of `user.signingKey`, or else for the one that `get_signing_key()` provides for the format.
pub(crate) fn signing_options(repo: &crate::Repository, key: &BStr) -> Result<Options> {
    let config = repo.config_snapshot();
    let value = config
        .plumbing()
        .string_filter(Gpg::FORMAT, &mut repo.filter_config_section());
    let format = Gpg::FORMAT.try_into_signature_format(value.as_ref().map(|value| value.as_bstr()))?;
    let program = super::signature_program(&config, format)
        .or_raise(|| message("Could not interpolate the configured signing program path"))?;
    let key = if key.is_empty() {
        config
            .plumbing()
            .string_filter(User::SIGNING_KEY, &mut repo.filter_config_section())
            .filter(|key| !key.is_empty())
    } else {
        Some(key.to_owned())
    };
    let signing_key = match key {
        Some(key) if format == Format::Ssh && is_literal_ssh_key(&key).is_none() => ssh_key_path(repo, key)?,
        Some(key) => gix_path::from_bstring(key)?.into_os_string(),
        None if format == Format::Ssh => default_ssh_key(&config)?.ok_or_else(|| {
            Error::from_error(message(
                "user.signingKey or gpg.ssh.defaultKeyCommand must provide an SSH signing key",
            ))
        })?,
        None => {
            let committer = repo
                .committer()
                .ok_or_else(|| {
                    Error::from_error(message(
                        "Committer identity is not configured and user.signingKey is unset",
                    ))
                })?
                .or_raise(|| message("Could not parse the committer time for commit signing"))?;
            let mut identity = committer.name.to_owned();
            identity.extend_from_slice(b" <");
            identity.extend_from_slice(committer.email);
            identity.extend_from_slice(b">");
            gix_path::from_bstring(identity)?.into_os_string()
        }
    };
    Ok(Options {
        format,
        program,
        program_arguments: Vec::new(),
        signing_key,
        environment: Vec::new(),
    })
}

pub(crate) fn signing_options_if_enabled(repo: &crate::Repository) -> Result<Option<Options>> {
    let enabled = repo
        .config
        .may_sign_commits()?
        .then(|| signing_options(repo, "".into()));
    enabled.transpose()
}

/// The path of the SSH key file `key`, interpolated as `sign_buffer_ssh()` interpolates it with `interpolate_path()`:
/// a leading `~` or `%(prefix)/` is expanded, and a leading `:(optional)` stays part of the path, as only configured
/// paths can be optional.
fn ssh_key_path(repo: &crate::Repository, key: BString) -> Result<OsString> {
    let install_dir = repo.install_dir().ok();
    let home = repo.config.home_dir();
    let context = crate::config::cache::interpolate_context(install_dir.as_deref(), home.as_deref());
    let path = gix_config::Path {
        value: key,
        is_optional: false,
    }
    .interpolate(context)
    .or_raise(|| message("Could not interpolate the path of the commit-signing key"))?;
    Ok(path.into_os_string())
}

fn default_ssh_key(config: &crate::config::Snapshot<'_>) -> Result<Option<OsString>> {
    let Some(program) = config.program(gpg::Ssh::DEFAULT_KEY_COMMAND)? else {
        return Ok(None);
    };
    let mut command: std::process::Command = gix_command::prepare(&program)
        .command_may_be_shell_script()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .try_into()?;
    let program_display = if program == command.get_program() {
        String::new()
    } else {
        format!(" {}", program.display())
    };
    let output = command
        .spawn()
        .or_raise(|| {
            message!("Could not execute gpg.ssh.defaultKeyCommand{program_display}").with_program(command.get_program())
        })?
        .wait_with_output()
        .or_raise(|| {
            message!("Could not execute gpg.ssh.defaultKeyCommand{program_display}").with_program(command.get_program())
        })?;
    if !output.status.success() {
        bail!(message("gpg.ssh.defaultKeyCommand failed").with_command_output(&command, output));
    }
    let key = output.stdout.as_bstr().lines().next().unwrap_or_default().trim();
    if is_literal_ssh_key(key).is_none() {
        bail!("gpg.ssh.defaultKeyCommand returned an invalid key: {key:?}");
    }
    Ok(Some(gix_path::from_bstr(key.as_bstr())?.into_owned().into_os_string()))
}
