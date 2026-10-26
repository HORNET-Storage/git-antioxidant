//! The object checks against Git's verdicts on the objects that `make_object_checks.sh` makes: what `git fsck` reports
//! with and without `--strict`, what `git index-pack --fsck-objects` prints for packs of them, and what
//! `git hash-object` prints for commits and tags, among them some that Git can't parse.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use bstr::{BString, ByteSlice};
use gix_fsck::object::{Checker, Level, MessageId, Options, Outcome, Report, Severity};
use gix_hash::ObjectId;
use gix_object::{Find, Kind};
use gix_testtools::TestResult;

/// The directory with the objects and Git's verdicts on them, or `None` if the host's Git is older than Git 2.55, whose
/// checks these follow and which earlier versions make differently or don't make at all.
fn fixture() -> TestResult<Option<PathBuf>> {
    if gix_testtools::should_skip_as_git_version_is_smaller_than(2, 55, 0) {
        return Ok(None);
    }
    Ok(Some(gix_testtools::scripted_fixture_read_only(
        "make_object_checks.sh",
    )?))
}

/// The objects of `objects.git` in the order of `objects.list`, each once, and the cases they belong to.
struct Objects {
    db: gix_odb::Handle,
    ids: Vec<ObjectId>,
    cases: HashMap<ObjectId, Vec<String>>,
}

impl Objects {
    fn at(dir: &Path) -> TestResult<Self> {
        let mut db = gix_odb::at(dir.join("objects.git").join("objects"), gix_testtools::object_hash())?;
        db.refresh_never();
        let mut ids = Vec::new();
        let mut cases: HashMap<ObjectId, Vec<String>> = HashMap::new();
        for line in std::fs::read_to_string(dir.join("objects.list"))?.lines() {
            let mut fields = line.splitn(3, ' ');
            let (Some(id), Some(_kind), Some(case)) = (fields.next(), fields.next(), fields.next()) else {
                return Err(format!("unexpected line in objects.list: {line:?}").into());
            };
            let id = ObjectId::from_hex(id.as_bytes())?;
            let names = cases.entry(id).or_default();
            if names.is_empty() {
                ids.push(id);
            }
            names.push(case.to_owned());
        }
        Ok(Objects { db, ids, cases })
    }

    /// The kind and content of the object `id`.
    fn get(&self, id: &ObjectId) -> TestResult<(Kind, Vec<u8>)> {
        let mut buf = Vec::new();
        let object = self
            .db
            .try_find(id, &mut buf)?
            .ok_or_else(|| format!("object {id} is missing"))?;
        Ok((object.kind, object.data.to_vec()))
    }

    /// The cases that the object `id` belongs to, for messages.
    fn cases_of(&self, id: &ObjectId) -> String {
        self.cases
            .get(id)
            .map_or_else(|| "no case".into(), |cases| cases.join(", "))
    }
}

/// The objects of a pack, from which `git index-pack` reads the blobs that are still queued at the end.
struct Pack(HashMap<ObjectId, (Kind, Vec<u8>)>);

impl Find for Pack {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix_error::Result<Option<gix_object::Data<'a>>> {
        Ok(self.0.get(id).map(|(kind, data)| {
            buffer.clear();
            buffer.extend_from_slice(data);
            gix_object::Data {
                kind: *kind,
                object_hash: id.kind(),
                data: buffer.as_slice(),
            }
        }))
    }
}

/// A report as `git fsck` prints it.
fn fsck_line(report: &Report) -> BString {
    let level = match report.level {
        Level::Error => "error",
        Level::Warning => "warning",
    };
    let mut line = BString::from(format!("{level} in {} {}: {}: ", report.kind, report.object, report.id));
    line.extend_from_slice(&report.message);
    line
}

/// The reports in what `git fsck` printed to its standard error, by object: the lines
/// `error in <type> <id>: <check>: <message>` and `warning in …`, without its other errors about objects that it can't
/// read or reach.
fn parse_fsck_reports(stderr: &[u8]) -> TestResult<BTreeMap<ObjectId, Vec<BString>>> {
    let hex_len = gix_testtools::object_hash().len_in_hex();
    let mut reports: BTreeMap<ObjectId, Vec<BString>> = BTreeMap::new();
    for line in stderr.split(|&byte| byte == b'\n') {
        let Some(rest) = line
            .strip_prefix(b"error in ")
            .or_else(|| line.strip_prefix(b"warning in "))
        else {
            continue;
        };
        let Some((_kind, rest)) = rest.split_once_str(" ") else {
            continue;
        };
        let (Some(hex), Some(rest)) = (rest.get(..hex_len), rest.get(hex_len..)) else {
            continue;
        };
        let Some((name, _message)) = rest.strip_prefix(b": ").and_then(|rest| rest.split_once_str(": ")) else {
            continue;
        };
        if MessageId::ALL.iter().any(|id| id.name().as_bytes() == name) {
            reports.entry(ObjectId::from_hex(hex)?).or_default().push(line.into());
        }
    }
    Ok(reports)
}

/// The reports of checking each object once and then the blobs that are still queued, as `git fsck` does with
/// `options`, by object.
fn fsck_reports(objects: &Objects, options: Options) -> TestResult<BTreeMap<ObjectId, Vec<BString>>> {
    let mut checker = Checker::new(options, gix_testtools::object_hash());
    let mut outcomes = Vec::new();
    for id in &objects.ids {
        let (kind, data) = objects.get(id)?;
        outcomes.push(checker.check(id, kind, &data));
    }
    outcomes.push(checker.finish(&objects.db)?);
    let mut reports: BTreeMap<ObjectId, Vec<BString>> = BTreeMap::new();
    for report in outcomes.iter().flat_map(|outcome| &outcome.reports) {
        reports.entry(report.object).or_default().push(fsck_line(report));
    }
    Ok(reports)
}

fn assert_fsck_reports_match(file: &str, options: Options) -> TestResult {
    let Some(dir) = fixture()? else {
        return Ok(());
    };
    let objects = Objects::at(&dir)?;
    let expected = parse_fsck_reports(&std::fs::read(dir.join(file))?)?;
    assert!(
        expected.len() > 100,
        "Git reports about many objects, not {}",
        expected.len()
    );
    let actual = fsck_reports(&objects, options)?;
    let ids: BTreeSet<_> = expected.keys().chain(actual.keys()).collect();
    let mismatches: Vec<_> = ids
        .into_iter()
        .filter(|id| expected.get(*id) != actual.get(*id))
        .map(|id| {
            format!(
                "{id} ({}):\n  Git: {:?}\n  gix: {:?}",
                objects.cases_of(id),
                expected.get(id),
                actual.get(id)
            )
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} objects with other reports than {file}:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    Ok(())
}

#[test]
fn reports_are_those_of_git_fsck() -> TestResult {
    assert_fsck_reports_match("fsck.err", Options::default())
}

#[test]
fn reports_are_those_of_git_fsck_strict() -> TestResult {
    assert_fsck_reports_match("fsck-strict.err", Options::strict())
}

/// What a Git command printed and its exit code.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    stdout: BString,
    stderr: BString,
    exit: i32,
}

impl Verdict {
    fn success() -> Self {
        Verdict {
            stdout: BString::default(),
            stderr: BString::default(),
            exit: 0,
        }
    }

    /// Print `prefix` and `message` to the standard error as Git's `vreportf()` does: cut to 4095 bytes, with control
    /// characters other than tabs and newlines in `message` replaced by `?`, and followed by a newline.
    fn print(&mut self, prefix: &str, message: &[u8]) {
        let mut line = BString::from(prefix);
        line.extend_from_slice(message);
        line.truncate(4095);
        for byte in &mut line[prefix.len()..] {
            if byte.is_ascii_control() && *byte != b'\t' && *byte != b'\n' {
                *byte = b'?';
            }
        }
        self.stderr.extend_from_slice(&line);
        self.stderr.push(b'\n');
    }

    /// End with Git's fatal error `message`.
    fn die(mut self, message: &[u8]) -> Self {
        self.print("fatal: ", message);
        self.exit = 128;
        self
    }
}

/// Check the objects that `dir/objects` lists in a pack as `git index-pack --fsck-objects=<dir/options>` does: in
/// strict mode, with blobs larger than the number of bytes in `dir/threshold` read as large blobs, which it checks
/// after the other objects.
fn index_pack(dir: &Path, objects: &Objects) -> TestResult<Verdict> {
    let mut verdict = Verdict::success();
    let mut options = Options::strict();
    if let Err(message) = set_options(&mut options, &std::fs::read(dir.join("options"))?, dir) {
        return Ok(verdict.die(&message));
    }
    let threshold = match std::fs::read_to_string(dir.join("threshold")) {
        Ok(threshold) => Some(threshold.trim().parse::<usize>()?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    let mut pack = Vec::new();
    for line in std::fs::read_to_string(dir.join("objects"))?.lines() {
        let id = ObjectId::from_hex(line.as_bytes())?;
        let (kind, data) = objects.get(&id)?;
        pack.push((id, kind, data));
    }
    let is_large =
        |kind: Kind, data: &[u8]| kind == Kind::Blob && threshold.is_some_and(|threshold| data.len() > threshold);
    let mut checker = Checker::new(options, gix_testtools::object_hash());
    let outcomes = pack
        .iter()
        .filter(|(_, kind, data)| !is_large(*kind, data))
        .map(|(id, kind, data)| (id, Some((*kind, data))))
        .chain(
            pack.iter()
                .filter(|(_, kind, data)| is_large(*kind, data))
                .map(|(id, _, _)| (id, None)),
        );
    for (id, object) in outcomes {
        let outcome = match object {
            Some((kind, data)) => checker.check(id, kind, data),
            None => checker.check_large_blob(id),
        };
        print_index_pack_reports(&mut verdict, &outcome);
        if outcome.failed {
            return Ok(verdict.die(b"fsck error in packed object"));
        }
    }
    let pack = Pack(pack.into_iter().map(|(id, kind, data)| (id, (kind, data))).collect());
    let outcome = checker.finish(&pack)?;
    print_index_pack_reports(&mut verdict, &outcome);
    // The missing `.gitmodules` blobs that `git index-pack` prints for a later check don't fail the pack.
    let failed = outcome
        .reports
        .iter()
        .any(|report| report.level == Level::Error && report.id != MessageId::GitmodulesMissing);
    Ok(if failed {
        verdict.die(b"fsck error in pack objects")
    } else {
        verdict
    })
}

/// Print the reports of `outcome` as `git index-pack` does, which prints the IDs of missing `.gitmodules` blobs to its
/// standard output instead.
fn print_index_pack_reports(verdict: &mut Verdict, outcome: &Outcome) {
    for report in &outcome.reports {
        if report.id == MessageId::GitmodulesMissing {
            verdict
                .stdout
                .extend_from_slice(format!("{}\n", report.object).as_bytes());
            continue;
        }
        let prefix = match report.level {
            Level::Error => "error: ",
            Level::Warning => "warning: ",
        };
        let mut message = BString::from(format!("object {}: {}: ", report.object, report.id));
        message.extend_from_slice(&report.message);
        verdict.print(prefix, &message);
    }
}

/// Apply the options of `git index-pack --fsck-objects=<values>` as Git's `fsck_set_msg_types()` does, with the skip
/// lists in `dir`, or return the message with which Git stops.
fn set_options(options: &mut Options, values: &[u8], dir: &Path) -> Result<(), BString> {
    let message = |err: gix_error::Error| BString::from(err.to_string());
    for setting in values
        .split(|byte| b" ,|".contains(byte))
        .filter(|setting| !setting.is_empty())
    {
        let separator = setting.iter().position(|&byte| byte == b'=' || byte == b':');
        let name = &setting[..separator.unwrap_or(setting.len())];
        let value = separator.map(|separator| &setting[separator + 1..]);
        if name.eq_ignore_ascii_case(b"skiplist") {
            let path = value.ok_or("skiplist requires a path")?;
            // Paths are relative to the repository, which is a directory in `dir`.
            let path = path
                .strip_prefix(b"../")
                .and_then(|path| path.to_str().ok())
                .expect("skip lists are next to the repository");
            let list = std::fs::read(dir.join(path)).expect("skip lists can be read");
            options
                .add_skip_list(&list, gix_testtools::object_hash())
                .map_err(message)?;
        } else {
            let value = value.ok_or_else(|| format!("Missing '=': '{}'", name.to_ascii_lowercase().as_bstr()))?;
            options
                .set_severity_from_config(name.as_bstr(), value.as_bstr())
                .map_err(message)?;
        }
    }
    Ok(())
}

#[test]
fn index_pack_verdicts_are_those_of_git() -> TestResult {
    let Some(dir) = fixture()? else {
        return Ok(());
    };
    let objects = Objects::at(&dir)?;
    let mut cases = std::fs::read_dir(dir.join("index-pack"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    cases.sort();
    assert!(cases.len() > 50, "there are many packs, not {}", cases.len());
    let mut mismatches = Vec::new();
    for case in &cases {
        let stdout: Vec<u8> = std::fs::read(case.join("stdout"))?
            .lines_with_terminator()
            .filter(|line| !line.starts_with(b"pack\t"))
            .flatten()
            .copied()
            .collect();
        let expected = Verdict {
            stdout: stdout.into(),
            stderr: std::fs::read(case.join("stderr"))?.into(),
            exit: std::fs::read_to_string(case.join("exit"))?.trim().parse()?,
        };
        let actual = index_pack(case, &objects)?;
        if actual != expected {
            mismatches.push(format!(
                "{}:\n  Git: {expected:?}\n  gix: {actual:?}",
                case.file_name().unwrap_or_default().display()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} packs with other verdicts than Git's:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    Ok(())
}

/// Check `data` as `git hash-object -t <kind>` does before writing it: in strict mode, with a callback that prints each
/// report as an error and fails the object, as if each check with the default severity `info` reported errors.
fn hash_object(kind: Kind, data: &[u8]) -> TestResult<Verdict> {
    let object_hash = gix_testtools::object_hash();
    let mut options = Options::strict();
    for &id in MessageId::ALL {
        if id.default_severity() == Severity::Info {
            options.set_severity(id, Severity::Error)?;
        }
    }
    let outcome = Checker::new(options, object_hash).check(&ObjectId::null(object_hash), kind, data);
    let mut verdict = Verdict::success();
    for report in &outcome.reports {
        let mut message = BString::from(format!("object fails fsck: {}: ", report.id));
        message.extend_from_slice(&report.message);
        verdict.print("error: ", &message);
    }
    Ok(if outcome.failed {
        verdict.die(b"refusing to create malformed object")
    } else {
        verdict
    })
}

#[test]
fn hash_object_verdicts_are_those_of_git() -> TestResult {
    let Some(dir) = fixture()? else {
        return Ok(());
    };
    let dir = dir.join("hash-object");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        let kind = match path.extension().and_then(|extension| extension.to_str()) {
            Some("commit") => Kind::Commit,
            Some("tag") => Kind::Tag,
            _ => continue,
        };
        files.push((path, kind));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(files.len() > 50, "there are many commits and tags, not {}", files.len());
    let mut mismatches = Vec::new();
    for (path, kind) in &files {
        let expected = Verdict {
            stdout: BString::default(),
            stderr: std::fs::read(path.with_extension("err"))?.into(),
            exit: std::fs::read_to_string(path.with_extension("exit"))?.trim().parse()?,
        };
        let actual = hash_object(*kind, &std::fs::read(path)?)?;
        if actual != expected {
            mismatches.push(format!(
                "{}:\n  Git: {expected:?}\n  gix: {actual:?}",
                path.file_name().unwrap_or_default().display()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} objects with other verdicts than Git's:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    Ok(())
}

#[test]
fn checks_with_the_default_severity_fatal_can_only_be_set_to_errors() -> TestResult {
    let mut options = Options::default();
    let err = options
        .set_severity(MessageId::NulInHeader, Severity::Ignore)
        .expect_err("a fatal check can't be ignored");
    assert_eq!(err.to_string(), "Cannot demote nulInHeader to ignore");
    assert_eq!(
        options.severity(MessageId::NulInHeader),
        Severity::Fatal,
        "a refused setting changes nothing"
    );
    options.set_severity(MessageId::NulInHeader, Severity::Error)?;
    assert_eq!(options.severity(MessageId::NulInHeader), Severity::Error);
    Ok(())
}

#[test]
fn blobs_named_by_trees_stay_queued_until_they_are_checked() -> TestResult {
    let object_hash = gix_testtools::object_hash();
    let fake_id = |digit: u8| ObjectId::from_hex(&vec![digit; object_hash.len_in_hex()]);
    let (tree, gitmodules, gitattributes) = (fake_id(b'1')?, fake_id(b'2')?, fake_id(b'3')?);
    let mut data = b"100644 .gitattributes\0".to_vec();
    data.extend_from_slice(gitattributes.as_slice());
    data.extend_from_slice(b"100644 .gitmodules\0");
    data.extend_from_slice(gitmodules.as_slice());

    let mut checker = Checker::new(Options::default(), object_hash);
    assert_eq!(checker.check(&tree, Kind::Tree, &data), Outcome::default());
    assert!(checker.has_queued_checks());
    assert_eq!(
        checker.check(&gitmodules, Kind::Blob, b"[submodule \"a\"]\n\tpath = a\n"),
        Outcome::default()
    );
    assert!(checker.has_queued_checks(), "the `.gitattributes` blob is still queued");

    let outcome = checker.finish(&Pack(HashMap::new()))?;
    assert_eq!(
        outcome
            .reports
            .iter()
            .map(|report| (report.id, report.object))
            .collect::<Vec<_>>(),
        [(MessageId::GitattributesMissing, gitattributes)],
        "only the blob that wasn't checked is read, and it's missing"
    );
    assert!(outcome.failed);
    assert!(!checker.has_queued_checks(), "finish() empties the queues");
    Ok(())
}
