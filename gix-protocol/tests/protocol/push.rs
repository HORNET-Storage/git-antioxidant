use std::sync::atomic::AtomicBool;

use bstr::{BString, ByteSlice};
use gix_error::{ResultExt, message};
use gix_hash::ObjectId;
use gix_protocol::push::{
    Command, Context, Options, Outcome, RemoteRefs,
    response::{RefStatus, Report, Status, Unpack},
};
use gix_transport::Protocol;
use gix_utils::progress;

use crate::TestResult;

#[cfg(feature = "blocking-client")]
type Cursor = std::io::Cursor<Vec<u8>>;
#[cfg(all(feature = "async-client", not(feature = "blocking-client")))]
type Cursor = futures_lite::io::Cursor<Vec<u8>>;

/// The pushes recorded by `push.sh`.
const RECORDED: &[&str] = &[
    "create",
    "update",
    "delete-only",
    "rejected",
    "atomic",
    "push-options",
    "alternates",
];

/// What `git receive-pack` advertises by default.
const CAPABILITIES: &str = "report-status report-status-v2 delete-refs side-band-64k quiet atomic ofs-delta object-format=sha1 agent=git/2.55.0";

fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.as_bytes()).expect("valid hex")
}

fn null() -> ObjectId {
    gix_hash::Kind::Sha1.null()
}

fn command(name: &str, old_id: ObjectId, new_id: ObjectId) -> Command {
    Command {
        name: name.into(),
        old_id,
        new_id,
    }
}

fn options(object_hash: gix_hash::Kind) -> Options {
    Options {
        object_hash,
        atomic: false,
        push_options: Vec::new(),
        quiet: true,
    }
}

fn agent() -> gix_protocol::command::Feature {
    ("agent", Some("git/gix-fixture".into()))
}

/// Encode `data` as packet line.
fn pkt(data: impl AsRef<[u8]>) -> Vec<u8> {
    let data = data.as_ref();
    let mut line = format!("{:04x}", data.len() + 4).into_bytes();
    line.extend_from_slice(data);
    line
}

/// Encode `data` as packet line of `band`, as sent with `side-band-64k`.
fn band(band: u8, data: impl AsRef<[u8]>) -> Vec<u8> {
    let mut payload = vec![band];
    payload.extend_from_slice(data.as_ref());
    pkt(payload)
}

/// The advertisement of `git receive-pack` with `capabilities` and `refs` as `(id, name)`.
fn advertisement(capabilities: &str, refs: &[(ObjectId, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    if refs.is_empty() {
        out.extend(pkt(format!("{} capabilities^{{}}\0{capabilities}\n", null())));
    }
    for (index, (id, name)) in refs.iter().enumerate() {
        out.extend(pkt(if index == 0 {
            format!("{id} {name}\0{capabilities}\n")
        } else {
            format!("{id} {name}\n")
        }));
    }
    out.extend_from_slice(b"0000");
    out
}

/// Return the payloads of the packet lines in `data` before the next flush packet, and what follows it.
fn lines_until_flush(mut data: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut lines = Vec::new();
    loop {
        let len = std::str::from_utf8(&data[..4])
            .ok()
            .and_then(|hex| usize::from_str_radix(hex, 16).ok())
            .expect("a packet line starts with its length");
        if len == 0 {
            return (lines, &data[4..]);
        }
        lines.push(&data[4..len]);
        data = &data[len..];
    }
}

/// What `git push` sent to `git receive-pack`.
struct Request {
    commands: Vec<Command>,
    /// The capabilities sent after the first command.
    capabilities: BString,
    push_options: Vec<BString>,
    pack: Vec<u8>,
}

fn parse_request(request: &[u8]) -> Request {
    let (lines, rest) = lines_until_flush(request);
    let mut capabilities = BString::default();
    let mut commands = Vec::new();
    for line in lines {
        let line = match line.split_once_str(b"\0") {
            Some((line, first_capabilities)) => {
                capabilities = first_capabilities.into();
                line
            }
            None => line,
        };
        let mut fields = line.splitn_str(3, " ");
        let mut field = || fields.next().expect("commands have three fields");
        let old_id = ObjectId::from_hex(field()).expect("old id");
        let new_id = ObjectId::from_hex(field()).expect("new id");
        commands.push(Command {
            name: field().into(),
            old_id,
            new_id,
        });
    }
    let (push_options, pack) = if capabilities.contains_str("push-options") {
        let (push_options, pack) = lines_until_flush(rest);
        (push_options.into_iter().map(BString::from).collect(), pack)
    } else {
        (Vec::new(), rest)
    };
    Request {
        commands,
        capabilities,
        push_options,
        pack: pack.to_vec(),
    }
}

/// Assert that `outcome` agrees with what `git push --porcelain` reported in `porcelain` for the push `name`.
fn assert_agrees_with_git(name: &str, outcome: &Outcome, porcelain: &[u8]) {
    let mut reported = 0;
    for line in porcelain.lines() {
        let fields: Vec<_> = line.split_str("\t").collect();
        let [flag, refs, summary] = fields[..] else {
            continue;
        };
        let (_, destination) = refs.rsplit_once_str(":").expect("source:destination");
        let status = &outcome
            .refs
            .iter()
            .find(|r| r.name == destination)
            .unwrap_or_else(|| panic!("{name}: {} was pushed", destination.as_bstr()))
            .status;
        match status {
            Status::Ok => assert_ne!(flag, b"!", "{name}: Git updated {}", destination.as_bstr()),
            Status::Rejected { reason } => {
                assert_eq!(flag, b"!", "{name}: Git didn't update {}", destination.as_bstr());
                assert_eq!(
                    summary.as_bstr(),
                    format!("[remote rejected] ({reason})").as_bytes().as_bstr(),
                    "{name}: the remote gave the same reason"
                );
            }
            other => panic!("{name}: Git doesn't report {other:?} after pushing"),
        }
        reported += 1;
    }
    assert_eq!(
        reported,
        outcome.refs.len(),
        "{name}: Git reports about every reference"
    );
}

#[cfg(feature = "blocking-client")]
fn transport(
    response: Vec<u8>,
    out: &mut Vec<u8>,
) -> gix_transport::client::git::blocking_io::Connection<Cursor, &mut Vec<u8>> {
    gix_transport::client::git::blocking_io::Connection::new(
        Cursor::new(response),
        out,
        Protocol::V0,
        b"does/not/matter".as_bstr().to_owned(),
        None::<(&str, _)>,
        gix_transport::client::git::ConnectMode::Process,
        false,
    )
}

#[cfg(all(feature = "async-client", not(feature = "blocking-client")))]
fn transport(
    response: Vec<u8>,
    out: &mut Vec<u8>,
) -> gix_transport::client::git::async_io::Connection<Cursor, &mut Vec<u8>> {
    gix_transport::client::git::async_io::Connection::new(
        Cursor::new(response),
        out,
        Protocol::V0,
        b"does/not/matter".as_bstr().to_owned(),
        None::<(&str, _)>,
        gix_transport::client::git::ConnectMode::Process,
        false,
    )
}

/// Perform a handshake and push `commands` with `options` to a remote sending `response`, writing `pack` if a pack
/// is needed. Return what was sent to the remote, the references it advertised and the outcome.
#[crate::bisync::bisync]
async fn push_to(
    response: Vec<u8>,
    commands: &[Command],
    pack: &[u8],
    options: &Options,
) -> gix_error::Result<(Vec<u8>, RemoteRefs, Outcome)> {
    let mut sent = Vec::new();
    let mut transport = transport(response, &mut sent);
    let handshake = gix_protocol::handshake(
        &mut transport,
        gix_transport::Service::ReceivePack,
        |_| Ok(None),
        Vec::new(),
        &mut progress::Discard,
    )
    .await?;
    let remote_refs = RemoteRefs::from_handshake(&handshake)?;
    let outcome = gix_protocol::push(
        commands,
        |writer, _arguments, _progress, _should_interrupt| {
            writer.write_all(pack).or_raise(|| message("Could not write the pack"))
        },
        progress::Discard,
        &AtomicBool::default(),
        Context {
            handshake: &handshake,
            transport: &mut transport,
            user_agent: agent(),
            trace_packetlines: false,
        },
        options,
    )
    .await?;
    drop(transport);
    Ok((sent, remote_refs, outcome))
}

#[crate::bisync::bisync]
#[cfg_attr(feature = "blocking-client", test)]
#[cfg_attr(all(feature = "async-client", not(feature = "blocking-client")), async_std::test)]
async fn recorded_pushes_send_and_understand_what_git_does() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only("push.sh")?;
    for name in RECORDED {
        let case = dir.join(name);
        let request = std::fs::read(case.join("request"))?;
        let Request {
            commands,
            capabilities,
            push_options,
            pack,
        } = parse_request(&request);
        let options = Options {
            object_hash: gix_testtools::object_hash(),
            atomic: capabilities.contains_str(" atomic"),
            push_options,
            quiet: true,
        };
        let (sent, remote_refs, outcome) =
            push_to(std::fs::read(case.join("response"))?, &commands, &pack, &options).await?;
        assert_eq!(
            sent.as_bstr(),
            request.as_bstr(),
            "{name}: the request is the one of Git"
        );
        assert_eq!(
            outcome.unpack,
            Some(Unpack::Ok),
            "{name}: the remote unpacked the objects"
        );
        assert_agrees_with_git(name, &outcome, &std::fs::read(case.join("porcelain"))?);
        match *name {
            "create" => assert_eq!(
                remote_refs,
                RemoteRefs::default(),
                "the advertisement of an empty repository names no reference"
            ),
            "alternates" => {
                assert!(remote_refs.refs.is_empty(), "the remote has no reference of its own");
                assert_eq!(
                    remote_refs.extra_have.len(),
                    2,
                    "the distinct objects of the references of its alternate are advertised with `.have`"
                );
            }
            _ => assert!(remote_refs.extra_have.is_empty(), "{name}: there is no alternate"),
        }
    }
    Ok(())
}

#[crate::bisync::bisync]
#[cfg_attr(feature = "blocking-client", test)]
#[cfg_attr(all(feature = "async-client", not(feature = "blocking-client")), async_std::test)]
async fn report_status_v2_tells_what_the_remote_did_instead() -> TestResult {
    let (main, topic) = (
        oid("1111111111111111111111111111111111111111"),
        oid("2222222222222222222222222222222222222222"),
    );
    let mut report = Vec::new();
    for line in [
        "unpack ok".to_string(),
        "ok refs/for/main".into(),
        "option refname refs/heads/topic".into(),
        format!("option old-oid {topic}"),
        format!("option new-oid {main}"),
        "option forced-update".into(),
        "ok refs/for/main".into(),
        "option refname refs/heads/other".into(),
        "ng refs/heads/topic hook declined".into(),
    ] {
        report.extend(pkt(format!("{line}\n")));
    }
    report.extend_from_slice(b"0000");
    let mut response = advertisement(CAPABILITIES, &[(topic, "refs/heads/topic")]);
    response.extend(band(2, "Processing references\n"));
    // The data of the first band doesn't respect the boundaries of the packet lines it carries.
    let (first, second) = report.split_at(7);
    response.extend(band(1, first));
    response.extend(band(1, second));
    response.extend_from_slice(b"0000");

    let commands = [
        command("refs/for/main", null(), main),
        command("refs/heads/topic", topic, main),
    ];
    let (sent, _, outcome) = push_to(response, &commands, b"pack", &options(gix_hash::Kind::Sha1)).await?;
    let mut expected = pkt(format!(
        "{} {main} refs/for/main\0 report-status-v2 side-band-64k quiet object-format=sha1 agent=git/gix-fixture",
        null()
    ));
    expected.extend(pkt(format!("{topic} {main} refs/heads/topic")));
    expected.extend_from_slice(b"0000pack");
    assert_eq!(
        sent.as_bstr(),
        expected.as_bstr(),
        "the capabilities follow the first command, and the pack follows the commands"
    );
    assert_eq!(
        outcome,
        Outcome {
            unpack: Some(Unpack::Ok),
            refs: vec![
                RefStatus {
                    name: "refs/for/main".into(),
                    status: Status::Ok,
                    reports: vec![
                        Report {
                            name: Some("refs/heads/topic".into()),
                            old_id: Some(topic),
                            new_id: Some(main),
                            forced_update: true,
                        },
                        Report {
                            name: Some("refs/heads/other".into()),
                            ..Default::default()
                        },
                    ],
                },
                RefStatus {
                    name: "refs/heads/topic".into(),
                    status: Status::Rejected {
                        reason: "hook declined".into(),
                    },
                    reports: Vec::new(),
                },
            ],
        }
    );
    assert!(!outcome.is_success(), "a rejected reference fails the push");
    Ok(())
}

#[crate::bisync::bisync]
#[cfg_attr(feature = "blocking-client", test)]
#[cfg_attr(all(feature = "async-client", not(feature = "blocking-client")), async_std::test)]
async fn errors_sent_by_the_remote_fail_the_push() -> TestResult {
    let main = oid("1111111111111111111111111111111111111111");
    let commands = [command("refs/heads/main", null(), main)];

    let mut response = advertisement(CAPABILITIES, &[]);
    response.extend(band(2, "Unpacking objects\n"));
    response.extend(band(3, "out of space\n"));
    let err = push_to(response, &commands, b"pack", &options(gix_hash::Kind::Sha1))
        .await
        .expect_err("the third band carries errors");
    assert_eq!(err.to_string(), "remote error: out of space");

    let mut response = advertisement("report-status delete-refs ofs-delta", &[]);
    response.extend(pkt("ERR denied by policy\n"));
    let err = push_to(response, &commands, b"pack", &options(gix_hash::Kind::Sha1))
        .await
        .expect_err("ERR lines carry errors");
    assert_eq!(err.to_string(), "remote error: denied by policy");
    Ok(())
}

#[crate::bisync::bisync]
#[cfg_attr(feature = "blocking-client", test)]
#[cfg_attr(all(feature = "async-client", not(feature = "blocking-client")), async_std::test)]
async fn commands_are_sent_only_if_the_remote_can_apply_them() -> TestResult {
    let (old, new) = (
        oid("1111111111111111111111111111111111111111"),
        oid("2222222222222222222222222222222222222222"),
    );
    let remote = [
        (old, "refs/heads/main"),
        (old, "refs/heads/gone"),
        (old, "refs/heads/same"),
    ];
    let commands = [
        command("refs/heads/main", old, new),
        command("refs/heads/gone", old, null()),
        command("refs/heads/same", old, old),
    ];

    let mut report = Vec::new();
    for line in ["unpack ok\n", "ok refs/heads/main\n"] {
        report.extend(pkt(line));
    }
    report.extend_from_slice(b"0000");
    let mut response = advertisement("report-status side-band-64k", &remote);
    response.extend(band(1, report));
    response.extend_from_slice(b"0000");
    let (sent, _, outcome) = push_to(response, &commands, b"pack", &options(gix_hash::Kind::Sha1)).await?;
    let mut expected = pkt(format!("{old} {new} refs/heads/main\0 report-status side-band-64k"));
    expected.extend_from_slice(b"0000pack");
    assert_eq!(
        sent.as_bstr(),
        expected.as_bstr(),
        "neither deletions without `delete-refs` nor commands without change are sent"
    );
    let statuses: Vec<_> = outcome.refs.iter().map(|r| r.status.clone()).collect();
    assert_eq!(statuses, [Status::Ok, Status::DeleteUnsupported, Status::UpToDate]);
    assert!(!outcome.is_success(), "the deletion failed");

    let mut atomic = options(gix_hash::Kind::Sha1);
    atomic.atomic = true;
    let (sent, _, outcome) = push_to(
        advertisement("report-status side-band-64k atomic", &remote),
        &commands,
        b"pack",
        &atomic,
    )
    .await?;
    assert_eq!(
        sent.as_bstr(),
        "0000",
        "an atomic push fails without sending any command"
    );
    assert_eq!(outcome.unpack, None, "there is no report without commands");
    let statuses: Vec<_> = outcome.refs.iter().map(|r| r.status.clone()).collect();
    assert_eq!(
        statuses,
        [Status::AtomicPushFailed, Status::DeleteUnsupported, Status::UpToDate]
    );

    let (sent, _, outcome) = push_to(
        advertisement("report-status side-band-64k", &remote),
        &commands[2..],
        b"pack",
        &options(gix_hash::Kind::Sha1),
    )
    .await?;
    assert_eq!(
        sent.as_bstr(),
        "0000",
        "only the end of the interaction is sent if nothing changes"
    );
    assert!(outcome.is_success(), "references that are up to date are pushed");
    Ok(())
}

#[crate::bisync::bisync]
#[cfg_attr(feature = "blocking-client", test)]
#[cfg_attr(all(feature = "async-client", not(feature = "blocking-client")), async_std::test)]
async fn remotes_without_status_report_are_assumed_to_apply_all_commands() -> TestResult {
    let old = oid("1111111111111111111111111111111111111111");
    let commands = [command("refs/heads/gone", old, null())];
    let (sent, _, outcome) = push_to(
        advertisement("delete-refs", &[(old, "refs/heads/gone")]),
        &commands,
        b"pack",
        &options(gix_hash::Kind::Sha1),
    )
    .await?;
    let mut expected = pkt(format!("{old} {} refs/heads/gone\0", null()));
    expected.extend_from_slice(b"0000");
    assert_eq!(sent.as_bstr(), expected.as_bstr(), "deletions alone need no pack");
    assert_eq!(
        outcome,
        Outcome {
            unpack: None,
            refs: vec![RefStatus {
                name: "refs/heads/gone".into(),
                status: Status::Ok,
                reports: Vec::new(),
            }],
        }
    );
    Ok(())
}

mod arguments {
    use gix_protocol::push::{Arguments, ReportStatus};
    use gix_transport::client::Capabilities;

    use super::{command, null, oid, options};
    use crate::TestResult;

    fn capabilities(after_nul: &str) -> Capabilities {
        Capabilities::from_bytes(format!("{} capabilities^{{}}\0{after_nul}", null()).as_bytes())
            .expect("valid capabilities")
            .0
    }

    #[test]
    fn capabilities_are_chosen_and_sent_like_git_does() -> TestResult {
        let mut options = options(gix_hash::Kind::Sha1);
        options.atomic = true;
        options.push_options = vec!["ci.skip".into()];
        let arguments = Arguments::new(
            &capabilities(
                "report-status report-status-v2 delete-refs side-band-64k quiet atomic ofs-delta push-options \
                 object-format=sha1 agent=git/2.55.0",
            ),
            &("agent", Some(" my agent\t1.0 ".into())),
            &options,
        )?;
        assert_eq!(
            arguments.capabilities(),
            " report-status-v2 side-band-64k quiet atomic push-options object-format=sha1 agent=my.agent.1.0",
            "the agent is trimmed and has its whitespace replaced"
        );
        assert_eq!(arguments.report_status, ReportStatus::V2);
        assert!(arguments.delete_refs && arguments.ofs_delta && arguments.thin_pack);

        let main = oid("1111111111111111111111111111111111111111");
        assert_eq!(
            arguments.command_line(&command("refs/heads/main", null(), main), true),
            format!(
                "{} {main} refs/heads/main\0 report-status-v2 side-band-64k quiet atomic push-options \
                 object-format=sha1 agent=my.agent.1.0",
                null()
            )
        );
        assert_eq!(
            arguments.command_line(&command("refs/heads/main", main, null()), false),
            format!("{main} {} refs/heads/main", null())
        );
        Ok(())
    }

    #[test]
    fn only_what_the_remote_advertises_is_used() -> TestResult {
        let arguments = Arguments::new(
            &capabilities("report-status no-thin"),
            &super::agent(),
            &options(gix_hash::Kind::Sha1),
        )?;
        assert_eq!(
            arguments.capabilities(),
            " report-status",
            "no agent, object format or quiet without the remote supporting them"
        );
        assert_eq!(arguments.report_status, ReportStatus::V1);
        assert!(!arguments.side_band && !arguments.delete_refs && !arguments.ofs_delta && !arguments.thin_pack);

        let arguments = Arguments::new(&capabilities("agent"), &super::agent(), &options(gix_hash::Kind::Sha1))?;
        assert_eq!(arguments.report_status, ReportStatus::None);
        assert_eq!(arguments.capabilities(), " agent=git/gix-fixture");
        Ok(())
    }

    #[test]
    fn options_need_the_capabilities_they_use() {
        let mut atomic = options(gix_hash::Kind::Sha1);
        atomic.atomic = true;
        let err = Arguments::new(&capabilities("report-status"), &super::agent(), &atomic)
            .expect_err("atomic pushes need `atomic`");
        assert_eq!(err.to_string(), "the receiving end does not support --atomic push");
        assert!(err.is_unsupported());

        let mut push_options = options(gix_hash::Kind::Sha1);
        push_options.push_options = vec!["ci.skip".into()];
        let err = Arguments::new(&capabilities("report-status"), &super::agent(), &push_options)
            .expect_err("push options need `push-options`");
        assert_eq!(err.to_string(), "the receiving end does not support push options");
        assert!(err.is_unsupported());
    }

    #[test]
    fn the_remote_must_support_the_object_format() -> TestResult {
        let options = options(gix_hash::Kind::Sha1);
        let arguments = Arguments::new(
            &capabilities("object-format=sha256 object-format=sha1"),
            &super::agent(),
            &options,
        )?;
        assert_eq!(
            arguments.object_format,
            Some(gix_hash::Kind::Sha1),
            "any of the advertised formats can be used"
        );

        let err = Arguments::new(&capabilities("object-format=sha256"), &super::agent(), &options)
            .expect_err("SHA-1 isn't advertised");
        assert_eq!(
            err.to_string(),
            "the receiving end does not support this repository's hash algorithm"
        );
        assert!(err.is_unsupported());
        Ok(())
    }

    #[test]
    #[cfg(feature = "sha256")]
    fn remotes_without_object_format_only_support_sha1() {
        let err = Arguments::new(
            &capabilities("report-status"),
            &super::agent(),
            &options(gix_hash::Kind::Sha256),
        )
        .expect_err("SHA-256 needs to be advertised");
        assert!(err.is_unsupported());
    }
}

mod response {
    use bstr::BString;
    use gix_protocol::push::response::{RefStatus, Status, Unpack, parse_report};

    use crate::TestResult;

    fn refs(names: &[&str]) -> Vec<RefStatus> {
        names
            .iter()
            .map(|name| RefStatus {
                name: (*name).into(),
                status: Status::NotReported,
                reports: Vec::new(),
            })
            .collect()
    }

    fn parse(lines: &[&str], refs: &mut [RefStatus]) -> gix_error::Result<Unpack> {
        let lines: Vec<BString> = lines.iter().map(|line| (*line).into()).collect();
        parse_report(&lines, refs, gix_hash::Kind::Sha1)
    }

    #[test]
    fn statuses_are_assigned_to_the_reported_references() -> TestResult {
        let mut refs = refs(&["refs/heads/a", "refs/heads/b", "refs/heads/c"]);
        refs[2].status = Status::UpToDate;
        let unpack = parse(
            &[
                "unpack index-pack abnormal exit",
                "ng refs/heads/b",
                "ok refs/heads/c",
                "ok refs/heads/unknown",
            ],
            &mut refs,
        )?;
        assert_eq!(
            unpack,
            Unpack::Failed {
                error: "index-pack abnormal exit".into()
            }
        );
        let statuses: Vec<_> = refs.into_iter().map(|r| r.status).collect();
        assert_eq!(
            statuses,
            [
                Status::NotReported,
                Status::Rejected {
                    reason: "failed".into()
                },
                Status::UpToDate
            ],
            "unreported references remain so, rejections have a default reason, \
             and references that weren't sent or are unknown are ignored"
        );
        Ok(())
    }

    #[test]
    fn malformed_reports_are_corrupt() {
        for (lines, expected) in [
            (&[][..], "unexpected flush packet while reading remote unpack status"),
            (
                &["ok refs/heads/a"][..],
                "unable to parse remote unpack status: ok refs/heads/a",
            ),
            (&["unpack ok", "bogus"][..], "invalid status line from remote: bogus"),
            (
                &["unpack ok", "maybe refs/heads/a"][..],
                "invalid ref status from remote: maybe",
            ),
            (
                &["unpack ok", "option refname refs/heads/b"][..],
                "'option' without a matching 'ok/ng' directive",
            ),
            (
                &["unpack ok", "ng refs/heads/a no", "option refname refs/heads/b"][..],
                "'option' without a matching 'ok/ng' directive",
            ),
        ] {
            let err = parse(lines, &mut refs(&["refs/heads/a"])).expect_err("malformed");
            assert_eq!(err.to_string(), expected);
            assert!(err.is_corrupted());
        }
    }
}

mod remote_refs {
    use gix_protocol::{Handshake, handshake::Ref, push::RemoteRefs};

    use super::oid;

    #[test]
    fn have_lines_name_objects_but_no_reference() {
        let (a, b) = (
            oid("1111111111111111111111111111111111111111"),
            oid("2222222222222222222222222222222222222222"),
        );
        let main = Ref::Direct {
            full_ref_name: "refs/heads/main".into(),
            object: a,
        };
        let have = |object| Ref::Direct {
            full_ref_name: ".have".into(),
            object,
        };
        assert_eq!(
            RemoteRefs::from_refs([have(b), main.clone(), have(a)]),
            RemoteRefs {
                refs: vec![main],
                extra_have: vec![b, a],
            }
        );
    }

    #[test]
    fn protocol_v2_is_unsupported() {
        let handshake = Handshake {
            server_protocol_version: gix_transport::Protocol::V2,
            ..Default::default()
        };
        let err = RemoteRefs::from_handshake(&handshake).expect_err("receive-pack doesn't support V2");
        assert_eq!(err.to_string(), "support for protocol v2 not implemented yet");
        assert!(err.is_unsupported());
    }
}

#[cfg(feature = "blocking-client")]
mod file {
    use std::{collections::BTreeSet, sync::atomic::AtomicBool};

    use bstr::{BString, ByteSlice};
    use gix_error::{ResultExt, message};
    use gix_hash::ObjectId;
    use gix_protocol::push::{Context, Options, RemoteRefs};
    use gix_transport::{Protocol, Service, client::blocking_io::file};
    use gix_utils::progress;

    use super::{RECORDED, agent, assert_agrees_with_git, parse_request};
    use crate::TestResult;

    /// Return the references of the remote at `path` as advertised by `git receive-pack`.
    fn remote_refs(path: &str) -> gix_error::Result<BTreeSet<(BString, ObjectId)>> {
        let mut transport = file::connect(path, Protocol::V0, false).expect("infallible");
        let handshake = gix_protocol::handshake(
            &mut transport,
            Service::ReceivePack,
            |_| Ok(None),
            Vec::new(),
            &mut progress::Discard,
        )?;
        gix_protocol::indicate_end_of_interaction(&mut transport, false)
            .or_raise(|| message("Could not end the interaction"))?;
        Ok(RemoteRefs::from_handshake(&handshake)?
            .refs
            .iter()
            .map(|r| {
                let (name, id, _) = r.unpack();
                (
                    name.to_owned(),
                    id.expect("advertised references point to objects").to_owned(),
                )
            })
            .collect())
    }

    #[test]
    fn pushing_into_bare_repositories_has_the_outcome_of_git_push() -> TestResult {
        if gix_testtools::run_in_isolated_process()? {
            return Ok(());
        }
        let dir = gix_testtools::scripted_fixture_writable("push.sh")?;
        for name in RECORDED {
            let case = dir.path().join(name);
            let request = parse_request(&std::fs::read(case.join("request"))?);
            let remote = dir.path().join(format!("{name}-before.git"));
            let remote = remote.to_str().expect("fixture paths are valid UTF-8");

            let mut transport = file::connect(remote, Protocol::V0, false).expect("infallible");
            let handshake = gix_protocol::handshake(
                &mut transport,
                Service::ReceivePack,
                |_| Ok(None),
                Vec::new(),
                &mut progress::Discard,
            )?;
            let outcome = gix_protocol::push(
                &request.commands,
                |writer, _arguments, _progress, _should_interrupt| {
                    writer
                        .write_all(&request.pack)
                        .or_raise(|| message("Could not write the pack"))
                },
                progress::Discard,
                &AtomicBool::default(),
                Context {
                    handshake: &handshake,
                    transport: &mut transport,
                    user_agent: agent(),
                    trace_packetlines: false,
                },
                &Options {
                    object_hash: gix_testtools::object_hash(),
                    atomic: request.capabilities.contains_str(" atomic"),
                    push_options: request.push_options.clone(),
                    quiet: true,
                },
            )?;
            drop(transport);
            assert_agrees_with_git(name, &outcome, &std::fs::read(case.join("porcelain"))?);

            let expected: BTreeSet<_> = std::fs::read(case.join("refs-after"))?
                .lines()
                .map(|line| {
                    let (id, name) = line.split_once_str(" ").expect("<object> <name>");
                    (name.into(), ObjectId::from_hex(id).expect("valid hex"))
                })
                .collect();
            assert_eq!(
                remote_refs(remote)?,
                expected,
                "{name}: the remote has the references `git push` left it with"
            );
        }
        Ok(())
    }
}
