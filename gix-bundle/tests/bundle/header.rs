use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use bstr::{BString, ByteSlice};
use gix_bundle::{Header, Prerequisite, Reference, Version};
use gix_hash::{Kind, ObjectId};
use gix_testtools::TestResult;

use crate::{GIT_BUNDLES, cases, fixture, history, open_bundle, references, revs};

const SHA1: &str = "1111111111111111111111111111111111111111";
const SHA256: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn read(header: &str) -> gix_error::Result<Header> {
    Header::from_read(&mut header.as_bytes())
}

/// The length of the header of `bundle`, which ends with its first empty line.
fn header_len(bundle: &[u8]) -> usize {
    bundle.find(b"\n\n").expect("a bundle header ends with an empty line") + 2
}

/// The references of the bundle `name` in `dir`, as `git bundle list-heads` lists them.
fn heads(dir: &Path, name: &str) -> TestResult<Vec<Reference>> {
    references(&dir.join(format!("{name}.heads")))
}

#[test]
fn reads_the_headers_that_git_writes() -> TestResult {
    let dir = fixture()?;
    let revs = revs(&dir)?;
    let object_hash = gix_testtools::object_hash();
    let default_version = if object_hash == Kind::Sha1 {
        Version::V2
    } else {
        Version::V3
    };
    let base = Prerequisite {
        id: revs.base_commit_id,
        comment: "first".into(),
    };
    let main = Reference {
        id: revs.main_commit_id,
        name: "refs/heads/main".into(),
    };
    let v1 = Reference {
        id: revs.v1_tag_id,
        name: "refs/tags/v1".into(),
    };
    for name in GIT_BUNDLES {
        let (version, filter, prerequisites, references) = match *name {
            "full" => (default_version, None, vec![], vec![main.clone()]),
            "incremental" => (default_version, None, vec![base.clone()], vec![main.clone()]),
            "with-tag" => (
                default_version,
                None,
                vec![base.clone()],
                vec![main.clone(), v1.clone()],
            ),
            "incremental-v3" => (Version::V3, None, vec![base.clone()], vec![main.clone()]),
            "filtered" => (
                Version::V3,
                Some(BString::from("blob:none")),
                vec![],
                vec![main.clone()],
            ),
            other => unreachable!("{other} has no expectations"),
        };
        let (header, mut pack) = open_bundle(&dir, name)?;
        assert_eq!(
            header,
            Header {
                version,
                object_hash,
                filter,
                prerequisites,
                references,
            },
            "{name}: the header is read as git wrote it"
        );
        assert_eq!(
            header.references,
            heads(&dir, name)?,
            "{name}: git bundle list-heads reads the same references"
        );
        let mut signature = [0; 4];
        pack.read_exact(&mut signature)?;
        assert_eq!(&signature, b"PACK", "{name}: the pack follows the header");
    }
    Ok(())
}

#[test]
fn writes_the_headers_that_git_writes() -> TestResult {
    let dir = fixture()?;
    let history = history()?;
    let mut paths: Vec<_> = GIT_BUNDLES
        .iter()
        .map(|name| dir.join(format!("{name}.bundle")))
        .collect();
    // git refuses to write some of the bundles of the history.
    paths.extend(
        cases(&history)?
            .into_iter()
            .map(|case| history.join(format!("{}.bundle", case.name)))
            .filter(|path| path.is_file()),
    );
    for path in paths {
        let name = path.display();
        let bundle = std::fs::read(&path)?;
        let len = header_len(&bundle);
        let mut rest = bundle.as_slice();
        let header = Header::from_read(&mut rest)?;
        assert_eq!(rest.len(), bundle.len() - len, "{name}: exactly the header was read");

        let mut written = Vec::new();
        header.write_to(&mut written)?;
        assert_eq!(
            written.as_bstr(),
            bundle[..len].as_bstr(),
            "{name}: the header is written as git wrote it"
        );
    }
    Ok(())
}

#[test]
fn reads_exactly_the_header_through_any_buffer() -> TestResult {
    let dir = fixture()?;
    for name in GIT_BUNDLES {
        let path = dir.join(format!("{name}.bundle"));
        let bundle = std::fs::read(&path)?;
        let pack_len = bundle.len() - header_len(&bundle);
        for capacity in [1, 2, 16, 8192] {
            let mut read = BufReader::with_capacity(capacity, File::open(&path)?);
            Header::from_read(&mut read)?;
            let mut rest = Vec::new();
            read.read_to_end(&mut rest)?;
            assert_eq!(rest.len(), pack_len, "{name} through a buffer of {capacity} bytes");
        }
    }
    Ok(())
}

#[test]
fn refuses_malformed_headers() {
    for header in [
        "# v4 git bundle\n\n".to_owned(),
        "# v2 git bundle".to_owned(),
        format!("# v2 git bundle\n{SHA1} refs/heads/main\n"),
        format!("# v2 git bundle\n{SHA1} refs/heads/main"),
        format!("# v2 git bundle\n{SHA1} refs/heads/main\n\r\n"),
        format!("# v2 git bundle\n{SHA1}\n\n"),
        format!("# v2 git bundle\n{SHA1} \t\n\n"),
        format!("# v2 git bundle\n{SHA1}0 refs/heads/main\n\n"),
        format!("# v2 git bundle\n-{SHA1}0\n\n"),
        format!("# v2 git bundle\n{} refs/heads/main\n\n", "g".repeat(40)),
        format!("# v2 git bundle\n{} refs/heads/main\n\n", &SHA1[..39]),
        format!("# v2 git bundle\n{}\n\n", &SHA1[..39]),
        format!("# v2 git bundle\n@object-format=sha1\n{SHA1} refs/heads/main\n\n"),
        format!("# v3 git bundle\n{SHA1} refs/heads/main\n@object-format=sha1\n\n"),
        "# v3 git bundle\n\n".to_owned(),
        format!("# v3 git bundle\n@object-format=sha1\n{SHA1} refs/heads/main\n@filter=blob:none\n\n"),
        format!("# v3 git bundle\n@object-format=sha1\n-{SHA1}\n@filter=blob:none\n\n"),
        "# v3 git bundle\n@object-format=sha1\n@object-format=sha1\n\n".to_owned(),
        "# v3 git bundle\n@object-format=sha1\n@filter=blob:none\n@filter=tree:0\n\n".to_owned(),
        "# v3 git bundle\n@object-format=sha1\n@filter=\n\n".to_owned(),
        format!("# v3 git bundle\n@object-format=sha256\n{SHA1} refs/heads/main\n\n"),
    ] {
        let err = read(&header).expect_err(&header);
        assert!(
            err.is_corrupted(),
            "{header:?}: a malformed header is refused as corrupted: {err:?}"
        );
    }
}

#[test]
fn refuses_unsupported_capabilities() {
    for header in [
        "# v3 git bundle\n@object-format=sha1\n@unknown\n\n",
        "# v3 git bundle\n@object-format=sha1\n@unknown=value\n\n",
        "# v3 git bundle\n@object-format=sha512\n\n",
        "# v3 git bundle\n@object-format=SHA1\n\n",
    ] {
        let err = read(header).expect_err(header);
        assert!(
            err.is_unsupported(),
            "{header:?}: an unknown capability or object hash is refused as unsupported: {err:?}"
        );
    }
}

#[test]
fn reads_lines_as_git_does() -> TestResult {
    let id = ObjectId::from_hex(SHA1.as_bytes())?;
    for (line, name) in [
        ("\trefs/heads/main", "refs/heads/main"),
        (" refs/heads/main \t\r", "refs/heads/main"),
        ("  refs/heads/main", " refs/heads/main"),
        (" refs/heads/main\x0c", "refs/heads/main\x0c"),
        (" refs/heads/a name", "refs/heads/a name"),
    ] {
        let header = read(&format!("# v2 git bundle\n{SHA1}{line}\n\n"))?;
        assert_eq!(
            header.references,
            [Reference { id, name: name.into() }],
            "{line:?}: the reference is read as git reads it"
        );
    }
    for (line, comment) in [
        ("", ""),
        (" ", ""),
        (" first", "first"),
        ("\tfirst commit ", "first commit"),
    ] {
        let header = read(&format!("# v2 git bundle\n-{SHA1}{line}\n{SHA1} refs/heads/main\n\n"))?;
        assert_eq!(
            header.prerequisites,
            [Prerequisite {
                id,
                comment: comment.into()
            }],
            "{line:?}: the comment is read without its separator and trailing whitespace"
        );
    }
    Ok(())
}

#[test]
fn writes_every_field() -> TestResult {
    let id = ObjectId::from_hex(SHA256.as_bytes())?;
    let header = Header {
        version: Version::V3,
        object_hash: Kind::Sha256,
        filter: Some("blob:none".into()),
        prerequisites: vec![
            Prerequisite {
                id,
                comment: "first".into(),
            },
            Prerequisite {
                id,
                comment: BString::default(),
            },
        ],
        references: vec![
            Reference {
                id,
                name: "refs/heads/main".into(),
            },
            Reference {
                id,
                name: "refs/tags/v1".into(),
            },
        ],
    };
    let mut written = Vec::new();
    header.write_to(&mut written)?;
    assert_eq!(
        written.as_bstr(),
        format!(
            "# v3 git bundle\n@object-format=sha256\n@filter=blob:none\n-{SHA256} first\n-{SHA256} \n{SHA256} refs/heads/main\n{SHA256} refs/tags/v1\n\n"
        )
        .as_bytes()
        .as_bstr(),
        "every field is written in the form that git writes, with a space after a prerequisite without comment"
    );
    assert_eq!(
        Header::from_read(&mut written.as_slice())?,
        header,
        "it reads back as it was"
    );

    let id = ObjectId::from_hex(SHA1.as_bytes())?;
    let header = Header {
        version: Version::V2,
        object_hash: Kind::Sha1,
        filter: None,
        prerequisites: vec![Prerequisite {
            id,
            comment: "a commit".into(),
        }],
        references: vec![Reference {
            id,
            name: "HEAD".into(),
        }],
    };
    let mut written = Vec::new();
    header.write_to(&mut written)?;
    assert_eq!(
        written.as_bstr(),
        format!("# v2 git bundle\n-{SHA1} a commit\n{SHA1} HEAD\n\n")
            .as_bytes()
            .as_bstr(),
        "the v2 header is written in the form that git reads"
    );
    assert_eq!(
        Header::from_read(&mut written.as_slice())?,
        header,
        "it reads back as it was"
    );
    Ok(())
}

#[test]
fn refuses_to_write_headers_that_would_read_back_differently() -> TestResult {
    let sha1 = ObjectId::from_hex(SHA1.as_bytes())?;
    let sha256 = ObjectId::from_hex(SHA256.as_bytes())?;
    let valid = Header {
        version: Version::V3,
        object_hash: Kind::Sha1,
        filter: None,
        prerequisites: vec![Prerequisite {
            id: sha1,
            comment: "first".into(),
        }],
        references: vec![Reference {
            id: sha1,
            name: "refs/heads/main".into(),
        }],
    };
    valid.write_to(&mut Vec::new())?;

    let with = |change: &dyn Fn(&mut Header)| {
        let mut header = valid.clone();
        change(&mut header);
        header
    };
    for (what, header) in [
        (
            "a v2 header with a filter",
            with(&|header| {
                header.version = Version::V2;
                header.filter = Some("blob:none".into());
            }),
        ),
        (
            "a v2 header of SHA-256 ids",
            with(&|header| {
                header.version = Version::V2;
                header.object_hash = Kind::Sha256;
                header.prerequisites.clear();
                header.references[0].id = sha256;
            }),
        ),
        (
            "an empty filter",
            with(&|header| header.filter = Some(BString::default())),
        ),
        (
            "a filter with a line feed",
            with(&|header| header.filter = Some("blob:none\n".into())),
        ),
        (
            "a prerequisite of another hash",
            with(&|header| header.prerequisites[0].id = sha256),
        ),
        (
            "a reference of another hash",
            with(&|header| header.references[0].id = sha256),
        ),
        (
            "a reference without a name",
            with(&|header| header.references[0].name = BString::default()),
        ),
        (
            "a name with a line feed",
            with(&|header| header.references[0].name = "refs/heads/a\nb".into()),
        ),
        (
            "a name with a NUL byte",
            with(&|header| header.references[0].name = "refs/heads/a\0b".into()),
        ),
        (
            "a name that ends with a space",
            with(&|header| header.references[0].name = "refs/heads/main ".into()),
        ),
        (
            "a comment that ends with a tab",
            with(&|header| header.prerequisites[0].comment = "first\t".into()),
        ),
        (
            "a comment with a line feed",
            with(&|header| header.prerequisites[0].comment = "first\nsecond".into()),
        ),
    ] {
        let mut written = Vec::new();
        let err = header.write_to(&mut written).expect_err(what);
        assert!(err.is_validation(), "{what}: refused as invalid: {err:?}");
        assert!(written.is_empty(), "{what}: nothing was written");
    }
    Ok(())
}
