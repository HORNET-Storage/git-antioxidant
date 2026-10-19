//! Most bundles of these tests are made by `git bundle create`. One fixture also records what `git bundle verify` and
//! `git fetch` do with each bundle, which the results of this crate are compared with. Another records what
//! `git bundle create` writes for references of a history, which the bundles that this crate creates are compared with.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use bstr::ByteSlice;
use gix_bundle::{Header, Reference, Version};
use gix_hash::ObjectId;
use gix_testtools::TestResult;

mod create;
mod header;
mod unbundle;
mod verify;

/// The script that makes the bundles, the repositories that receive them and the record of what git does with them.
const FIXTURE: &str = "make_bundles.sh";

/// The bundles that `git bundle create` wrote.
const GIT_BUNDLES: &[&str] = &["full", "incremental", "with-tag", "incremental-v3", "filtered"];

fn fixture() -> TestResult<PathBuf> {
    Ok(gix_testtools::scripted_fixture_read_only(FIXTURE)?)
}

/// The script that records what `git bundle create` writes for references of a history, as the cases that [`cases()`]
/// reads, along with the repositories that receive the bundles.
const HISTORY: &str = "make_history.sh";

fn history() -> TestResult<PathBuf> {
    Ok(gix_testtools::scripted_fixture_read_only(HISTORY)?)
}

/// A bundle that provides `references` of the repository `source` of [`HISTORY`] to repositories like `receiver`, which
/// have the `hidden` commits along with their history.
struct Case {
    /// The name of the case, which `git bundle create` wrote `<name>.bundle` for, unless it refused to.
    name: String,
    receiver: String,
    references: Vec<Reference>,
    hidden: Vec<ObjectId>,
    /// The version of the format that git was asked to write, or `None` if git chose it.
    version: Option<Version>,
}

/// The cases of [`HISTORY`] in `dir`.
fn cases(dir: &Path) -> TestResult<Vec<Case>> {
    std::fs::read_to_string(dir.join("cases"))?
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let [name, receiver, version] = fields.as_slice() else {
                return Err(format!("cases has the malformed line {line:?}").into());
            };
            Ok(Case {
                name: (*name).to_owned(),
                receiver: (*receiver).to_owned(),
                references: references(&dir.join(format!("{name}.refs")))?,
                hidden: ids(&dir.join(format!("{name}.hidden")))?,
                version: match *version {
                    "default" => None,
                    "2" => Some(Version::V2),
                    "3" => Some(Version::V3),
                    other => return Err(format!("cases names the unknown version {other:?}").into()),
                },
            })
        })
        .collect()
}

/// The references that the file at `path` lists as `<id> <name>`, as `git bundle list-heads` does.
fn references(path: &Path) -> TestResult<Vec<Reference>> {
    std::fs::read(path)?
        .lines()
        .map(|line| {
            let (hex, name) = line
                .split_once_str(" ")
                .ok_or("a reference is listed as an object id and a name")?;
            Ok(Reference {
                id: ObjectId::from_hex(hex)?,
                name: name.into(),
            })
        })
        .collect()
}

/// Read the header of the bundle `name` in `dir`, and return it with the rest of the bundle.
fn open_bundle(dir: &Path, name: &str) -> TestResult<(Header, BufReader<File>)> {
    let mut read = BufReader::new(File::open(dir.join(format!("{name}.bundle")))?);
    let header = Header::from_read(&mut read)?;
    Ok((header, read))
}

/// The objects of the repository `receiver` in `dir`.
fn objects(dir: &Path, receiver: &str) -> TestResult<gix_odb::Handle> {
    Ok(gix_odb::at(
        dir.join(receiver).join(".git").join("objects"),
        gix_testtools::object_hash(),
    )?)
}

/// The objects that the references of `receiver` in `dir` point to, as `git rev-parse --all` lists them.
fn tips(dir: &Path, receiver: &str) -> TestResult<Vec<ObjectId>> {
    ids(&dir.join(format!("{receiver}.tips")))
}

/// The object id that starts each line of the file at `path`.
fn ids(path: &Path) -> TestResult<Vec<ObjectId>> {
    std::fs::read_to_string(path)?
        .lines()
        .map(|line| {
            let hex = line.split(' ').next().unwrap_or_default();
            Ok(ObjectId::from_hex(hex.as_bytes())?)
        })
        .collect()
}

/// Objects of the repository the bundles were made from.
struct Revs {
    /// The first commit, which the bundles of `base..main` need.
    base_commit_id: ObjectId,
    /// The tree of `base`.
    base_tree_id: ObjectId,
    /// The blob `large` of `base`, of which the bundles of `base..main` store a delta.
    base_large_blob_id: ObjectId,
    /// The last commit.
    main_commit_id: ObjectId,
    /// The annotated tag of the second commit.
    v1_tag_id: ObjectId,
}

fn revs(dir: &Path) -> TestResult<Revs> {
    let ids = ids(&dir.join("source.revs"))?;
    let [
        base_commit_id,
        base_tree_id,
        base_large_blob_id,
        main_commit_id,
        v1_tag_id,
    ] = ids.as_slice()
    else {
        return Err(format!("source.revs lists {} objects instead of 5", ids.len()).into());
    };
    Ok(Revs {
        base_commit_id: *base_commit_id,
        base_tree_id: *base_tree_id,
        base_large_blob_id: *base_large_blob_id,
        main_commit_id: *main_commit_id,
        v1_tag_id: *v1_tag_id,
    })
}

/// What git did with a bundle in a receiving repository.
struct GitResult {
    receiver: String,
    bundle: String,
    /// Whether `git bundle verify` accepted the bundle.
    verify: bool,
    /// Whether `git fetch` accepted the bundle.
    fetch: bool,
}

/// What git did with every bundle in every receiving repository.
fn git_results(dir: &Path) -> TestResult<Vec<GitResult>> {
    let results = std::fs::read_to_string(dir.join("git-results"))?
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let [receiver, bundle, verify, fetch] = fields.as_slice() else {
                return Err(format!("git-results has the malformed line {line:?}").into());
            };
            Ok(GitResult {
                receiver: (*receiver).to_owned(),
                bundle: (*bundle).to_owned(),
                verify: is_ok(verify)?,
                fetch: is_ok(fetch)?,
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    assert_eq!(
        results.len(),
        3 * 7,
        "each of the 7 bundles was tried in each of the 3 receivers"
    );
    Ok(results)
}

fn is_ok(result: &str) -> TestResult<bool> {
    match result {
        "ok" => Ok(true),
        "fail" => Ok(false),
        other => Err(format!("git-results has the unknown result {other:?}").into()),
    }
}
