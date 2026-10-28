//! Low-level primitives to update references on a remote and send the objects they need, as `git send-pack` does.
//!
//! ### Order of operations
//!
//! * [handshake](crate::handshake()) with [`Service::ReceivePack`](crate::transport::Service::ReceivePack), asking for
//!   protocol version 0 or 1 as `git receive-pack` doesn't support version 2.
//! * obtain the [references of the remote](crate::push::RemoteRefs::from_handshake()).
//! * decide which [commands](crate::push::Command) to send, for instance by matching push refspecs with
//!   `gix_refspec::MatchGroup::match_push()`.
//! * [push](crate::push()), writing a pack with the objects the remote lacks if references are created or updated,
//!   and obtain the status of each reference in the [`Outcome`](crate::push::Outcome).
//!
//! Pushing ends the interaction, as `git receive-pack` exits after reporting.
use bstr::BString;
use gix_error::{Result, bail};
use gix_hash::ObjectId;

use crate::handshake::Ref;

mod arguments;
pub use arguments::{Arguments, ReportStatus};

///
pub mod response;
pub use response::Outcome;

#[cfg(any(feature = "blocking-client", feature = "async-client"))]
pub(crate) mod function;

/// An update of a reference on the remote, sent as `<old-id> <new-id> <name>` like `git send-pack` sends it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Command {
    /// The full name of the reference on the remote, like `refs/heads/main`.
    pub name: BString,
    /// The object the reference on the remote points to, or the null id if the remote doesn't have the reference.
    pub old_id: ObjectId,
    /// The object the reference should point to, or the null id to delete it.
    pub new_id: ObjectId,
}

impl Command {
    /// Return `true` if the reference is to be deleted.
    pub fn is_delete(&self) -> bool {
        self.new_id.is_null()
    }
}

/// Options for use in [`push()`](crate::push()).
#[derive(Debug, Clone)]
pub struct Options {
    /// The kind of hash the pushing repository uses, which the remote has to support.
    pub object_hash: gix_hash::Kind,
    /// If `true`, update all references or none of them, as `git push --atomic` does, which requires the `atomic`
    /// capability.
    pub atomic: bool,
    /// The strings to pass to the hooks of the remote, as `git push --push-option` does, which requires the
    /// `push-options` capability.
    pub push_options: Vec<BString>,
    /// If `true`, ask the remote not to send progress with the `quiet` capability, as Git does when it doesn't show
    /// progress.
    pub quiet: bool,
}

/// For use in [`push()`](crate::push()).
pub struct Context<'a, T> {
    /// The outcome of the handshake with `git receive-pack`, providing the capabilities of the remote.
    pub handshake: &'a crate::Handshake,
    /// The transport the handshake was performed with.
    pub transport: &'a mut T,
    /// How to self-identify with the `agent` capability if the remote supports it.
    ///
    /// This could be read from the `gitoxide.userAgent` configuration variable.
    pub user_agent: crate::command::Feature,
    /// If `true`, output all packetlines using the `gix-trace` machinery.
    pub trace_packetlines: bool,
}

/// The references a remote advertised before receiving a push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoteRefs {
    /// The references of the remote.
    pub refs: Vec<Ref>,
    /// Objects the remote has without naming a reference for them, advertised as `.have`, usually as they are
    /// referenced in the alternate object databases of the remote.
    ///
    /// Like the objects of [`refs`](Self::refs), they need not be sent.
    pub extra_have: Vec<ObjectId>,
}

impl RemoteRefs {
    /// Obtain the references the remote advertised in `handshake` as `get_remote_heads()` in Git's `connect.c` does.
    ///
    /// Fail if the remote responded with protocol version 2, which `git receive-pack` doesn't support.
    pub fn from_handshake(handshake: &crate::Handshake) -> Result<Self> {
        match (handshake.server_protocol_version, &handshake.refs) {
            (gix_transport::Protocol::V0 | gix_transport::Protocol::V1, Some(refs)) => {
                Ok(Self::from_refs(refs.iter().cloned()))
            }
            _ => bail!(gix_error::unsupported("support for protocol v2 not implemented yet")),
        }
    }

    /// Separate the objects named by `.have` lines from the references in `refs`.
    ///
    /// The `capabilities^{}` line advertising the capabilities of an empty repository names no reference and isn't
    /// part of `refs` anymore.
    pub fn from_refs(refs: impl IntoIterator<Item = Ref>) -> Self {
        let mut out = RemoteRefs::default();
        for r in refs {
            match r {
                Ref::Direct { full_ref_name, object } if full_ref_name == ".have" => out.extra_have.push(object),
                r => out.refs.push(r),
            }
        }
        out
    }
}

/// The progress ids used during a push.
///
/// Use this information to selectively extract the progress of interest in case the parent application has custom
/// visualization.
#[derive(Debug, Copy, Clone)]
pub enum ProgressId {
    /// The progress name is defined by the remote and the progress messages it sets, along with their progress values
    /// and limits.
    RemoteProgress,
}

impl From<ProgressId> for gix_utils::progress::Id {
    fn from(v: ProgressId) -> Self {
        match v {
            ProgressId::RemoteProgress => *b"PURP",
        }
    }
}
