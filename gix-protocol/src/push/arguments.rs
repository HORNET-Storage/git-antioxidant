use bstr::{BString, ByteVec};
use gix_error::{Result, bail};
use gix_transport::client::Capabilities;

use crate::push::{Command, Options};

/// The status report a remote sends after receiving the commands of a push.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ReportStatus {
    /// The remote doesn't report, so the references it received commands for are assumed to be updated.
    None,
    /// `report-status`: whether the remote unpacked the objects, and for each reference whether it was updated.
    V1,
    /// `report-status-v2`: like `report-status`, along with `option` lines describing updates that differ from the
    /// commands.
    V2,
}

/// The capabilities to use for a push, chosen from those the remote advertised as `send_pack()` in Git's
/// `send-pack.c` chooses them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arguments {
    /// The status report to ask for.
    pub report_status: ReportStatus,
    /// If `true`, the remote multiplexes its report with progress and errors using `side-band-64k`.
    pub side_band: bool,
    /// If `true`, ask the remote not to send progress with `quiet`.
    pub quiet: bool,
    /// If `true`, the remote updates all references or none of them, using `atomic`.
    pub atomic: bool,
    /// The push options to send after the commands, using `push-options` if there are any.
    pub push_options: Vec<BString>,
    /// The kind of hash of the pushing repository, sent with `object-format` if the remote advertised the formats
    /// it supports.
    pub object_format: Option<gix_hash::Kind>,
    /// The name to identify as with `agent`, if the remote advertised `agent`.
    pub agent: Option<String>,
    /// If `true`, the remote supports deleting references with `delete-refs`.
    pub delete_refs: bool,
    /// If `true`, the pack may contain deltas whose base is given by its offset in the pack, as the remote advertised
    /// `ofs-delta`.
    pub ofs_delta: bool,
    /// If `true`, the pack may contain deltas against objects the remote has without containing them, as the remote
    /// didn't advertise `no-thin`.
    pub thin_pack: bool,
}

impl Arguments {
    /// Choose the capabilities to use for pushing with `options` from the `capabilities` the remote advertised,
    /// identifying as `user_agent` if the remote supports it.
    ///
    /// Fail as `send_pack()` does if the remote doesn't support the kind of hash of the pushing repository, an atomic
    /// push, or push options when `options` ask for them.
    pub fn new(capabilities: &Capabilities, user_agent: &crate::command::Feature, options: &Options) -> Result<Self> {
        let object_format = object_format(capabilities, options.object_hash)?;
        if options.atomic && !capabilities.contains("atomic") {
            bail!(gix_error::unsupported(
                "the receiving end does not support --atomic push"
            ));
        }
        if !options.push_options.is_empty() && !capabilities.contains("push-options") {
            bail!(gix_error::unsupported(
                "the receiving end does not support push options"
            ));
        }
        Ok(Arguments {
            report_status: if capabilities.contains("report-status-v2") {
                ReportStatus::V2
            } else if capabilities.contains("report-status") {
                ReportStatus::V1
            } else {
                ReportStatus::None
            },
            side_band: capabilities.contains("side-band-64k"),
            quiet: options.quiet && capabilities.contains("quiet"),
            atomic: options.atomic,
            push_options: options.push_options.clone(),
            object_format,
            agent: capabilities
                .contains("agent")
                .then(|| user_agent.1.as_deref().map(sanitized_agent))
                .flatten(),
            delete_refs: capabilities.contains("delete-refs"),
            ofs_delta: capabilities.contains("ofs-delta"),
            thin_pack: !capabilities.contains("no-thin"),
        })
    }

    /// The capabilities to send after the first command, each preceded by a space, in the order Git sends them.
    pub fn capabilities(&self) -> BString {
        let mut out = BString::default();
        match self.report_status {
            ReportStatus::None => {}
            ReportStatus::V1 => out.push_str(" report-status"),
            ReportStatus::V2 => out.push_str(" report-status-v2"),
        }
        if self.side_band {
            out.push_str(" side-band-64k");
        }
        if self.quiet {
            out.push_str(" quiet");
        }
        if self.atomic {
            out.push_str(" atomic");
        }
        if !self.push_options.is_empty() {
            out.push_str(" push-options");
        }
        if let Some(kind) = self.object_format {
            out.push_str(format!(" object-format={kind}"));
        }
        if let Some(agent) = &self.agent {
            out.push_str(format!(" agent={agent}"));
        }
        out
    }

    /// The line to send for `command`, which carries the [capabilities](Self::capabilities()) behind a null byte if
    /// it `is_first` command.
    pub fn command_line(&self, command: &Command, is_first: bool) -> BString {
        let mut line: BString = format!("{} {} ", command.old_id, command.new_id).into();
        line.push_str(&command.name);
        if is_first {
            line.push_byte(0);
            line.push_str(self.capabilities());
        }
        line
    }
}

/// Return the kind of hash to announce with `object-format` if the remote advertised the formats it supports, and fail
/// if `object_hash` isn't among them, as `server_supports_hash()` in Git's `connect.c` does. A remote that doesn't
/// advertise formats supports only SHA-1.
fn object_format(capabilities: &Capabilities, object_hash: gix_hash::Kind) -> Result<Option<gix_hash::Kind>> {
    let name = object_hash.to_string();
    let advertised: Vec<_> = capabilities
        .iter()
        .filter(|c| c.name() == "object-format")
        .map(|c| c.value().unwrap_or_default())
        .collect();
    let supported = if advertised.is_empty() {
        name == "sha1"
    } else {
        advertised.iter().any(|format| *format == name.as_bytes())
    };
    if !supported {
        bail!(gix_error::unsupported(
            "the receiving end does not support this repository's hash algorithm"
        ));
    }
    Ok((!advertised.is_empty()).then_some(object_hash))
}

/// Replace whitespace and other bytes that aren't printable ASCII with dots, as `git_user_agent_sanitized()` in Git's
/// `version.c` does, so the agent remains a single capability.
fn sanitized_agent(agent: &str) -> String {
    agent
        .trim()
        .as_bytes()
        .iter()
        .map(|b| if b.is_ascii_graphic() { *b as char } else { '.' })
        .collect()
}
