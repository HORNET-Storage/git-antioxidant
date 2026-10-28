use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use bstr::{BString, ByteSlice};
use gix_error::{Result, ResultExt, bail, message};
use gix_utils::progress::DynNestedProgress;

#[crate::bisync::only_async]
use crate::futures_lite::AsyncWriteExt;
use crate::push::{
    Arguments, Command, Context, Options, Outcome, ProgressId, ReportStatus,
    response::{self, RefStatus, Status},
};
#[crate::bisync::only_async]
use crate::transport::{
    client::async_io::{ExtendedBufRead, HandleProgress, ReadlineBufRead, Transport},
    packetline::async_io::StreamingPeekableIter,
};
#[crate::bisync::only_sync]
use crate::transport::{
    client::blocking_io::{ExtendedBufRead, HandleProgress, ReadlineBufRead, Transport},
    packetline::blocking_io::StreamingPeekableIter,
};
use crate::transport::{
    client::{MessageKind, WriteMode},
    packetline::PacketLineRef,
};
#[crate::bisync::only_sync]
use std::io::Write;

/// Update the references of the remote on the other end of `transport` as `commands` ask, as `send_pack()` in Git's
/// `send-pack.c` does, and return what happened to each reference along with whether the remote unpacked the objects.
///
/// `write_pack(writer, arguments, progress, should_interrupt)` is called if a command creates or updates a reference
/// to write a pack to `writer` with the objects the remote needs for it: the new objects of the commands and the
/// objects they reference, except for those the remote has as they are reachable from its references or the objects
/// it advertised (see [`RemoteRefs`](crate::push::RemoteRefs)). `arguments` tell which kinds of deltas the pack may
/// contain.
///
/// `progress` and `should_interrupt` are passed to all potentially long-running parts of the operation, and the
/// progress messages of the remote are passed to a child of `progress`.
///
/// No command is sent for references that already point to their new object, nor to delete a reference if the remote
/// doesn't support it, which fails an atomic push without sending any command.
///
/// Pushing ends the interaction with the remote.
#[crate::bisync::bisync]
pub async fn push<P, T>(
    commands: &[Command],
    write_pack: impl FnOnce(&mut dyn std::io::Write, &Arguments, &mut dyn DynNestedProgress, &AtomicBool) -> Result<()>,
    mut progress: P,
    should_interrupt: &AtomicBool,
    Context {
        handshake,
        transport,
        user_agent,
        trace_packetlines,
    }: Context<'_, T>,
    options: &Options,
) -> Result<Outcome>
where
    P: gix_utils::progress::NestedProgress,
    P::SubProgress: 'static,
    T: Transport,
{
    let _span = gix_trace::coarse!("gix_protocol::push()");
    if !matches!(
        handshake.server_protocol_version,
        gix_transport::Protocol::V0 | gix_transport::Protocol::V1
    ) {
        bail!(gix_error::unsupported("support for protocol v2 not implemented yet"));
    }
    let arguments = Arguments::new(&handshake.capabilities, &user_agent, options)?;
    let mut refs: Vec<_> = commands
        .iter()
        .map(|command| RefStatus {
            name: command.name.clone(),
            status: if command.is_delete() && !arguments.delete_refs {
                Status::DeleteUnsupported
            } else if !command.is_delete() && command.old_id == command.new_id {
                Status::UpToDate
            } else if arguments.report_status == ReportStatus::None {
                Status::Ok
            } else {
                Status::NotReported
            },
            reports: Vec::new(),
        })
        .collect();
    if arguments.atomic && refs.iter().any(|r| r.status == Status::DeleteUnsupported) {
        for r in refs.iter_mut().filter(|r| is_to_be_sent(&r.status)) {
            r.status = Status::AtomicPushFailed;
        }
    }
    let to_send: Vec<_> = commands
        .iter()
        .zip(&refs)
        .filter_map(|(command, r)| is_to_be_sent(&r.status).then_some(command))
        .collect();
    if to_send.is_empty() {
        crate::indicate_end_of_interaction(&mut *transport, trace_packetlines)
            .await
            .or_raise(|| message("Failed to end the interaction with the remote"))?;
        return Ok(Outcome { unpack: None, refs });
    }
    if should_interrupt.load(Ordering::Relaxed) {
        bail!("Interrupted before sending commands to the remote".cancelled());
    }

    // The progress handler of the response refers to it, so it has to outlive the request.
    let remote_error = RefCell::new(None::<BString>);
    let mut request = transport
        .request(WriteMode::Binary, MessageKind::Flush, trace_packetlines)
        .or_raise(|| message("Failed to start sending commands to the remote"))?;
    for (index, command) in to_send.iter().enumerate() {
        request
            .write_all(&arguments.command_line(command, index == 0))
            .await
            .or_raise(|| message("Failed to send a command to the remote"))?;
    }
    request
        .write_message(MessageKind::Flush)
        .await
        .or_raise(|| message("Failed to send a command to the remote"))?;
    if !arguments.push_options.is_empty() {
        for option in &arguments.push_options {
            request
                .write_all(option)
                .await
                .or_raise(|| message("Failed to send a push option to the remote"))?;
        }
        request
            .write_message(MessageKind::Flush)
            .await
            .or_raise(|| message("Failed to send a push option to the remote"))?;
    }

    let (writer, mut reader) = request.into_parts();
    let mut writer = if to_send.iter().any(|command| !command.is_delete()) {
        progress.set_name("sending pack".into());
        write_pack_to(writer, write_pack, &arguments, &mut progress, should_interrupt)?
    } else {
        writer
    };
    writer
        .flush()
        .await
        .or_raise(|| message("Failed to send the request to the remote"))?;
    // Some transports only send the request once its writer is dropped.
    drop(writer);

    progress.set_name("reading status report".into());
    if arguments.side_band {
        setup_remote_progress(&mut progress, &mut reader, should_interrupt, &remote_error);
    }
    let mut lines = Vec::new();
    if arguments.report_status != ReportStatus::None {
        if arguments.side_band {
            // The report consists of packet lines which are sent as data of the first band.
            let mut report = StreamingPeekableIter::new(&mut reader, &[PacketLineRef::Flush], trace_packetlines);
            while let Some(line) = report.read_line().await {
                lines.push(report_line(line, &remote_error)?);
            }
        } else {
            while let Some(line) = reader.readline().await {
                lines.push(report_line(line, &remote_error)?);
            }
        }
    }
    // A multiplexed response ends with a flush packet, while nothing follows a report sent without side-band.
    let remaining = if arguments.side_band {
        read_remaining(&mut reader).await
    } else {
        Ok(())
    };
    if let Some(remote_error) = remote_error.take() {
        bail!("remote error: {remote_error}");
    }
    remaining.or_raise(|| message("Failed to read the end of the response of the remote"))?;

    let unpack = if arguments.report_status == ReportStatus::None {
        None
    } else {
        Some(response::parse_report(&lines, &mut refs, options.object_hash)?)
    };
    Ok(Outcome { unpack, refs })
}

/// Return `true` if a command is sent for a reference that has `status` before sending.
fn is_to_be_sent(status: &Status) -> bool {
    matches!(status, Status::Ok | Status::NotReported)
}

/// Return the content of a `line` of the status report without its trailing newline, or fail with the error the remote
/// sent instead, as Git's `packet_read_with_status()` does with `PACKET_READ_CHOMP_NEWLINE | PACKET_READ_DIE_ON_ERR_PACKET`.
fn report_line(
    line: std::io::Result<Result<PacketLineRef<'_>>>,
    remote_error: &RefCell<Option<BString>>,
) -> Result<BString> {
    let line = match line {
        Ok(line) => line,
        Err(err) => {
            if let Some(remote_error) = remote_error.take() {
                bail!("remote error: {remote_error}");
            }
            // The transport fails on `ERR` lines it reads on its own.
            if let Some(err) = err
                .get_ref()
                .and_then(|err| err.downcast_ref::<crate::transport::packetline::read::Error>())
            {
                let message = err.message.strip_suffix(b"\n").unwrap_or(&err.message);
                bail!("remote error: {}", message.as_bstr());
            }
            return Err(err).or_raise(|| message("Failed to read the status report of the remote"));
        }
    };
    let line = line.or_raise(|| message("Failed to decode the status report of the remote"))?;
    let Some(data) = line.as_slice() else {
        bail!("expected a line of the status report, got {line:?}".corrupted());
    };
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    if let Some(remote_error) = data.strip_prefix(b"ERR ") {
        bail!("remote error: {}", remote_error.as_bstr());
    }
    Ok(data.into())
}

/// Pass progress messages of the remote, which are sent on the second band, to a child of `progress`, and stop reading
/// with the error sent on the third band after storing it in `remote_error`, as `recv_sideband()` in Git does.
fn setup_remote_progress<'a>(
    progress: &mut dyn DynNestedProgress,
    reader: &mut Box<dyn ExtendedBufRead<'a> + Unpin + 'a>,
    should_interrupt: &'a AtomicBool,
    remote_error: &'a RefCell<Option<BString>>,
) {
    reader.set_progress_handler(Some(Box::new({
        let mut remote_progress = progress.add_child_with_id("remote".to_string(), ProgressId::RemoteProgress.into());
        move |is_err: bool, data: &[u8]| {
            if is_err {
                *remote_error.borrow_mut() = Some(data.trim_end().into());
                return std::ops::ControlFlow::Break(());
            }
            crate::RemoteProgress::translate_to_progress(false, data, &mut remote_progress);
            if should_interrupt.load(Ordering::Relaxed) {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        }
    }) as HandleProgress<'a>));
}

#[crate::bisync::only_async]
fn write_pack_to<W>(
    writer: W,
    write_pack: impl FnOnce(&mut dyn std::io::Write, &Arguments, &mut dyn DynNestedProgress, &AtomicBool) -> Result<()>,
    arguments: &Arguments,
    progress: &mut dyn DynNestedProgress,
    should_interrupt: &AtomicBool,
) -> Result<W>
where
    W: crate::futures_io::AsyncWrite + Unpin,
{
    let mut writer = crate::futures_lite::io::BlockOn::new(writer);
    write_pack(&mut writer, arguments, progress, should_interrupt)
        .or_raise(|| message("Failed to send the pack to the remote"))?;
    Ok(writer.into_inner())
}

#[crate::bisync::only_sync]
fn write_pack_to<W>(
    mut writer: W,
    write_pack: impl FnOnce(&mut dyn std::io::Write, &Arguments, &mut dyn DynNestedProgress, &AtomicBool) -> Result<()>,
    arguments: &Arguments,
    progress: &mut dyn DynNestedProgress,
    should_interrupt: &AtomicBool,
) -> Result<W>
where
    W: std::io::Write,
{
    write_pack(&mut writer, arguments, progress, should_interrupt)
        .or_raise(|| message("Failed to send the pack to the remote"))?;
    Ok(writer)
}

#[crate::bisync::only_async]
async fn read_remaining(reader: &mut (impl crate::futures_io::AsyncRead + Unpin)) -> std::io::Result<()> {
    crate::futures_lite::io::copy(reader, &mut crate::futures_lite::io::sink())
        .await
        .map(|_| ())
}

#[crate::bisync::only_sync]
fn read_remaining(reader: &mut impl std::io::Read) -> std::io::Result<()> {
    std::io::copy(reader, &mut std::io::sink()).map(|_| ())
}
