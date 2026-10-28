//! Matching push refspecs against local and remote references, as `match_push_refs()` in Git's `remote.c` does.
use std::ops::Range;

use bstr::{BStr, BString, ByteSlice, ByteVec};
use gix_hash::ObjectId;

use crate::{MatchGroup, RefSpecRef, match_group::Item, types::Mode};

/// What push matching needs to know about the local repository beyond the references it lists.
pub trait Resolve {
    /// Return the object that `spec` names, as `git rev-parse` would, or `None` if it names none.
    ///
    /// This is asked for the sources of refspecs that match no local reference, like `HEAD`, `main~1` or an object id.
    fn object(&mut self, spec: &BStr) -> Option<ObjectId>;

    /// Return the reference that `name` ultimately points to after following symbolic references,
    /// or `None` if `name` isn't a reference.
    ///
    /// A reference that isn't symbolic resolves to itself.
    fn reference(&mut self, name: &BStr) -> Option<ResolvedReference>;
}

/// A reference after following symbolic references, as returned by [`Resolve::reference()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReference {
    /// The full name of the reference that isn't symbolic, like `refs/heads/main`.
    pub name: BString,
    /// `true` if the name that was resolved is a symbolic reference, as `HEAD` usually is.
    pub is_symbolic: bool,
}

/// Where the object that a remote reference is updated with comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'spec> {
    /// A local reference.
    Reference {
        /// The index of the reference in the local items passed to [`MatchGroup::match_push()`].
        item_index: usize,
    },
    /// The object that the source of a refspec names without matching a local reference, like `HEAD` or `main~1`.
    Object {
        /// The source as the refspec spells it.
        name: &'spec BStr,
        /// The object it names.
        id: ObjectId,
    },
    /// Nothing, as the refspec deletes the remote reference, like `:refs/heads/main` does.
    Delete,
}

/// A remote reference to update and the source to update it with, as found by [`MatchGroup::match_push()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping<'spec> {
    /// What to update the remote reference with.
    pub source: Source<'spec>,
    /// The full name of the remote reference to update.
    pub destination: BString,
    /// The index of the reference in the remote items passed to [`MatchGroup::match_push()`],
    /// or `None` if the remote doesn't have it and the push creates it.
    pub remote_index: Option<usize>,
    /// `true` if the refspec starts with `+`, allowing an update that isn't a fast-forward.
    pub force: bool,
    /// The index of the refspec in the match group, or `None` for the `:` that an empty group matches with.
    pub spec_index: Option<usize>,
}

/// The outcome of [`MatchGroup::match_push()`].
#[derive(Debug, Clone)]
pub struct Outcome<'spec> {
    /// The match group that produced this outcome.
    pub group: MatchGroup<'spec>,
    /// The remote references to update: those the remote has in the order of the remote items, followed by those
    /// the push creates in the order they were matched.
    pub mappings: Vec<Mapping<'spec>>,
}

/// A problem with the push refspecs that makes Git refuse the push, with the message Git prints for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// The source of a refspec matches no local reference and names no object.
    SourceNotFound {
        /// The source as the refspec spells it.
        source: BString,
    },
    /// The source of a refspec matches more than one local reference equally well.
    SourceAmbiguous {
        /// The source as the refspec spells it.
        source: BString,
    },
    /// A refspec without destination has a source that isn't a reference, or a symbolic one pointing outside `refs/heads/`.
    ///
    /// Git stops matching at this issue.
    SourceNotABranch {
        /// The name of the local reference or object the source matched.
        source: BString,
    },
    /// The destination of a refspec matches more than one remote reference equally well.
    DestinationAmbiguous {
        /// The destination as the refspec spells it.
        destination: BString,
    },
    /// A refspec deletes a remote reference that doesn't exist and whose name isn't a full reference name.
    DeleteNonExistingDestination {
        /// The destination as the refspec spells it.
        destination: BString,
    },
    /// The destination of a refspec isn't a full reference name, matches no remote reference, and its source isn't a
    /// branch or tag that the full name could be taken from.
    UnqualifiedDestination {
        /// The destination as the refspec spells it.
        destination: BString,
        /// The name of the local reference or object the source matched.
        source: BString,
    },
    /// More than one refspec updates the same remote reference.
    DestinationReceivesFromMultipleSources {
        /// The full name of the remote reference.
        destination: BString,
    },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::SourceNotFound { source } => write!(f, "src refspec {source} does not match any"),
            Issue::SourceAmbiguous { source } => write!(f, "src refspec {source} matches more than one"),
            Issue::SourceNotABranch { source } => write!(f, "{source} cannot be resolved to branch"),
            Issue::DestinationAmbiguous { destination } => write!(f, "dst refspec {destination} matches more than one"),
            Issue::DeleteNonExistingDestination { destination } => {
                write!(f, "unable to delete '{destination}': remote ref does not exist")
            }
            Issue::UnqualifiedDestination { destination, source } => write!(
                f,
                "The destination you provided is not a full refname (i.e., starting with \"refs/\"): no ref matches \
                 '{destination}' on the remote side, and the <src> being pushed ('{source}') is not a ref in \
                 \"refs/{{heads,tags}}/\" whose prefix could be added. You must fully qualify the ref."
            ),
            Issue::DestinationReceivesFromMultipleSources { destination } => {
                write!(f, "dst ref {destination} receives from more than one src")
            }
        }
    }
}

/// The error returned by [`MatchGroup::match_push()`].
///
/// Its source is a classification-only [`gix_error::ClassificationMarker`].
/// Use [`gix_error::classify()`] or `is_validation()` on [`gix_error::Exn`] and [`gix_error::Error`] to check the
/// classification, without depending on the concrete diagnostic type. Downcast to this type for all issues.
#[derive(Debug)]
pub struct Error {
    /// All issues found, in the order Git reports them.
    pub issues: Vec<Issue>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Found {} {} the push: \n\t{}",
            self.issues.len(),
            if self.issues.len() == 1 {
                "issue that prevents"
            } else {
                "issues that prevent"
            },
            self.issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n\t")
        )
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(const { &gix_error::ClassificationMarker::VALIDATION })
    }
}

/// Matching
impl<'spec> MatchGroup<'spec> {
    /// Match the push refspecs of this group against `local` references to find the `remote` references to update,
    /// and what to update them with, as `match_push_refs()` in Git's `remote.c` does.
    ///
    /// `local` are the references of the local repository below `refs/`, as Git's `get_local_heads()` lists them for a push:
    /// without `HEAD`, and with symbolic references pointing to the object of their target. `remote` are the references
    /// the remote advertised for the push, those below `refs/`. `resolve` answers what the matching needs to know
    /// about the local repository beyond its references.
    ///
    /// * A refspec with a source and a destination, like `main:refs/heads/main`, updates the destination with the
    ///   source. The source is the local reference it matches, or the object it names if it matches none, like `HEAD`.
    ///   The destination is the remote reference it matches, or a new one if it's a full reference name.
    ///   If it's not, but the source is a branch or tag, it's created below `refs/heads/` or `refs/tags/` respectively.
    ///   References match as `git rev-parse` would expand their names, and matches outside `refs/heads/` and
    ///   `refs/tags/` that aren't spelled out (almost) fully only count if nothing else matches.
    /// * A refspec without destination, like `main`, uses the reference its source resolves to as destination.
    /// * `+` allows updates that aren't fast-forwards.
    /// * A refspec without source, like `:refs/heads/main`, deletes the destination.
    /// * `:` updates all branches that the remote has under the same name, and is what an empty group matches with.
    /// * Patterns like `refs/heads/*:refs/heads/other/*` update the references their destination names with all local
    ///   references their source matches.
    /// * Negative refspecs like `^refs/heads/main` exclude remote references whose names they match. Note that as
    ///   in Git, they are matched against the destinations of the push, not its sources.
    ///
    /// Refspecs with a source and a destination are matched first, in order, then patterns and `:` match the remaining
    /// local references.
    /// Errors are reported as Git does: first all sources that match nothing or more than one local reference, and
    /// if there are none, all problems of the destinations.
    pub fn match_push<'item>(
        self,
        local: impl IntoIterator<Item = Item<'item>>,
        remote: impl IntoIterator<Item = Item<'item>>,
        resolve: &mut dyn Resolve,
    ) -> Result<Outcome<'spec>, Error> {
        let local: Vec<_> = local.into_iter().collect();
        let mut destinations: Vec<Destination<'spec>> = remote
            .into_iter()
            .enumerate()
            .map(|(remote_index, item)| Destination {
                name: item.full_ref_name.to_owned(),
                remote_index: Some(remote_index),
                source: None,
                force: false,
                spec_index: None,
            })
            .collect();
        let specs = classify(&self.specs);

        // Like `check_push_refs()`, check all explicit sources before looking at any destination.
        let mut issues = Vec::new();
        let sources: Vec<Option<Source<'spec>>> = specs
            .iter()
            .map(|spec| match spec.kind {
                Kind::Explicit { src, .. } => match explicit_source(src, &local, resolve) {
                    Ok(source) => Some(source),
                    Err(issue) => {
                        issues.push(issue);
                        None
                    }
                },
                Kind::Pattern { .. } | Kind::Matching | Kind::Negative { .. } => None,
            })
            .collect();
        if !issues.is_empty() {
            return Err(Error { issues });
        }

        // `match_explicit_refs()`
        for (spec, source) in specs.iter().zip(sources) {
            let (Kind::Explicit { dst, .. }, Some(source)) = (spec.kind, source) else {
                continue;
            };
            let source_name = source_name(source, &local);
            let dst_value: BString = match dst {
                Some(dst) => dst.to_owned(),
                None => match resolve.reference(source_name) {
                    Some(resolved) if !resolved.is_symbolic || resolved.name.starts_with(b"refs/heads/") => {
                        resolved.name
                    }
                    _ => {
                        issues.push(Issue::SourceNotABranch {
                            source: source_name.to_owned(),
                        });
                        return Err(Error { issues });
                    }
                },
            };
            let destination_index =
                match count_refspec_match(dst_value.as_bstr(), destinations.iter().map(|d| d.name.as_bstr())) {
                    (1, Some(index)) => Some(index),
                    (0, _) => {
                        if dst_value.starts_with(b"refs/") {
                            Some(add_destination(&mut destinations, dst_value))
                        } else if source_is_null(source, &local) {
                            issues.push(Issue::DeleteNonExistingDestination { destination: dst_value });
                            None
                        } else if let Some(guess) = guess_ref(dst_value.as_bstr(), source_name, resolve) {
                            Some(add_destination(&mut destinations, guess))
                        } else {
                            issues.push(Issue::UnqualifiedDestination {
                                destination: dst_value,
                                source: source_name.to_owned(),
                            });
                            None
                        }
                    }
                    _ => {
                        issues.push(Issue::DestinationAmbiguous { destination: dst_value });
                        None
                    }
                };
            let Some(index) = destination_index else {
                continue;
            };
            let destination = &mut destinations[index];
            if destination.source.is_some() {
                issues.push(Issue::DestinationReceivesFromMultipleSources {
                    destination: destination.name.clone(),
                });
                continue;
            }
            destination.source = Some(source);
            destination.force = spec.force;
            destination.spec_index = spec.index;
        }
        if !issues.is_empty() {
            return Err(Error { issues });
        }

        // The remaining local references, matched by patterns and `:`.
        for (item_index, item) in local.iter().enumerate() {
            let Some((name, spec)) = pattern_match(&specs, item.full_ref_name) else {
                continue;
            };
            let index = match destinations.iter().position(|d| d.name == name) {
                Some(index) => {
                    if destinations[index].source.is_some() {
                        continue;
                    }
                    index
                }
                // `:` only updates what the remote has.
                None if matches!(spec.kind, Kind::Matching) => continue,
                None => add_destination(&mut destinations, name),
            };
            let destination = &mut destinations[index];
            destination.source = Some(Source::Reference { item_index });
            destination.force = spec.force;
            destination.spec_index = spec.index;
        }

        // `apply_negative_refspecs()`
        let negative_sources: Vec<_> = specs
            .iter()
            .filter_map(|spec| match spec.kind {
                Kind::Negative { src } => Some(src),
                Kind::Explicit { .. } | Kind::Pattern { .. } | Kind::Matching => None,
            })
            .collect();
        let mappings = destinations
            .into_iter()
            .filter(|d| {
                !negative_sources
                    .iter()
                    .any(|src| refspec_source_matches(src, d.name.as_bstr()))
            })
            .filter_map(|d| {
                Some(Mapping {
                    source: d.source?,
                    destination: d.name,
                    remote_index: d.remote_index,
                    force: d.force,
                    spec_index: d.spec_index,
                })
            })
            .collect();
        Ok(Outcome { group: self, mappings })
    }
}

/// A remote reference, either advertised or to be created, and what the push updates it with.
struct Destination<'spec> {
    name: BString,
    remote_index: Option<usize>,
    source: Option<Source<'spec>>,
    force: bool,
    spec_index: Option<usize>,
}

fn add_destination(destinations: &mut Vec<Destination<'_>>, name: BString) -> usize {
    destinations.push(Destination {
        name,
        remote_index: None,
        source: None,
        force: false,
        spec_index: None,
    });
    destinations.len() - 1
}

/// How a push refspec takes part in matching.
#[derive(Clone, Copy)]
enum Kind<'a> {
    /// A single source and destination, where an empty source deletes the destination and a missing destination
    /// is derived from the source.
    Explicit { src: &'a BStr, dst: Option<&'a BStr> },
    /// A source pattern and the destination pattern it maps to, which is the source pattern for one-sided refspecs.
    Pattern { src: &'a BStr, dst: &'a BStr },
    /// `:` for all branches the remote has under the same name.
    Matching,
    /// Excludes destinations matching `src`.
    Negative { src: &'a BStr },
}

struct Spec<'a> {
    kind: Kind<'a>,
    force: bool,
    index: Option<usize>,
}

fn classify<'a>(specs: &[RefSpecRef<'a>]) -> Vec<Spec<'a>> {
    if specs.is_empty() {
        return vec![Spec {
            kind: Kind::Matching,
            force: false,
            index: None,
        }];
    }
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let kind = match (spec.mode, spec.src, spec.dst) {
                (Mode::Negative, src, _) => Kind::Negative {
                    src: src.unwrap_or_default(),
                },
                (_, None, None) => Kind::Matching,
                (_, None, Some(dst)) => Kind::Explicit {
                    src: Default::default(),
                    dst: Some(dst),
                },
                (_, Some(src), dst) if src.contains(&b'*') => Kind::Pattern {
                    src,
                    dst: dst.unwrap_or(src),
                },
                (_, Some(src), dst) => Kind::Explicit { src, dst },
            };
            Spec {
                kind,
                force: spec.mode == Mode::Force,
                index: Some(index),
            }
        })
        .collect()
}

/// `match_explicit_lhs()`: the local reference `src` matches, the object it names, or a deletion if it's empty.
fn explicit_source<'spec>(
    src: &'spec BStr,
    local: &[Item<'_>],
    resolve: &mut dyn Resolve,
) -> Result<Source<'spec>, Issue> {
    match count_refspec_match(src, local.iter().map(|item| item.full_ref_name)) {
        (1, Some(item_index)) => Ok(Source::Reference { item_index }),
        (0, _) if src.is_empty() => Ok(Source::Delete),
        (0, _) => resolve
            .object(src)
            .map(|id| Source::Object { name: src, id })
            .ok_or_else(|| Issue::SourceNotFound { source: src.to_owned() }),
        _ => Err(Issue::SourceAmbiguous { source: src.to_owned() }),
    }
}

/// The name Git gives the reference standing for `source`.
fn source_name<'a>(source: Source<'a>, local: &[Item<'a>]) -> &'a BStr {
    match source {
        Source::Reference { item_index } => local[item_index].full_ref_name,
        Source::Object { name, .. } => name,
        Source::Delete => "(delete)".into(),
    }
}

fn source_is_null(source: Source<'_>, local: &[Item<'_>]) -> bool {
    match source {
        Source::Reference { item_index } => local[item_index].target.is_null(),
        Source::Object { id, .. } => id.is_null(),
        Source::Delete => true,
    }
}

/// `guess_ref()`: put `name` below `refs/heads/` or `refs/tags/` if the source resolves to a branch or tag.
fn guess_ref(name: &BStr, source_name: &BStr, resolve: &mut dyn Resolve) -> Option<BString> {
    let resolved = resolve.reference(source_name)?;
    let mut guess: BString = if resolved.name.starts_with(b"refs/heads/") {
        "refs/heads/".into()
    } else if resolved.name.starts_with(b"refs/tags/") {
        "refs/tags/".into()
    } else {
        return None;
    };
    guess.push_str(name);
    Some(guess)
}

/// `count_refspec_match()`: return how many of `names` `pattern` matches, and the index of the last of them.
///
/// Matches outside `refs/heads/` and `refs/tags/` that `pattern` doesn't spell out (almost) fully are weak, and only
/// count if there are no others.
fn count_refspec_match<'a>(pattern: &BStr, names: impl Iterator<Item = &'a BStr>) -> (usize, Option<usize>) {
    let (mut strong, mut strong_index) = (0, None);
    let (mut weak, mut weak_index) = (0, None);
    for (index, name) in names.enumerate() {
        if !refname_match(pattern, name) {
            continue;
        }
        if name.len() != pattern.len()
            && pattern.len() + 5 != name.len()
            && !name.starts_with(b"refs/heads/")
            && !name.starts_with(b"refs/tags/")
        {
            weak += 1;
            weak_index = Some(index);
        } else {
            strong += 1;
            strong_index = Some(index);
        }
    }
    if strong_index.is_some() {
        (strong, strong_index)
    } else {
        (weak, weak_index)
    }
}

/// `refname_match()`: `true` if `full_name` is one of the names `abbreviated` expands to, as `git rev-parse` expands them.
fn refname_match(abbreviated: &BStr, full_name: &BStr) -> bool {
    crate::spec::expand_partial_name(abbreviated, |expanded| (expanded == full_name).then_some(())).is_some()
}

/// `get_ref_match()`: the destination that a pattern or `:` maps the local reference `name` to, with its refspec.
fn pattern_match<'s, 'a>(specs: &'s [Spec<'a>], name: &BStr) -> Option<(BString, &'s Spec<'a>)> {
    let mut matching = None;
    for spec in specs {
        match spec.kind {
            Kind::Matching => {
                if matching.is_none() || spec.force {
                    matching = Some(spec);
                }
            }
            Kind::Pattern { src, dst } => {
                if let Some(destination) = glob_capture(src, name).and_then(|capture| replace_glob(dst, name, capture))
                {
                    return Some((destination, spec));
                }
            }
            Kind::Explicit { .. } | Kind::Negative { .. } => {}
        }
    }
    let spec = matching?;
    // `:` only matches branches.
    name.starts_with(b"refs/heads/").then(|| (name.to_owned(), spec))
}

/// `refspec_match()` for negative refspecs: `src` is a pattern or a name to compare exactly.
fn refspec_source_matches(src: &BStr, name: &BStr) -> bool {
    if src.contains(&b'*') {
        glob_capture(src, name).is_some()
    } else {
        src == name
    }
}

/// The portion of `name` matched by the `*` of `pattern`, if `pattern` matches `name`.
fn glob_capture(pattern: &BStr, name: &BStr) -> Option<Range<usize>> {
    let star = pattern.find_byte(b'*')?;
    let (prefix, suffix) = (&pattern[..star], &pattern[star + 1..]);
    (name.starts_with(prefix) && name.len() >= star + suffix.len() && name.ends_with(suffix))
        .then(|| star..name.len() - suffix.len())
}

/// Substitute the `*` of `replacement` with the `capture` of `name`.
fn replace_glob(replacement: &BStr, name: &BStr, capture: Range<usize>) -> Option<BString> {
    let star = replacement.find_byte(b'*')?;
    let mut out = BString::from(&replacement[..star]);
    out.push_str(&name[capture]);
    out.push_str(&replacement[star + 1..]);
    Some(out)
}
