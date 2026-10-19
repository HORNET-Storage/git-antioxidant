//! Bundles that [`gix_bundle::create()`] writes are compared with those that `git bundle create` wrote for the same
//! references and hidden commits in [`HISTORY`], and given to git and to `unbundle()` in the repositories they are
//! meant for.

use std::{cell::RefCell, collections::BTreeSet, path::Path, sync::atomic::AtomicBool};

use bstr::ByteSlice;
use gix_bundle::{Header, Prerequisite, Reference, Version, create};
use gix_hash::ObjectId;
use gix_object::{Exists, Find};
use gix_pack::data::input::{BytesToEntriesIter, EntryDataMode, Mode};
use gix_testtools::{TestResult, scripted_fixture_writable};
use gix_utils::progress;

use crate::{Case, HISTORY, cases, history, ids, objects, unbundle::unbundle_into};

/// The cases in which a reference points into the history of the hidden commits, for which git writes another bundle
/// than [`create()`].
const UNLIKE_GIT: &[&str] = &[
    "tag-in-history",
    "tag-unrelated",
    "ref-at-boundary",
    "ref-is-hidden",
    "ref-in-history",
];

/// Objects of `source` in [`HISTORY`] that the tests name.
struct Revs {
    /// The root commit of `main`.
    a_commit_id: ObjectId,
    /// The commit that the annotated tag `v1` points to.
    b_commit_id: ObjectId,
    /// The commit that `main` points to.
    h_commit_id: ObjectId,
    /// The root commit of `lone`, which the annotated tag `lone-tag` points to.
    lone_root_commit_id: ObjectId,
    /// The tree `dir` of `A`, which `E` has as well.
    a_dir_tree_id: ObjectId,
    /// The blob `dir/b` of `A`, which `E` has as well.
    a_dir_b_blob_id: ObjectId,
}

fn revs(dir: &Path) -> TestResult<Revs> {
    let ids = ids(&dir.join("source.revs"))?;
    let [
        a_commit_id,
        b_commit_id,
        h_commit_id,
        lone_root_commit_id,
        a_dir_tree_id,
        a_dir_b_blob_id,
    ] = ids.as_slice()
    else {
        return Err(format!("source.revs lists {} objects instead of 6", ids.len()).into());
    };
    Ok(Revs {
        a_commit_id: *a_commit_id,
        b_commit_id: *b_commit_id,
        h_commit_id: *h_commit_id,
        lone_root_commit_id: *lone_root_commit_id,
        a_dir_tree_id: *a_dir_tree_id,
        a_dir_b_blob_id: *a_dir_b_blob_id,
    })
}

/// The case `name` of [`HISTORY`] in `dir`.
fn case_named(dir: &Path, name: &str) -> TestResult<Case> {
    cases(dir)?
        .into_iter()
        .find(|case| case.name == name)
        .ok_or_else(|| format!("{HISTORY} has no case {name}").into())
}

/// Write a bundle of `references` with the objects of `repository` in `dir` for repositories that have the `hidden`
/// commits, and return what was written along with the outcome.
fn write(
    dir: &Path,
    repository: &str,
    references: Vec<Reference>,
    hidden: Vec<ObjectId>,
    options: create::Options,
    should_interrupt: &AtomicBool,
) -> TestResult<(Vec<u8>, gix_error::Result<create::Outcome>)> {
    let mut bundle = Vec::new();
    let mut handle = objects(dir, repository)?.into_arc()?;
    // gix-pack looks up where objects are stored in packs, which a handle answers only if it keeps its packs loaded.
    handle.prevent_pack_unload();
    let outcome = gix_bundle::create(
        handle,
        references,
        hidden,
        &mut bundle,
        &mut progress::Discard,
        should_interrupt,
        options,
    );
    Ok((bundle, outcome))
}

/// Write the bundle of `case` with the objects of `source` in `dir`.
fn write_case(dir: &Path, case: &Case) -> TestResult<(Vec<u8>, create::Outcome)> {
    let options = create::Options {
        version: case.version,
        ..Default::default()
    };
    let (bundle, outcome) = write(
        dir,
        "source",
        case.references.clone(),
        case.hidden.clone(),
        options,
        &AtomicBool::new(false),
    )?;
    Ok((bundle, outcome?))
}

/// What a bundle contains.
struct Contents {
    header: Header,
    /// The ids of the objects in the pack.
    object_ids: BTreeSet<ObjectId>,
}

/// Read `bundle`, which must end with its pack, taking the bases that a thin pack lacks from `bases`.
fn read_bundle(bundle: &[u8], bases: Option<gix_odb::Handle>) -> TestResult<Contents> {
    let mut rest = bundle;
    let header = Header::from_read(&mut rest)?;
    let bases = bases.map(|objects| Bases {
        objects,
        used: RefCell::default(),
    });
    let directory = gix_testtools::tempfile::tempdir()?;
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut rest,
        Some(directory.path()),
        &mut progress::Discard,
        &AtomicBool::new(false),
        bases.as_ref(),
        header.object_hash,
        Default::default(),
    )?;
    assert!(rest.is_empty(), "the bundle ends with its pack");
    // Completing a thin pack adds the bases that it lacks.
    let added = bases.map(|bases| bases.used.into_inner()).unwrap_or_default();
    let object_ids = match outcome.index_path {
        Some(index_path) => gix_pack::index::File::at(index_path, header.object_hash)?
            .iter()
            .map(|entry| entry.oid)
            .filter(|id| !added.contains(id))
            .collect(),
        // A pack without objects gets no index.
        None => BTreeSet::new(),
    };
    Ok(Contents { header, object_ids })
}

/// The objects that complete thin packs, with the ids of those that were used.
struct Bases {
    objects: gix_odb::Handle,
    used: RefCell<BTreeSet<ObjectId>>,
}

impl Find for Bases {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix_error::Result<Option<gix_object::Data<'a>>> {
        let data = self.objects.try_find(id, buffer)?;
        if data.is_some() {
            self.used.borrow_mut().insert(id.to_owned());
        }
        Ok(data)
    }
}

/// What `git bundle create` wrote for `case` in `dir`, unless it refused to, with the bases that its pack lacks taken
/// from `source`.
fn read_git_bundle(dir: &Path, case: &Case) -> TestResult<Option<Contents>> {
    let path = dir.join(format!("{}.bundle", case.name));
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_bundle(&std::fs::read(path)?, Some(objects(dir, "source")?))?))
}

/// `header` with its prerequisites in the order of their ids, as git may list them in another order.
fn sorted(mut header: Header) -> Header {
    header.prerequisites.sort_by_key(|prerequisite| prerequisite.id);
    header
}

#[test]
fn agrees_with_git_bundle_create() -> TestResult {
    let dir = history()?;
    for case in cases(&dir)?
        .iter()
        .filter(|case| !UNLIKE_GIT.contains(&case.name.as_str()))
    {
        let name = &case.name;
        let ours = read_bundle(&write_case(&dir, case)?.0, None)?;
        let git = read_git_bundle(&dir, case)?.ok_or_else(|| format!("{name}: git wrote no bundle"))?;
        assert_eq!(
            sorted(ours.header),
            sorted(git.header),
            "{name}: the header has the references and prerequisites that git wrote"
        );
        assert_eq!(
            ours.object_ids, git.object_ids,
            "{name}: the pack carries the objects of git's pack"
        );
        let listed: BTreeSet<_> = ids(&dir.join(format!("{name}.objects")))?.into_iter().collect();
        assert_eq!(
            ours.object_ids, listed,
            "{name}: the pack carries the objects that git rev-list --objects lists"
        );
    }
    Ok(())
}

/// git doesn't require the commit that a tag points to if it is in the history of the hidden commits, so that its
/// bundle passes verification in a repository that lacks the commit, while fetching from it fails there.
#[test]
fn requires_the_commits_that_tags_point_to() -> TestResult {
    let dir = history()?;
    let revs = revs(&dir)?;
    for (name, commit_id, comment) in [
        ("tag-in-history", revs.b_commit_id, "B subject second line of subject"),
        ("tag-unrelated", revs.lone_root_commit_id, "Lone"),
    ] {
        let case = case_named(&dir, name)?;
        let ours = read_bundle(&write_case(&dir, &case)?.0, None)?;
        let mut expected = read_git_bundle(&dir, &case)?.ok_or_else(|| format!("{name}: git wrote no bundle"))?;
        expected.header.prerequisites.push(Prerequisite {
            id: commit_id,
            comment: comment.into(),
        });
        assert_eq!(
            sorted(ours.header),
            sorted(expected.header),
            "{name}: the commit of the tag is a prerequisite as well"
        );
        assert_eq!(
            ours.object_ids, expected.object_ids,
            "{name}: the pack carries the objects of git's pack"
        );
    }
    Ok(())
}

/// git leaves out the reference `side`, whose commit `S1` is hidden.
#[test]
fn keeps_a_reference_to_a_hidden_commit() -> TestResult {
    let dir = history()?;
    let case = case_named(&dir, "ref-at-boundary")?;
    let ours = read_bundle(&write_case(&dir, &case)?.0, None)?;
    let mut expected = read_git_bundle(&dir, &case)?.ok_or("git wrote a bundle")?;
    expected.header.references = case.references.clone();
    assert_eq!(
        sorted(ours.header),
        sorted(expected.header),
        "`side` is kept, and its commit is a prerequisite already"
    );
    assert_eq!(
        ours.object_ids, expected.object_ids,
        "the pack carries the objects of git's pack"
    );
    Ok(())
}

/// git refuses to write a bundle whose only reference points to a hidden commit, as it leaves out the reference.
#[test]
fn writes_a_bundle_whose_only_reference_is_hidden() -> TestResult {
    let dir = history()?;
    let revs = revs(&dir)?;
    let case = case_named(&dir, "ref-is-hidden")?;
    assert!(
        read_git_bundle(&dir, &case)?.is_none(),
        "git refused to write the bundle"
    );

    let (bundle, outcome) = write_case(&dir, &case)?;
    let ours = read_bundle(&bundle, None)?;
    assert_eq!(ours.header.references, case.references, "`main` is kept");
    assert_eq!(
        ours.header.prerequisites,
        vec![Prerequisite {
            id: revs.h_commit_id,
            comment: "H".into(),
        }],
        "the commit of `main` is the only prerequisite"
    );
    assert_eq!(
        (outcome.num_objects, ours.object_ids.len()),
        (0, 0),
        "the pack carries no objects"
    );
    Ok(())
}

/// git leaves out the reference `A`, whose commit is in the history of the hidden commit `D`, and sends what `E` shares
/// with `A`, as it doesn't count on the trees of commits that aren't prerequisites.
#[test]
fn leaves_out_what_the_commits_of_references_provide() -> TestResult {
    let dir = history()?;
    let revs = revs(&dir)?;
    let case = case_named(&dir, "ref-in-history")?;
    let ours = read_bundle(&write_case(&dir, &case)?.0, None)?;
    let mut expected = read_git_bundle(&dir, &case)?.ok_or("git wrote a bundle")?;
    expected.header.references = case.references.clone();
    expected.header.prerequisites.push(Prerequisite {
        id: revs.a_commit_id,
        comment: "A".into(),
    });
    for id in [revs.a_dir_tree_id, revs.a_dir_b_blob_id] {
        assert!(
            expected.object_ids.remove(&id),
            "git's pack carries {id}, which `E` shares with `A`"
        );
    }
    assert_eq!(
        sorted(ours.header),
        sorted(expected.header),
        "`A` is kept, and its commit is a prerequisite"
    );
    assert_eq!(
        ours.object_ids, expected.object_ids,
        "the pack leaves out the objects of the tree of `A`"
    );
    Ok(())
}

#[test]
fn git_fetches_every_bundle() -> TestResult {
    let dir = history()?;
    for case in cases(&dir)? {
        let name = &case.name;
        let copy = scripted_fixture_writable(HISTORY)?;
        let path = copy.path().join(format!("{name}.created.bundle"));
        std::fs::write(&path, write_case(&dir, &case)?.0)?;
        let receiver = copy.path().join(&case.receiver);

        let verify = gix_testtools::git_command(&receiver)
            .args(["bundle", "verify", "-q"])
            .arg(&path)
            .output()?;
        assert!(
            verify.status.success(),
            "{name}: git bundle verify accepts the bundle in {}: {}",
            case.receiver,
            verify.stderr.as_bstr()
        );
        // Fetching from a bundle keeps a pack, whose path with a SHA-256 name can exceed Windows' traditional path
        // limit.
        let fetch = gix_testtools::git_command(&receiver)
            .args(["-c", "core.longpaths=true", "fetch", "-q"])
            .arg(&path)
            .arg("refs/*:refs/fetched/*")
            .output()?;
        assert!(
            fetch.status.success(),
            "{name}: git fetch accepts the bundle in {}: {}",
            case.receiver,
            fetch.stderr.as_bstr()
        );
        for reference in &case.references {
            let fetched_name = reference
                .name
                .strip_prefix(b"refs/")
                .ok_or("the references of the cases are in refs/")?
                .as_bstr();
            let fetched_id = gix_testtools::git(&receiver, &format!("rev-parse --verify refs/fetched/{fetched_name}"))?;
            assert_eq!(
                fetched_id.trim(),
                reference.id.to_string(),
                "{name}: git fetched {} as it was written",
                reference.name
            );
        }
    }
    Ok(())
}

#[test]
fn unbundles_every_bundle() -> TestResult {
    let dir = history()?;
    for case in cases(&dir)? {
        let name = &case.name;
        let (bundle, outcome) = write_case(&dir, &case)?;
        let copy = scripted_fixture_writable(HISTORY)?;
        let mut rest = bundle.as_slice();
        let header = Header::from_read(&mut rest)?;
        let unbundled = unbundle_into(
            copy.path(),
            &case.receiver,
            &header,
            &mut rest,
            gix_testtools::object_hash(),
            &AtomicBool::new(false),
        )?
        .map_err(|err| format!("{name}: the bundle can't be unbundled in {}: {err:?}", case.receiver))?;
        assert_eq!(
            unbundled.index.num_objects, outcome.num_objects,
            "{name}: every object of the pack was indexed"
        );
        let objects = objects(copy.path(), &case.receiver)?;
        for reference in &case.references {
            assert!(
                objects.exists(&reference.id),
                "{name}: the object of {} is present",
                reference.name
            );
        }
    }
    Ok(())
}

#[test]
fn copies_deltas_whose_bases_are_in_the_bundle() -> TestResult {
    let dir = history()?;
    for case in cases(&dir)? {
        let name = &case.name;
        let (bundle, _) = write_case(&dir, &case)?;
        let mut pack = bundle.as_slice();
        Header::from_read(&mut pack)?;
        let (mut offset_deltas, mut ref_delta_base_ids) = (0, Vec::new());
        for entry in BytesToEntriesIter::new_from_header(
            pack,
            Mode::Verify,
            EntryDataMode::Ignore,
            gix_testtools::object_hash(),
        )? {
            match entry?.header {
                gix_pack::data::entry::Header::OfsDelta { .. } => offset_deltas += 1,
                gix_pack::data::entry::Header::RefDelta { base_id } => ref_delta_base_ids.push(base_id),
                _ => {}
            }
        }
        assert_eq!(
            ref_delta_base_ids,
            Vec::<ObjectId>::new(),
            "{name}: no delta names its base by id, as those of thin packs do"
        );
        if name == "full" {
            assert!(
                offset_deltas > 0,
                "full: the deltas of the pack of `source` are copied, as the bundle carries all of their bases"
            );
        }
    }
    Ok(())
}

/// git provides the empty tree whether or not a repository stores it.
#[test]
fn carries_the_empty_tree_that_the_repository_lacks() -> TestResult {
    let dir = history()?;
    let empty_tree_id = ObjectId::empty_tree(gix_testtools::object_hash());
    assert!(
        !objects(&dir, "source")?.exists(&empty_tree_id),
        "`source` doesn't store the empty tree"
    );
    let case = case_named(&dir, "empty-tree")?;
    let ours = read_bundle(&write_case(&dir, &case)?.0, None)?;
    assert!(
        ours.object_ids.contains(&empty_tree_id),
        "the pack carries the empty tree, which the commit of `empty` has"
    );
    Ok(())
}

#[test]
fn reports_what_it_wrote() -> TestResult {
    let dir = history()?;
    let case = case_named(&dir, "incremental-B")?;
    let (bundle, outcome) = write_case(&dir, &case)?;
    let contents = read_bundle(&bundle, None)?;
    assert_eq!(
        outcome.header, contents.header,
        "the header is the one that was written"
    );
    assert_eq!(
        outcome.num_objects as usize,
        contents.object_ids.len(),
        "the amount of objects is that of the pack"
    );
    let checksum = outcome.pack_checksum.as_slice();
    assert_eq!(
        &bundle[bundle.len() - checksum.len()..],
        checksum,
        "the checksum of the pack ends the bundle"
    );
    Ok(())
}

#[test]
fn stops_when_interrupted() -> TestResult {
    let dir = history()?;
    let case = case_named(&dir, "full")?;
    let (bundle, outcome) = write(
        &dir,
        "source",
        case.references,
        case.hidden,
        Default::default(),
        &AtomicBool::new(true),
    )?;
    let err = outcome.expect_err("the interrupt is noticed while the commits are selected");
    assert!(err.is_cancelled(), "the interrupt is reported as cancellation: {err:?}");
    assert!(bundle.is_empty(), "nothing was written");
    Ok(())
}

#[test]
fn refuses_bundles_it_cannot_write() -> TestResult {
    let dir = history()?;
    let revs = revs(&dir)?;
    let object_hash = gix_testtools::object_hash();
    let other_hash = gix_hash::Kind::all()
        .iter()
        .copied()
        .find(|kind| *kind != object_hash)
        .expect("both object hashes are compiled in");
    let reference = |id: ObjectId, name: &str| Reference { id, name: name.into() };
    let main = reference(revs.h_commit_id, "refs/heads/main");
    let cases = [
        ("no reference", vec![], vec![], None),
        (
            "a name twice",
            vec![main.clone(), reference(revs.b_commit_id, "refs/heads/main")],
            vec![],
            None,
        ),
        ("an empty name", vec![reference(revs.h_commit_id, "")], vec![], None),
        (
            "a name with a line feed",
            vec![reference(revs.h_commit_id, "refs/heads/a\nb")],
            vec![],
            None,
        ),
        (
            "references of two object hashes",
            vec![main.clone(), reference(ObjectId::null(other_hash), "refs/heads/other")],
            vec![],
            None,
        ),
        (
            "a hidden commit of another object hash",
            vec![main.clone()],
            vec![ObjectId::null(other_hash)],
            None,
        ),
        ("a hidden tree", vec![main.clone()], vec![revs.a_dir_tree_id], None),
        (
            "a v2 bundle of SHA-256 objects",
            vec![reference(ObjectId::null(gix_hash::Kind::Sha256), "refs/heads/main")],
            vec![],
            Some(Version::V2),
        ),
    ];
    for (what, references, hidden, version) in cases {
        let options = create::Options {
            version,
            ..Default::default()
        };
        let (bundle, outcome) = write(&dir, "source", references, hidden, options, &AtomicBool::new(false))?;
        let err = outcome.err().ok_or_else(|| format!("{what}: the bundle is refused"))?;
        assert!(err.is_validation(), "{what}: the bundle is refused as invalid: {err:?}");
        assert!(bundle.is_empty(), "{what}: nothing is written");
    }
    Ok(())
}

#[test]
fn fails_if_an_object_is_missing() -> TestResult {
    let dir = history()?;
    let revs = revs(&dir)?;
    let object_hash = gix_testtools::object_hash();
    let missing_id = ObjectId::from_hex("1".repeat(object_hash.len_in_hex()).as_bytes())?;
    let broken_commit_id = *ids(&dir.join("broken.tips"))?.first().ok_or("`broken` has a commit")?;
    let main = |id: ObjectId| Reference {
        id,
        name: "refs/heads/main".into(),
    };
    let cases = [
        ("broken", main(broken_commit_id), vec![], "a blob of the commit"),
        ("source", main(missing_id), vec![], "the object of the reference"),
        ("source", main(revs.h_commit_id), vec![missing_id], "a hidden commit"),
    ];
    for (repository, reference, hidden, what) in cases {
        let (bundle, outcome) = write(
            &dir,
            repository,
            vec![reference],
            hidden,
            Default::default(),
            &AtomicBool::new(false),
        )?;
        let err = outcome.err().ok_or_else(|| format!("{what} is missing, which fails"))?;
        assert!(err.is_not_found(), "{what} is missing, which is reported: {err:?}");
        assert!(bundle.is_empty(), "{what} is missing: nothing is written");
    }
    Ok(())
}
