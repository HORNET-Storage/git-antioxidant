/// Convert a hexadecimal hash into its corresponding `ObjectId` or _panic_.
fn hex_to_id(hex_sha1: &str, hex_sha256: &str) -> gix_hash::ObjectId {
    match gix_testtools::object_hash() {
        gix_hash::Kind::Sha1 => gix_hash::ObjectId::from_hex(hex_sha1.as_bytes()).expect("40 bytes hex"),
        gix_hash::Kind::Sha256 => gix_hash::ObjectId::from_hex(hex_sha256.as_bytes()).expect("64 bytes hex"),
        _ => unimplemented!(),
    }
}

#[test]
fn impossible_path_allocation_preserves_its_source() {
    let mut input = Vec::new();
    input.extend_from_slice(&usize::MAX.to_le_bytes());
    input.extend_from_slice(&0usize.to_le_bytes());
    input.extend_from_slice(&[0, 0]);
    input.extend_from_slice(&[0; 20]);

    let err = gix_worktree_stream::Stream::from_read(std::io::Cursor::new(input))
        .next_entry()
        .expect_err("the declared path length cannot be allocated");
    insta::assert_debug_snapshot!(err, "the allocation failure remains classified", @"
    Could not read stream entry

    Caused by:
        0: I/O error (OutOfMemory)
        1: memory allocation failed because the computed capacity exceeded the collection's maximum
    ");
    let io_err = err
        .downcast_any_ref::<std::io::Error>()
        .expect("the I/O error is retained");
    insta::assert_debug_snapshot!(io_err, "impossible path allocation preserves its source", @"
    Custom {
        kind: OutOfMemory,
        error: TryReserveError {
            kind: CapacityOverflow,
        },
    }
    ");
    assert_eq!(io_err.kind(), std::io::ErrorKind::OutOfMemory);
    assert!(
        io_err
            .get_ref()
            .expect("the allocation error is retained")
            .is::<std::collections::TryReserveError>(),
        "the allocation error is retained"
    );
    assert!(err.is_resource_exhausted(), "the allocation failure remains classified");
    assert!(!err.is_corrupted());
    assert!(!err.can_retry());
}

/// The bytes of an entry with `body` at `path`, of a known length or in chunks of unknown length, in the format of
/// [`gix_worktree_stream::Stream::into_read()`].
fn entry(path: &[u8], body: &[u8], known_length: bool) -> Vec<u8> {
    let hash = gix_testtools::object_hash();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&path.len().to_le_bytes());
    bytes.extend_from_slice(&(if known_length { body.len() } else { usize::MAX }).to_le_bytes());
    bytes.push(1); // a blob
    bytes.push(match hash {
        gix_hash::Kind::Sha1 => 0,
        gix_hash::Kind::Sha256 => 1,
        _ => unimplemented!(),
    });
    bytes.extend_from_slice(hash.null().as_bytes());
    bytes.extend_from_slice(path);
    if !known_length && !body.is_empty() {
        bytes.extend_from_slice(&(body.len() as u16).to_le_bytes());
    }
    bytes.extend_from_slice(body);
    if !known_length {
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    bytes
}

/// A stream may only end before an entry. If it ends within the information of an entry or before its body does, it was
/// cut short.
#[test]
fn streams_that_end_within_an_entry_were_cut_short() -> gix_testtools::TestResult {
    use std::io::{Cursor, ErrorKind, Read};
    assert!(
        gix_worktree_stream::Stream::from_read(Cursor::new(Vec::new()))
            .next_entry()?
            .is_none(),
        "a stream may end before an entry"
    );
    for known_length in [true, false] {
        let bytes = entry(b"f", b"abc", known_length);
        let body = if known_length { 3 } else { 2 + 3 + 2 };
        let information = bytes.len() - body;
        for end in 1..information {
            let err = gix_worktree_stream::Stream::from_read(Cursor::new(bytes[..end].to_vec()))
                .next_entry()
                .expect_err("the information is incomplete");
            let kind = err.downcast_any_ref::<std::io::Error>().expect("an I/O error").kind();
            assert_eq!(kind, ErrorKind::UnexpectedEof, "cut after {end} bytes");
        }
        for end in information..bytes.len() {
            let mut stream = gix_worktree_stream::Stream::from_read(Cursor::new(bytes[..end].to_vec()));
            let mut entry = stream.next_entry()?.expect("the information is complete");
            let err = entry.read_to_end(&mut Vec::new()).expect_err("the body is incomplete");
            assert_eq!(err.kind(), ErrorKind::UnexpectedEof, "cut after {end} bytes");
        }
    }
    Ok(())
}

/// Reading into an empty buffer reads nothing, and leaves the entry and the entries after it as they are.
#[test]
fn empty_reads_leave_the_entry_as_it_is() -> gix_testtools::TestResult {
    use std::io::{Cursor, Read};
    for known_length in [true, false] {
        let mut bytes = entry(b"f", b"abc", known_length);
        bytes.extend(entry(b"g", b"de", known_length));
        let mut stream = gix_worktree_stream::Stream::from_read(Cursor::new(bytes));
        for (path, body) in [("f", "abc"), ("g", "de")] {
            let mut entry = stream.next_entry()?.expect("two entries");
            assert_eq!(entry.relative_path(), path);
            let remaining = entry.bytes_remaining();
            assert_eq!(entry.read(&mut [])?, 0);
            assert_eq!(entry.bytes_remaining(), remaining, "nothing was read");
            let mut read = Vec::new();
            entry.read_to_end(&mut read)?;
            assert_eq!(read, body.as_bytes());
        }
        assert!(stream.next_entry()?.is_none());
    }
    Ok(())
}

/// Bytes that name no mode or kind of hash are an error, as a stream can come from anywhere.
#[test]
fn unknown_modes_and_kinds_of_hash_are_errors() {
    let header = 2 * std::mem::size_of::<usize>();
    for (field, value) in [(0, 5), (0, u8::MAX), (1, 2), (1, u8::MAX)] {
        let mut bytes = entry(b"f", b"", true);
        bytes[header + field] = value;
        let err = gix_worktree_stream::Stream::from_read(std::io::Cursor::new(bytes))
            .next_entry()
            .expect_err("the information names no mode or kind of hash");
        let kind = err.downcast_any_ref::<std::io::Error>().expect("an I/O error").kind();
        assert_eq!(kind, std::io::ErrorKind::InvalidData, "{value} in field {field}");
    }
}

mod from_tree {
    use gix_error::Result;
    use std::{
        convert::Infallible,
        io::{Error, Read, Write},
        path::PathBuf,
        sync::Arc,
    };

    use gix_attributes::glob::pattern::Case;
    use gix_error::ErrorExt;
    use gix_hash::oid;
    use gix_object::{Data, bstr::ByteSlice, tree::EntryKind};
    use gix_worktree::stack::state::attributes::Source;
    use std::sync::LazyLock;

    use crate::hex_to_id;

    #[derive(Clone)]
    struct FailObjectRetrieval;

    impl gix_object::Find for FailObjectRetrieval {
        fn try_find<'a>(&self, _id: &oid, _buffer: &'a mut Vec<u8>) -> Result<Option<Data<'a>>> {
            Err(Error::other("object retrieval failed").raise())
        }
    }

    #[test]
    fn can_receive_err_if_root_is_not_found() {
        let mut stream = gix_worktree_stream::from_tree(
            gix_testtools::object_hash().null(),
            FailObjectRetrieval,
            mutating_pipeline(false),
            gix_object::tree::DEFAULT_MAX_DEPTH,
            |_, _, _| -> std::result::Result<_, Infallible> { unreachable!("must not be called") },
        );
        let err = stream.next_entry().unwrap_err();
        insta::assert_debug_snapshot!(err, "can receive err if root is not found", @"
        Could not find a tree to traverse

        Caused by:
            0: I/O error (Other)
            1: object retrieval failed
        ");
    }

    #[test]
    fn can_receive_err_if_attribute_not_found() -> gix_testtools::TestResult {
        let (_dir, head_tree, odb, _cache) = basic()?;
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb,
            mutating_pipeline(false),
            gix_object::tree::DEFAULT_MAX_DEPTH,
            |_, _, _| Err(Error::other("attribute retrieval failed")),
        );
        let err = stream.next_entry().unwrap_err();
        insta::assert_debug_snapshot!(err, "can receive err if attribute not found", @r#"
        Could not query attributes for path ".gitattributes"

        Caused by:
            0: I/O error (Other)
            1: attribute retrieval failed
        "#);
        Ok(())
    }

    #[test]
    fn filter_process_failure_during_shutdown_is_ignored() -> gix_testtools::TestResult {
        let (_dir, head_tree, odb, mut cache) = basic()?;
        let mut pipeline = mutating_pipeline(true);
        *pipeline
            .options_mut()
            .drivers
            .first_mut()
            .expect("the filter driver was configured") = driver_with_process("fail-on-shutdown");
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb.clone(),
            pipeline,
            gix_object::tree::DEFAULT_MAX_DEPTH,
            move |rela_path, mode, attrs| {
                cache
                    .at_entry(rela_path, Some(mode.into()), &odb)
                    .map(|entry| entry.matching_attributes(attrs))
                    .map(|_| ())
            },
        );

        while let Some(mut entry) = stream.next_entry()? {
            std::io::copy(&mut entry, &mut std::io::sink())?;
        }
        Ok(())
    }

    #[test]
    fn will_provide_all_information_and_respect_export_ignore() -> gix_testtools::TestResult {
        let (dir, head_tree, odb, mut cache) = basic()?;
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb.clone(),
            mutating_pipeline(true),
            gix_object::tree::DEFAULT_MAX_DEPTH,
            move |rela_path, mode, attrs| {
                cache
                    .at_entry(rela_path, Some(mode.into()), &odb)
                    .map(|entry| entry.matching_attributes(attrs))
                    .map(|_| ())
            },
        );
        let object_hash = gix_testtools::object_hash();
        stream
            .add_entry_from_path(&dir, &dir.join("extra-file"), object_hash)?
            .add_entry_from_path(&dir, &dir.join("extra-bigfile"), object_hash)?
            .add_entry_from_path(&dir, &dir.join("extra-exe"), object_hash)?
            .add_entry_from_path(&dir, &dir.join("extra-dir-empty"), object_hash)?
            .add_entry_from_path(&dir, &dir.join("extra-dir").join("symlink-to-extra"), object_hash)?;

        let tee_read = TeeToMemory {
            read: stream.into_read(),
            write: Default::default(),
        };
        let copy = tee_read.write.clone();
        let mut paths_and_modes = Vec::new();
        let mut stream = gix_worktree_stream::Stream::from_read(tee_read);

        while let Some(mut entry) = stream.next_entry()? {
            paths_and_modes.push((entry.relative_path().to_owned(), entry.mode.kind(), entry.id));
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if !buf.is_empty() && entry.mode.is_blob() {
                if entry.relative_path().contains_str("extra") {
                    assert!(
                        buf.find_byte(b'\r').is_none(),
                        "extra-files are not processed in any way"
                    );
                } else if !entry.relative_path().contains_str("big") {
                    assert!(
                        buf.find_byte(b'\r').is_some(),
                        "'{}' did not contain a carriage return as sign of having been filtered",
                        buf.as_bstr()
                    );
                    if entry.relative_path().ends_with_str(b"streamed") {
                        assert_eq!(buf.as_bstr(), "➡streamed-by-driver\r\n");
                    }
                }
            }
        }

        assert_eq!(
            paths_and_modes,
            &[
                (
                    ".gitattributes".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "45c160c35c17ad264b96431cceb9793160396e99",
                        "e34ae8e2c517230c6c613202a955b5f2095567830de5c3bb7081d72f1af393dd"
                    )
                ),
                (
                    "a".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "45b983be36b73c0788dc9cbcb76cbb80fc7bb057",
                        "96c18f0297e38d01f4b2dacddea4259aea6b2961eb0822bd2c0c3f6029030045"
                    )
                ),
                (
                    "bigfile".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "4995fde49ed64e043977e22539f66a0d372dd129",
                        "527a170c0c29520af58bf61a383c4e99a7a7c63c0475cbb630cf98d38d6c0dce"
                    )
                ),
                (
                    "symlink-to-a".into(),
                    EntryKind::Link,
                    hex_to_id(
                        "2e65efe2a145dda7ee51d1741299f848e5bf752e",
                        "eb337bcee2061c5313c9a1392116b6c76039e9e30d71467ae359b36277e17dc7"
                    )
                ),
                (
                    "dir/.gitattributes".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "81b9a375276405703e05be6cecf0fc1c8b8eed64",
                        "4b74474eca46ad7e1807afe8855e7a511d132b314d3d0f481bb55a331b249fb7"
                    )
                ),
                (
                    "dir/b".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "ab4a98190cf776b43cb0fe57cef231fb93fd07e6",
                        "cf84f9ae227fa5c481dacd97f6d6506de727dabd30377c6907f623ef1192637a"
                    )
                ),
                (
                    "dir/subdir/exe".into(),
                    EntryKind::BlobExecutable,
                    hex_to_id(
                        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
                        "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
                    )
                ),
                (
                    "dir/subdir/streamed".into(),
                    EntryKind::Blob,
                    hex_to_id(
                        "08991f58f4de5d85b61c0f87f3ac053c79d0e739",
                        "8c13ad7df9686daf00357f34700922485802cb0be33e684ec5171f7d0d8a84fd"
                    )
                ),
                ("extra-file".into(), EntryKind::Blob, object_hash.null()),
                ("extra-bigfile".into(), EntryKind::Blob, object_hash.null()),
                (
                    "extra-exe".into(),
                    if cfg!(windows) {
                        EntryKind::Blob
                    } else {
                        EntryKind::BlobExecutable
                    },
                    object_hash.null()
                ),
                ("extra-dir-empty".into(), EntryKind::Tree, object_hash.null()),
                ("extra-dir/symlink-to-extra".into(), EntryKind::Link, object_hash.null())
            ]
        );

        #[cfg(target_pointer_width = "64")]
        let expected_buffer_length: usize = match gix_testtools::object_hash() {
            gix_hash::Kind::Sha1 => 320302,
            gix_hash::Kind::Sha256 => 320458,
            _ => unimplemented!(),
        };
        #[cfg(target_pointer_width = "32")]
        let expected_buffer_length: usize = match gix_testtools::object_hash() {
            gix_hash::Kind::Sha1 => 320198,
            gix_hash::Kind::Sha256 => todo!("let the test fail on CI and add the value here"),
            _ => unimplemented!(),
        };

        assert_eq!(
            copy.lock().len(),
            expected_buffer_length,
            "keep track of file size changes of the streaming format"
        );

        let mut copied_stream =
            gix_worktree_stream::Stream::from_read(std::io::Cursor::new(copy.lock().as_bytes().to_owned()));
        let mut copied_paths_and_modes = Vec::new();
        let mut buf = Vec::new();
        while let Some(mut entry) = copied_stream.next_entry()? {
            copied_paths_and_modes.push((entry.relative_path().to_owned(), entry.mode.kind(), entry.id));
            buf.clear();
            entry.read_to_end(&mut buf)?;
        }
        assert_eq!(
            copied_paths_and_modes, paths_and_modes,
            "a stream copy yields exactly the same result"
        );
        Ok(())
    }

    #[test]
    fn stops_at_trees_below_the_limit() -> gix_testtools::TestResult {
        let (_dir, head_tree, odb, mut cache) = basic()?;
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb.clone(),
            mutating_pipeline(false),
            0,
            move |rela_path, mode, attrs| {
                cache
                    .at_entry(rela_path, Some(mode.into()), &odb)
                    .map(|entry| entry.matching_attributes(attrs))
                    .map(|_| ())
            },
        );
        // The walk has its own thread, so its error may reach the read of an entry or the read of the next one.
        let message = loop {
            match stream.next_entry() {
                Ok(Some(mut entry)) => {
                    if let Err(err) = std::io::copy(&mut entry, &mut std::io::sink()) {
                        break format!("{err:?}");
                    }
                }
                Ok(None) => panic!("the tree has a directory"),
                Err(err) => break format!("{err:?}"),
            }
        };
        assert!(message.contains("exceeded maximum allowed tree depth"), "{message}");
        Ok(())
    }

    /// The bytes of a stream whose walk failed end with the walk's error, so that they can't pass for a complete stream.
    #[test]
    fn raw_bytes_end_with_the_error_of_the_walk() -> gix_testtools::TestResult {
        let failed_walks = || -> gix_testtools::Result<Vec<(gix_worktree_stream::Stream, &'static str)>> {
            let (_dir, head_tree, odb, mut cache) = basic()?;
            Ok(vec![
                (
                    gix_worktree_stream::from_tree(
                        gix_testtools::object_hash().null(),
                        FailObjectRetrieval,
                        mutating_pipeline(false),
                        gix_object::tree::DEFAULT_MAX_DEPTH,
                        |_, _, _| -> std::result::Result<_, Infallible> { unreachable!("must not be called") },
                    ),
                    "object retrieval failed",
                ),
                (
                    gix_worktree_stream::from_tree(
                        head_tree,
                        odb.clone(),
                        mutating_pipeline(false),
                        gix_object::tree::DEFAULT_MAX_DEPTH,
                        |_, _, _| Err(Error::other("attribute retrieval failed")),
                    ),
                    "attribute retrieval failed",
                ),
                (
                    gix_worktree_stream::from_tree(
                        head_tree,
                        odb.clone(),
                        mutating_pipeline(false),
                        0,
                        move |rela_path, mode, attrs| {
                            cache
                                .at_entry(rela_path, Some(mode.into()), &odb)
                                .map(|entry| entry.matching_attributes(attrs))
                                .map(|_| ())
                        },
                    ),
                    "exceeded maximum allowed tree depth",
                ),
            ])
        };
        for as_read_mut in [false, true] {
            for (mut stream, cause) in failed_walks()? {
                let mut bytes = Vec::new();
                let err = if as_read_mut {
                    stream.as_read_mut().read_to_end(&mut bytes)
                } else {
                    stream.into_read().read_to_end(&mut bytes)
                }
                .expect_err("the walk failed");
                let message = format!("{err:?}");
                assert!(message.contains(cause), "{message}");
            }
        }
        Ok(())
    }

    /// A walk that fails within an entry makes the read of the entry fail with the walk's error, rather than with the end
    /// of the bytes or the walk's cancellation.
    #[test]
    fn entries_cut_short_by_the_walk_fail_with_its_error() -> gix_testtools::TestResult {
        let (_dir, head_tree, odb, mut cache) = basic()?;
        let mut pipeline = mutating_pipeline(false);
        pipeline.options_mut().drivers.push(gix_filter::Driver {
            name: "arrow".into(),
            clean: None,
            smudge: Some(driver_command("smudge fail")),
            process: None,
            required: true,
        });
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb.clone(),
            pipeline,
            gix_object::tree::DEFAULT_MAX_DEPTH,
            move |rela_path, mode, attrs| {
                cache
                    .at_entry(rela_path, Some(mode.into()), &odb)
                    .map(|entry| entry.matching_attributes(attrs))
                    .map(|_| ())
            },
        );
        // The walk has its own thread, so its error may reach the start of the entry or the read of its body.
        let message = loop {
            match stream.next_entry() {
                Ok(Some(mut entry)) => {
                    if let Err(err) = std::io::copy(&mut entry, &mut std::io::sink()) {
                        break format!("{err:?}");
                    }
                }
                Ok(None) => panic!("the filter of 'dir/subdir/streamed' fails"),
                Err(err) => break format!("{err:?}"),
            }
        };
        assert!(message.contains("Could not write stream data"), "{message}");
        Ok(())
    }

    /// A walk that panics has its panic passed on where its bytes end, so that they can't pass for a complete stream.
    #[test]
    fn panics_of_the_walk_reach_its_reader() -> gix_testtools::TestResult {
        for raw in [false, true] {
            let (_dir, head_tree, odb, _cache) = basic()?;
            let mut stream = gix_worktree_stream::from_tree(
                head_tree,
                odb,
                mutating_pipeline(false),
                gix_object::tree::DEFAULT_MAX_DEPTH,
                |_, _, _| -> std::result::Result<_, Infallible> { panic!("the attributes of the walk panicked") },
            );
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if raw {
                    stream.as_read_mut().read_to_end(&mut Vec::new()).map(drop).ok()
                } else {
                    stream.next_entry().map(|entry| entry.is_some()).ok().map(drop)
                }
            }))
            .expect_err("the panic of the walk is passed on");
            assert_eq!(
                panic.downcast_ref::<&str>(),
                Some(&"the attributes of the walk panicked"),
                "raw: {raw}"
            );
        }
        Ok(())
    }

    #[test]
    fn can_drop_entry_without_reading_it() -> gix_testtools::TestResult {
        let (_dir, head_tree, odb, mut cache) = basic()?;
        let mut stream = gix_worktree_stream::from_tree(
            head_tree,
            odb.clone(),
            mutating_pipeline(false),
            gix_object::tree::DEFAULT_MAX_DEPTH,
            move |rela_path, mode, attrs| {
                cache
                    .at_entry(rela_path, Some(mode.into()), &odb)
                    .map(|entry| entry.matching_attributes(attrs))
                    .map(|_| ())
            },
        );

        drop(stream.next_entry()?);
        Ok(())
    }

    fn basic() -> gix_testtools::Result<(PathBuf, gix_hash::ObjectId, gix_odb::HandleArc, gix_worktree::Stack)> {
        let dir = gix_testtools::scripted_fixture_read_only("basic.sh")?;

        let head = {
            let hex = std::fs::read(dir.join("head.hex"))?;
            gix_hash::ObjectId::from_hex(hex.trim())?
        };
        let odb = gix_odb::at(dir.join(".git").join("objects"), gix_testtools::object_hash())?;

        let mut collection = Default::default();
        let mut buf = Default::default();
        let attributes = gix_worktree::stack::state::Attributes::new(
            gix_attributes::Search::new_globals(None::<PathBuf>, &mut buf, &mut collection)?,
            None,
            Source::WorktreeThenIdMapping,
            collection,
        );
        let state = gix_worktree::stack::State::AttributesStack(attributes);
        let cache = gix_worktree::Stack::new(&dir, state, Case::Sensitive, Default::default(), Default::default());
        Ok((dir, head, odb.into_arc()?, cache))
    }

    fn mutating_pipeline(driver: bool) -> gix_filter::Pipeline {
        gix_filter::Pipeline::new(
            Default::default(),
            gix_testtools::object_hash(),
            gix_filter::pipeline::Options {
                drivers: if driver { vec![driver_with_process("")] } else { vec![] },
                eol_config: gix_filter::eol::Configuration {
                    auto_crlf: gix_filter::eol::AutoCrlf::Enabled,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
    }

    /// The command line of the test driver with `args`.
    fn driver_command(args: &str) -> gix_object::bstr::BString {
        let mut exe = DRIVER.to_string_lossy().into_owned();
        if cfg!(windows) {
            exe = exe.replace('\\', "/");
        }
        (exe + " " + args).into()
    }

    pub(crate) fn driver_with_process(extra_args: &str) -> gix_filter::Driver {
        gix_filter::Driver {
            name: "arrow".into(),
            clean: None,
            smudge: None,
            process: Some(driver_command(&format!("process {extra_args}"))),
            required: true,
        }
    }

    static DRIVER: LazyLock<PathBuf> =
        LazyLock::new(|| gix_testtools::build_example_for_test("gix-filter", "arrow", env!("CARGO_TARGET_TMPDIR")));

    struct TeeToMemory<R> {
        read: R,
        write: Arc<parking_lot::Mutex<Vec<u8>>>,
    }

    impl<R> std::io::Read for TeeToMemory<R>
    where
        R: std::io::Read,
    {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let nb = self.read.read(buf)?;
            self.write.lock().write_all(&buf[..nb])?;
            Ok(nb)
        }
    }
}
