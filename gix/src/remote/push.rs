//! The types of pushing references and the objects they need to a remote, as `git push` does.
//!
//! Find the remote to push to with [`Repository::find_push_remote()`](crate::Repository::find_push_remote()), connect to it
//! with `Remote::connect(Direction::Push)`, learn which references would be updated with `Connection::prepare_push()`
//! and send them with `Prepare::push()`, which the `blocking-network-client` and `async-network-client` features
//! provide.
#[cfg(any(feature = "blocking-network-client", feature = "async-network-client"))]
pub use super::connection::push::{Expected, Lease, Options, Outcome, Prepare, Rejection, Source, Status, Update};

use crate::{
    Remote, Result,
    bstr::{BStr, ByteSlice},
    error::{ResultExt, bail, message, not_found},
    remote,
};

/// The message of `git push` when it finds no remote to push to.
const NO_PUSH_DESTINATION: &str = "No configured push destination.\nEither specify the URL from the command-line or configure a remote repository using\n\n    git remote add <name> <url>\n\nand then push using the remote name\n\n    git push <name>";

impl crate::Repository {
    /// Find the remote to push to as `git push [<name_or_url>]` does.
    ///
    /// With `name_or_url` being `Some`, use the configured remote of that name, or a new remote in memory if it is a URL.
    ///
    /// If `name_or_url` is `None`, take the name of the remote from `branch.<name>.pushRemote` of the branch `HEAD` points to,
    /// then from `remote.pushDefault`, then from `branch.<name>.remote`, and finally use the only configured remote,
    /// or `origin` if it is configured among several. A name taken from the configuration may also be a URL.
    ///
    /// Fail with the message of `git push` if no remote is found.
    pub fn find_push_remote(&self, name_or_url: Option<&BStr>) -> Result<Remote<'_>> {
        let name = match name_or_url {
            Some(name) => Some(name.to_owned()),
            None => {
                let head = self.head_name()?;
                head.as_ref()
                    .and_then(|head| head.as_bstr().strip_prefix(b"refs/heads/"))
                    .and_then(|branch| self.branch_remote_name(branch.as_bstr(), remote::Direction::Push))
                    .map(|name| name.as_bstr().to_owned())
                    .or_else(|| self.remote_default_name(remote::Direction::Push))
            }
        };
        let Some(name) = name else {
            bail!(not_found(NO_PUSH_DESTINATION));
        };
        match self.try_find_remote(name.as_bstr()).transpose()? {
            Some(remote) => Ok(remote),
            None => self.remote_at(gix_url::parse(name.as_bstr()).or_raise(|| message!("bad repository '{name}'"))?),
        }
    }
}
