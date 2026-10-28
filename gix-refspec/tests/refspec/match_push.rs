use std::collections::{BTreeSet, HashMap};

use bstr::{BStr, BString, ByteSlice};
use gix_hash::ObjectId;
use gix_refspec::{
    MatchGroup,
    match_group::{
        Item,
        match_push::{Issue, Resolve, ResolvedReference, Source},
    },
    parse::Operation,
};

#[test]
fn agrees_with_git_push() -> crate::Result {
    let dir = gix_testtools::scripted_fixture_read_only("match_push_baseline.sh")?;
    let remote = parse_remote_refs(&std::fs::read(dir.join("remote-refs.list"))?)?;
    let remote_items: Vec<_> = remote.iter().map(|(name, id)| item(name, id)).collect();

    let mut cases = 0;
    for repository in ["base", "detached"] {
        let local = Local::at(&dir.join(repository))?;
        let local_items: Vec<_> = local.refs.iter().map(|r| item(&r.name, &r.id)).collect();
        for (specs, expected) in parse_baseline(&std::fs::read(dir.join(repository).join("baseline.git"))?) {
            let parsed = specs
                .iter()
                .map(|spec| gix_refspec::parse(spec.as_bstr(), Operation::Push))
                .collect::<Result<Vec<_>, _>>()?;
            let actual = MatchGroup::from_push_specs(parsed.iter().copied()).match_push(
                local_items.iter().copied(),
                remote_items.iter().copied(),
                &mut local.resolver(),
            );
            match (actual, expected) {
                (Ok(outcome), Expected::Updates { updates, forced, new }) => {
                    let actual: BTreeSet<_> = outcome
                        .mappings
                        .iter()
                        .map(|m| {
                            let source = match m.source {
                                Source::Reference { item_index } => {
                                    Some(local_items[item_index].full_ref_name.to_owned())
                                }
                                Source::Object { name, .. } => Some(name.to_owned()),
                                Source::Delete => None,
                            };
                            (source, m.destination.clone())
                        })
                        .collect();
                    assert_eq!(
                        actual, updates,
                        "{repository} {specs:?}: the same remote references are updated from the same sources"
                    );
                    for mapping in &outcome.mappings {
                        if forced.contains(&mapping.destination) {
                            assert!(
                                mapping.force,
                                "{repository} {specs:?}: {} is updated by force as Git does",
                                mapping.destination
                            );
                        }
                        if mapping.source != Source::Delete {
                            assert_eq!(
                                mapping.remote_index.is_none(),
                                new.contains(&mapping.destination),
                                "{repository} {specs:?}: only {} references the remote lacks are created",
                                mapping.destination
                            );
                        }
                        if let Some(index) = mapping.remote_index {
                            assert_eq!(
                                remote_items[index].full_ref_name, mapping.destination,
                                "{repository} {specs:?}: the remote index refers to the destination"
                            );
                        }
                    }
                }
                (Err(err), Expected::Refused(messages)) => {
                    let actual: Vec<_> = err.issues.iter().map(issue_message).collect();
                    assert_eq!(
                        actual, messages,
                        "{repository} {specs:?}: the push is refused for the same reasons"
                    );
                    use gix_error::ErrorExt;
                    assert!(err.raise().is_validation(), "refused refspecs are invalid input");
                }
                (actual, expected) => {
                    panic!("{repository} {specs:?}: expected {expected:?}, got {actual:?}")
                }
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 56, "all recorded cases were compared");
    Ok(())
}

#[test]
fn an_empty_group_matches_branches_the_remote_has() -> crate::Result {
    let dir = gix_testtools::scripted_fixture_read_only("match_push_baseline.sh")?;
    let remote = parse_remote_refs(&std::fs::read(dir.join("remote-refs.list"))?)?;
    let remote_items: Vec<_> = remote.iter().map(|(name, id)| item(name, id)).collect();
    let local = Local::at(&dir.join("base"))?;
    let local_items: Vec<_> = local.refs.iter().map(|r| item(&r.name, &r.id)).collect();

    let matching = gix_refspec::parse(":".into(), Operation::Push)?;
    let expected = MatchGroup::from_push_specs(Some(matching)).match_push(
        local_items.iter().copied(),
        remote_items.iter().copied(),
        &mut local.resolver(),
    )?;
    let actual = MatchGroup::from_push_specs(None).match_push(
        local_items.iter().copied(),
        remote_items.iter().copied(),
        &mut local.resolver(),
    )?;
    assert_eq!(
        actual
            .mappings
            .iter()
            .map(|m| (m.source, &m.destination))
            .collect::<Vec<_>>(),
        expected
            .mappings
            .iter()
            .map(|m| (m.source, &m.destination))
            .collect::<Vec<_>>(),
        "without refspecs, the branches the remote has are updated, as with `:`"
    );
    assert!(
        actual.mappings.iter().all(|m| m.spec_index.is_none()),
        "the implied `:` isn't part of the group"
    );
    Ok(())
}

fn item<'a>(name: &'a BString, id: &'a ObjectId) -> Item<'a> {
    Item {
        full_ref_name: name.as_bstr(),
        target: id,
        object: None,
    }
}

/// A reference of a pushing repository as `git for-each-ref` lists it, with the target of symbolic references.
struct LocalRef {
    name: BString,
    id: ObjectId,
    symref: Option<BString>,
}

/// What the fixture recorded about a pushing repository.
struct Local {
    refs: Vec<LocalRef>,
    head_symref: Option<BString>,
    revisions: HashMap<BString, ObjectId>,
}

impl Local {
    fn at(dir: &std::path::Path) -> crate::Result<Self> {
        let refs = std::fs::read(dir.join("local-refs.list"))?
            .lines()
            .map(|line| -> crate::Result<LocalRef> {
                let mut tokens = line.splitn(3, |b| *b == b' ');
                let id = ObjectId::from_hex(tokens.next().expect("object id"))?;
                let name = tokens.next().expect("reference name").into();
                let symref = tokens.next().filter(|target| !target.is_empty()).map(Into::into);
                Ok(LocalRef { name, id, symref })
            })
            .collect::<crate::Result<_>>()?;
        let head_symref = std::fs::read(dir.join("head-symref"))?.trim().to_owned();
        let revisions = std::fs::read(dir.join("revisions.list"))?
            .lines()
            .map(|line| -> crate::Result<(BString, ObjectId)> {
                let (id, revision) = line.split_once_str(" ").expect("object id and revision");
                Ok((revision.into(), ObjectId::from_hex(id)?))
            })
            .collect::<crate::Result<_>>()?;
        Ok(Local {
            refs,
            head_symref: (!head_symref.is_empty()).then(|| head_symref.into()),
            revisions,
        })
    }

    fn resolver(&self) -> Recorded<'_> {
        Recorded {
            refs: &self.refs,
            head_symref: self.head_symref.as_ref().map(|name| name.as_bstr()),
            revisions: &self.revisions,
        }
    }
}

/// Answers what the matching asks about a pushing repository from what the fixture recorded about it.
struct Recorded<'a> {
    refs: &'a [LocalRef],
    head_symref: Option<&'a BStr>,
    revisions: &'a HashMap<BString, ObjectId>,
}

impl Resolve for Recorded<'_> {
    fn object(&mut self, spec: &BStr) -> Option<ObjectId> {
        self.revisions.get(spec).copied()
    }

    fn reference(&mut self, name: &BStr) -> Option<ResolvedReference> {
        let symref = if name == "HEAD" {
            self.head_symref
        } else {
            self.refs
                .iter()
                .find(|r| r.name == name)?
                .symref
                .as_ref()
                .map(|target| target.as_bstr())
        };
        Some(match symref {
            Some(target) => ResolvedReference {
                name: target.to_owned(),
                is_symbolic: true,
            },
            None => ResolvedReference {
                name: name.to_owned(),
                is_symbolic: false,
            },
        })
    }
}

fn parse_remote_refs(buf: &[u8]) -> crate::Result<Vec<(BString, ObjectId)>> {
    let mut out = Vec::new();
    for line in buf.lines() {
        let (id, name) = line.split_once_str("\t").expect("object id and name");
        // `git push` only knows the references below `refs/` and doesn't peel them.
        if name.starts_with(b"refs/") && !name.ends_with(b"^{}") {
            out.push((name.into(), ObjectId::from_hex(id)?));
        }
    }
    Ok(out)
}

/// What Git did with the refspecs of one case.
#[derive(Debug)]
enum Expected {
    Updates {
        /// The source and destination of each updated reference, without source for deletions.
        updates: BTreeSet<(Option<BString>, BString)>,
        /// The destinations of forced updates that aren't fast-forwards.
        forced: BTreeSet<BString>,
        /// The destinations the remote doesn't have yet.
        new: BTreeSet<BString>,
    },
    /// The messages of the errors Git refused the push with, in order.
    Refused(Vec<String>),
}

/// Parse the output of `git push --dry-run --porcelain` for each case, ended by the refspecs of the case.
fn parse_baseline(buf: &[u8]) -> Vec<(Vec<BString>, Expected)> {
    let mut cases = Vec::new();
    let mut updates = BTreeSet::new();
    let mut forced = BTreeSet::new();
    let mut new = BTreeSet::new();
    let mut errors = Vec::<String>::new();
    let mut lines = buf.lines();
    while let Some(line) = lines.next() {
        if let Some(specs) = line.strip_prefix(b"specs: ") {
            let expected = if errors.is_empty() {
                Expected::Updates {
                    updates: std::mem::take(&mut updates),
                    forced: std::mem::take(&mut forced),
                    new: std::mem::take(&mut new),
                }
            } else {
                Expected::Refused(std::mem::take(&mut errors))
            };
            cases.push((specs.split_str(" ").map(BString::from).collect(), expected));
        } else if let Some(message) = line.strip_prefix(b"error: ").or_else(|| line.strip_prefix(b"fatal: ")) {
            let message = message.to_str_lossy();
            // Git reports a failed push afterwards, and `git receive-pack` the connection Git dropped when refusing it.
            if message.starts_with("failed to push some refs") || message == "the remote end hung up unexpectedly" {
                continue;
            }
            if message.starts_with("The destination you provided is not a full refname") {
                // The message continues on the following lines, which name the destination and the source.
                let (mut destination, mut source) = (None, None);
                while destination.is_none() || source.is_none() {
                    let line = lines.next().expect("Git names the destination and the source");
                    destination = destination.or_else(|| quoted(line, "matches '", "'"));
                    source = source.or_else(|| quoted(line, "being pushed ('", "')"));
                }
                errors.push(unqualified_destination_message(
                    destination.expect("Git names the destination").as_bstr(),
                    source.expect("Git names the source").as_bstr(),
                ));
            } else {
                errors.push(message.into_owned());
            }
        } else if let [flag, source_and_destination, summary] = line.split_str("\t").collect::<Vec<_>>()[..] {
            // Reference names can't contain colons, unlike revisions used as sources.
            let (source, destination) = source_and_destination
                .rsplit_once_str(":")
                .expect("source and destination");
            let destination = BString::from(destination);
            if flag == b"+" {
                forced.insert(destination.clone());
            }
            if summary.starts_with(b"[new ") {
                new.insert(destination.clone());
            }
            updates.insert(((!source.is_empty()).then(|| source.into()), destination));
        }
        // Other lines, like `To ../remote.git` and `Done`, carry no information about the matching.
    }
    cases
}

fn quoted(line: &[u8], start: &str, end: &str) -> Option<BString> {
    let (_, rest) = line.split_once_str(start)?;
    let (quoted, _) = rest.split_once_str(end)?;
    Some(quoted.into())
}

/// Git's message for an unqualified destination spans several lines, so it's compared by the names it contains.
fn unqualified_destination_message(destination: &BStr, source: &BStr) -> String {
    format!("unqualified destination '{destination}' for source '{source}'")
}

fn issue_message(issue: &Issue) -> String {
    match issue {
        Issue::UnqualifiedDestination { destination, source } => {
            unqualified_destination_message(destination.as_bstr(), source.as_bstr())
        }
        other => other.to_string(),
    }
}
