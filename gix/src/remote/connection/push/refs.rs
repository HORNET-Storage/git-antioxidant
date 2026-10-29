//! Decide which remote references a push updates and with what, and check the updates before anything is sent, as
//! `builtin/push.c` and `remote.c` in Git do.
use std::collections::{BTreeMap, BTreeSet};

use gix_hash::ObjectId;
use gix_ref::FullName;
use gix_refspec::{
    Instruction, MatchGroup, RefSpec, instruction,
    match_group::{
        Item,
        match_push::{Issue, Resolve, ResolvedReference, Source as Matched},
    },
    parse::Operation,
};

use super::{Expected, Lease, Options, Rejection, Source, Status, Update};
use crate::{
    Remote, Repository, Result,
    bstr::{BStr, BString, ByteSlice, ByteVec},
    config::{cache::util::ApplyLeniency, tree::Push},
    error::{ResultExt, bail, validation},
    push, remote,
    revision::walk::{CommitTimeOrder, Sorting},
};

/// A local reference as Git's `get_local_heads()` lists it for a push, with the object it points to.
pub(super) struct LocalRef {
    pub(super) name: FullName,
    pub(super) id: ObjectId,
}

/// The references of `repo` below `refs/`, sorted by name, with symbolic references pointing to the object of their
/// target, leaving out those that don't point to an available object, as `get_local_heads()` in Git's `remote.c`
/// lists them.
pub(super) fn local_refs(repo: &Repository) -> Result<Vec<LocalRef>> {
    let platform = repo.references()?;
    let mut out = Vec::new();
    for reference in platform.all()? {
        // Git leaves out the references it can't read or resolve.
        let Ok(mut reference) = reference else {
            continue;
        };
        if !reference.name().as_bstr().starts_with(b"refs/") {
            continue;
        }
        let name = reference.name().to_owned();
        let Ok(id) = reference.follow_to_object() else {
            continue;
        };
        let id = id.detach();
        if repo.has_object(id) {
            out.push(LocalRef { name, id });
        }
    }
    Ok(out)
}

/// The refspecs to push to `remote` with `options`, as `git push` decides them: `refs/tags/*` for `--tags`, then the
/// given refspecs, mapped as `set_refspecs()` in Git's `builtin/push.c` maps them, else the push refspecs of `remote`,
/// else the one that `push.default` implies, as `setup_default_push_refspecs()` sets it up.
pub(super) fn refspecs(
    repo: &Repository,
    remote: &Remote<'_>,
    options: &Options,
    local: &[LocalRef],
) -> Result<Vec<RefSpec>> {
    if options.delete && options.tags {
        bail!(validation("options '--delete' and '--tags' cannot be used together"));
    }
    if options.delete && options.refspecs.is_empty() {
        bail!(validation("--delete doesn't make sense without any refs"));
    }
    let push_default = push_default(repo)?;
    let mut specs = Vec::<BString>::new();
    if options.tags {
        specs.push("refs/tags/*".into());
    }
    let mut given = options.refspecs.iter();
    while let Some(spec) = given.next() {
        if spec == "tag" {
            let Some(tag) = given.next() else {
                bail!(validation("tag shorthand without <tag>"));
            };
            let mut spec = BString::from(if options.delete { ":refs/tags/" } else { "refs/tags/" });
            spec.push_str(tag);
            specs.push(spec);
        } else if options.delete {
            if spec.contains(&b':') || spec.is_empty() {
                bail!(validation("--delete only accepts plain target ref names"));
            }
            let mut deletion = BString::from(":");
            deletion.push_str(spec);
            specs.push(deletion);
        } else if spec.contains(&b':') {
            specs.push(spec.clone());
        } else {
            match unique_match(spec.as_bstr(), local) {
                Some(matched) => specs.push(mapped(repo, remote, push_default, spec.as_bstr(), matched)),
                None => specs.push(spec.clone()),
            }
        }
    }
    if specs.is_empty() {
        let configured = remote.refspecs(remote::Direction::Push);
        if !configured.is_empty() {
            return Ok(configured.to_vec());
        }
        specs.push(default_refspec(repo, remote, push_default)?);
    }
    specs
        .iter()
        .map(|spec| {
            gix_refspec::parse(spec.as_bstr(), Operation::Push)
                .map(|spec| spec.to_owned())
                .or_raise(|| validation(format!("invalid refspec '{spec}'")))
        })
        .collect()
}

/// The value of `push.default`, or `None` if it isn't set.
fn push_default(repo: &Repository) -> Result<Option<push::Default>> {
    repo.config
        .resolved
        .string(Push::DEFAULT)
        .map(|value| Push::DEFAULT.try_into_default(value))
        .transpose()
        .with_leniency(repo.config.lenient_config)
}

/// The local reference `spec` names if it names only one, as `count_refspec_match()` in Git's `remote.c` finds it:
/// matches outside `refs/heads/` and `refs/tags/` that `spec` doesn't spell out (almost) fully only count if there
/// are no others.
fn unique_match<'a>(spec: &BStr, local: &'a [LocalRef]) -> Option<&'a LocalRef> {
    let (mut strong, mut weak) = (Vec::new(), Vec::new());
    for reference in local {
        let name = reference.name.as_bstr();
        if !refname_match(spec, name) {
            continue;
        }
        if name.len() != spec.len()
            && spec.len() + 5 != name.len()
            && !name.starts_with(b"refs/heads/")
            && !name.starts_with(b"refs/tags/")
        {
            weak.push(reference);
        } else {
            strong.push(reference);
        }
    }
    match (strong.as_slice(), weak.as_slice()) {
        ([one], _) | ([], [one]) => Some(*one),
        _ => None,
    }
}

/// Return `true` if `full_name` is one of the names `abbreviated` expands to with the rules of `git rev-parse`, as
/// `refname_match()` in Git's `refs.c` matches them.
pub(super) fn refname_match(abbreviated: &BStr, full_name: &BStr) -> bool {
    const RULES: &[(&str, &str)] = &[
        ("", ""),
        ("refs/", ""),
        ("refs/tags/", ""),
        ("refs/heads/", ""),
        ("refs/remotes/", ""),
        ("refs/remotes/", "/HEAD"),
    ];
    RULES.iter().any(|(prefix, suffix)| {
        full_name
            .strip_prefix(prefix.as_bytes())
            .and_then(|rest| rest.strip_suffix(suffix.as_bytes()))
            .is_some_and(|name| name == abbreviated.as_bytes())
    })
}

/// The refspec for `spec`, which names the local reference `matched`, as `refspec_append_mapped()` in Git's
/// `builtin/push.c` maps it: to the destination a push refspec of `remote` gives it, or with `push.default=upstream`
/// to the upstream branch of the branch it names.
fn mapped(
    repo: &Repository,
    remote: &Remote<'_>,
    push_default: Option<push::Default>,
    spec: &BStr,
    matched: &LocalRef,
) -> BString {
    let matched = matched.name.as_bstr();
    if let Some((destination, force)) = find_match(remote.refspecs(remote::Direction::Push), matched) {
        let mut mapped = BString::from(if force { "+" } else { "" });
        mapped.push_str(matched);
        mapped.push_byte(b':');
        mapped.push_str(&destination);
        return mapped;
    }
    if push_default == Some(push::Default::Upstream)
        && let Some(branch) = matched.strip_prefix(b"refs/heads/")
        && let [merge] = upstream_merge(repo, branch.as_bstr()).as_slice()
    {
        let mut mapped = spec.to_owned();
        mapped.push_byte(b':');
        mapped.push_str(merge);
        return mapped;
    }
    spec.to_owned()
}

/// The values of `branch.<name>.merge` for the local branch with the short name `branch`, or none if
/// `branch.<name>.remote` isn't set, as `set_merge()` in Git's `remote.c` reads them.
fn upstream_merge(repo: &Repository, branch: &BStr) -> Vec<BString> {
    let config = &repo.config.resolved;
    if config.string_by("branch", Some(branch), "remote").is_none() {
        return Vec::new();
    }
    config.strings_by("branch", Some(branch), "merge").unwrap_or_default()
}

/// The refspec that `push.default` implies for pushing the current branch to `remote`, as
/// `setup_default_push_refspecs()` in Git's `builtin/push.c` sets it up, failing with its messages, but without the
/// advice they end with.
fn default_refspec(repo: &Repository, remote: &Remote<'_>, push_default: Option<push::Default>) -> Result<BString> {
    let remote_name = remote_name(remote);
    match push_default {
        Some(push::Default::Matching) => return Ok(":".into()),
        Some(push::Default::Nothing) => {
            bail!(validation(
                "You didn't specify any refspecs to push, and push.default is \"nothing\"."
            ))
        }
        _ => {}
    }
    let Some(branch) = repo
        .head_name()?
        .filter(|name| name.category() == Some(gix_ref::Category::LocalBranch))
    else {
        bail!(validation(format!(
            "You are not currently on a branch.\nTo push the history leading to the current (detached HEAD)\n\
             state now, use\n\n    git push {remote_name} HEAD:<name-of-remote-branch>"
        )));
    };
    let short_name = branch.shorten();
    let same_remote = remote_name == remote_for_branch(repo, short_name);
    let mut destination = branch.as_bstr().to_owned();
    match push_default.unwrap_or_default() {
        push::Default::Upstream => {
            if !same_remote {
                bail!(validation(format!(
                    "You are pushing to remote '{remote_name}', which is not the upstream of\n\
                     your current branch '{short_name}', without telling me what to push\n\
                     to update which remote branch."
                )));
            }
            destination = upstream(repo, short_name, remote_name.as_bstr())?;
        }
        push::Default::Simple => {
            if same_remote {
                let upstream = upstream(repo, short_name, remote_name.as_bstr())?;
                if upstream.as_bstr() != branch.as_bstr() {
                    let short_upstream = upstream
                        .strip_prefix(b"refs/heads/")
                        .unwrap_or(upstream.as_bytes())
                        .as_bstr();
                    bail!(validation(format!(
                        "The upstream branch of your current branch does not match\n\
                         the name of your current branch.  To push to the upstream branch\n\
                         on the remote, use\n\n    git push {remote_name} HEAD:{short_upstream}\n\n\
                         To push to the branch of the same name on the remote, use\n\n    git push {remote_name} HEAD"
                    )));
                }
            }
        }
        push::Default::Current | push::Default::Matching | push::Default::Nothing => {}
    }
    let mut spec = branch.as_bstr().to_owned();
    spec.push_byte(b':');
    spec.push_str(&destination);
    Ok(spec)
}

/// The name Git gives `remote`: its name, or the URL it pushes to if it has none.
fn remote_name(remote: &Remote<'_>) -> BString {
    match remote.name() {
        Some(name) => name.as_bstr().to_owned(),
        None => remote
            .url(remote::Direction::Push)
            .map(gix_url::Url::to_bstring)
            .unwrap_or_default(),
    }
}

/// The name of the remote of the local branch with the short name `branch`: `branch.<name>.remote`, else the only
/// remote, else `origin`, as `remotes_remote_for_branch()` in Git's `remote.c` names it.
fn remote_for_branch(repo: &Repository, branch: &BStr) -> BString {
    if let Some(name) = repo.config.resolved.string_by("branch", Some(branch), "remote") {
        return name;
    }
    let names = repo.remote_names();
    let only_remote = match names.len() {
        1 => names.into_iter().next(),
        _ => None,
    };
    only_remote.unwrap_or_else(|| "origin".into())
}

/// The upstream branch of the local branch with the short name `branch` on the remote named `remote_name`, the value
/// of `branch.<name>.merge`, failing as `get_upstream_ref()` in Git's `builtin/push.c` fails if there is none or more
/// than one.
fn upstream(repo: &Repository, branch: &BStr, remote_name: &BStr) -> Result<BString> {
    match upstream_merge(repo, branch).as_slice() {
        [] => bail!(validation(format!(
            "The current branch {branch} has no upstream branch.\n\
             To push the current branch and set the remote as upstream, use\n\n    \
             git push --set-upstream {remote_name} {branch}"
        ))),
        [merge] => Ok(merge.clone()),
        _ => bail!(validation(format!(
            "The current branch {branch} has multiple upstream branches, refusing to push."
        ))),
    }
}

/// Fail if the source of a refspec with a source and a destination matches no local reference and names no object, or
/// matches more than one local reference, as `check_push_refs()` in Git's `remote.c` checks before connecting.
pub(super) fn check_sources(repo: &Repository, specs: &[RefSpec], local: &[LocalRef]) -> Result {
    let outcome = MatchGroup::from_push_specs(specs.iter().map(RefSpec::to_ref)).match_push(
        local_items(local),
        std::iter::empty(),
        &mut Resolver { repo },
    );
    match outcome {
        Ok(_) => Ok(()),
        Err(mut err) => {
            err.issues
                .retain(|issue| matches!(issue, Issue::SourceNotFound { .. } | Issue::SourceAmbiguous { .. }));
            if err.issues.is_empty() {
                Ok(())
            } else {
                Err(crate::Error::from_error(err))
            }
        }
    }
}

/// The `local` references as items to match.
fn local_items(local: &[LocalRef]) -> impl Iterator<Item = Item<'_>> {
    local.iter().map(|reference| Item {
        full_ref_name: reference.name.as_bstr(),
        target: &reference.id,
        object: None,
    })
}

/// The references of the remote below `refs/` with valid names, along with the objects they point to, as Git's
/// `get_remote_heads()` keeps them for a push.
fn advertised_refs(remote_refs: &gix_protocol::push::RemoteRefs) -> Vec<(BString, ObjectId)> {
    remote_refs
        .refs
        .iter()
        .filter_map(|reference| {
            let (name, Some(id), _) = reference.unpack() else {
                return None;
            };
            (name.starts_with(b"refs/") && gix_validate::reference::name(name).is_ok())
                .then(|| (name.to_owned(), id.to_owned()))
        })
        .collect()
}

/// The updates that pushing `specs` from the `local` references to the references in `remote_refs` makes, along with
/// the annotated tags to follow if `follow_tags` is set, checked as `match_push_refs()`, `apply_push_cas()` and
/// `set_ref_status_for_push()` in Git's `remote.c` check them.
pub(super) fn updates(
    repo: &Repository,
    remote: &Remote<'_>,
    specs: &[RefSpec],
    local: &[LocalRef],
    remote_refs: &gix_protocol::push::RemoteRefs,
    options: &Options,
    follow_tags: bool,
) -> Result<Vec<Update>> {
    let advertised = advertised_refs(remote_refs);
    let outcome = MatchGroup::from_push_specs(specs.iter().map(RefSpec::to_ref))
        .match_push(
            local_items(local),
            advertised.iter().map(|(name, id)| Item {
                full_ref_name: name.as_bstr(),
                target: id,
                object: None,
            }),
            &mut Resolver { repo },
        )
        .map_err(crate::Error::from_error)?;
    let mut planned: Vec<_> = outcome
        .mappings
        .into_iter()
        .map(|mapping| {
            let (source, new_id) = match mapping.source {
                Matched::Reference { item_index } => (
                    Source::Reference(local[item_index].name.clone()),
                    Some(local[item_index].id),
                ),
                Matched::Object { name, id } => (Source::Object(name.to_owned()), Some(id)),
                Matched::Delete => (Source::Delete, None),
            };
            let update = Update {
                name: mapping.destination,
                source,
                old_id: mapping.remote_index.map(|index| advertised[index].1),
                new_id,
                forced_update: false,
                status: Status::Pending,
                reports: Vec::new(),
            };
            (update, mapping.force)
        })
        .collect();
    if follow_tags {
        let tags = missing_tags(repo, specs, local, &advertised, &planned)?;
        planned.extend(tags.into_iter().map(|tag| (tag, false)));
    }
    planned
        .into_iter()
        .map(|(mut update, force)| {
            let expected = expected_old_id(repo, remote, &options.force_with_lease, update.name.as_bstr())?;
            set_status(repo, &mut update, force || options.force, expected)?;
            Ok(update)
        })
        .collect()
}

/// The updates that create the annotated tags of `local` which the remote doesn't have, if they point to a commit the
/// remote has after the `planned` updates, as `add_missing_tags()` in Git's `remote.c` finds them for `--follow-tags`,
/// unless a negative refspec of `specs` excludes them.
fn missing_tags(
    repo: &Repository,
    specs: &[RefSpec],
    local: &[LocalRef],
    advertised: &[(BString, ObjectId)],
    planned: &[(Update, bool)],
) -> Result<Vec<Update>> {
    let pushed: BTreeMap<&BStr, Option<ObjectId>> = planned
        .iter()
        .map(|(update, _)| (update.name.as_bstr(), update.new_id))
        .collect();
    let mut names = BTreeSet::new();
    let mut tips = Vec::new();
    for (name, old_id) in advertised {
        names.insert(name.as_bstr());
        tips.push(match pushed.get(name.as_bstr()) {
            Some(Some(new_id)) => *new_id,
            _ => *old_id,
        });
    }
    for (update, _) in planned.iter().filter(|(update, _)| update.old_id.is_none()) {
        names.insert(update.name.as_bstr());
        tips.extend(update.new_id);
    }
    let mut tip_commits: Vec<_> = tips.into_iter().filter_map(|id| peel_to_commit(repo, id)).collect();
    tip_commits.sort();
    tip_commits.dedup();

    let mut candidates = Vec::new();
    for tag in local {
        let name = tag.name.as_bstr();
        if !name.starts_with(b"refs/tags/") || names.contains(name) {
            continue;
        }
        // Only annotated tags are followed.
        if repo.try_find_object(tag.id)?.map(|object| object.kind) != Some(gix_object::Kind::Tag) {
            continue;
        }
        if let Some(commit) = peel_to_commit(repo, tag.id) {
            candidates.push((tag, commit));
        }
    }
    if tip_commits.is_empty() || candidates.is_empty() {
        return Ok(Vec::new());
    }
    let candidate_commits: Vec<_> = candidates.iter().map(|(_, commit)| *commit).collect();
    let reachable = reachable_subset(repo, &tip_commits, &candidate_commits)?;
    Ok(candidates
        .into_iter()
        .filter(|(tag, commit)| reachable.contains(commit) && !is_excluded(specs, tag.name.as_bstr()))
        .map(|(tag, _)| Update {
            name: tag.name.as_bstr().to_owned(),
            source: Source::Reference(tag.name.clone()),
            old_id: None,
            new_id: Some(tag.id),
            forced_update: false,
            status: Status::Pending,
            reports: Vec::new(),
        })
        .collect())
}

/// The commits of `candidates` that are reachable from the commits `tips`, as `get_reachable_subset()` in Git's
/// `commit-reach.c` finds them: without looking at commits older than the oldest candidate.
fn reachable_subset(repo: &Repository, tips: &[ObjectId], candidates: &[ObjectId]) -> Result<BTreeSet<ObjectId>> {
    let mut found = BTreeSet::new();
    let oldest = candidates
        .iter()
        .filter_map(|id| repo.find_commit(*id).ok()?.time().ok())
        .map(|time| time.seconds)
        .min();
    let Some(oldest) = oldest else {
        return Ok(found);
    };
    let mut remaining: BTreeSet<_> = candidates.iter().copied().collect();
    let walk = repo
        .rev_walk(tips.iter().copied())
        .sorting(Sorting::ByCommitTimeCutoff {
            order: CommitTimeOrder::NewestFirst,
            seconds: oldest,
        })
        .all()?;
    for info in walk {
        let id = info?.id;
        if remaining.remove(&id) {
            found.insert(id);
            if remaining.is_empty() {
                break;
            }
        }
    }
    Ok(found)
}

/// The commit `id` points to, itself or through annotated tags, or `None` if it's missing or points to something else, as
/// `lookup_commit_reference_gently()` in Git's `commit.c` finds it.
fn peel_to_commit(repo: &Repository, id: ObjectId) -> Option<ObjectId> {
    let object = repo.try_find_object(id).ok()??.peel_tags_to_end().ok()?;
    (object.kind == gix_object::Kind::Commit).then_some(object.id)
}

/// What `lease` expects the remote reference `name` to point to: `None` if it doesn't apply to it, and `Some(None)` if
/// it expects it not to exist, as `apply_cas()` in Git's `remote.c` decides it.
fn expected_old_id(
    repo: &Repository,
    remote: &Remote<'_>,
    lease: &Lease,
    name: &BStr,
) -> Result<Option<Option<ObjectId>>> {
    let use_tracking_ref = match lease
        .refs
        .iter()
        .find(|(refname, _)| refname_match(refname.as_bstr(), name))
    {
        Some((_, Expected::Id(id))) => return Ok(Some((!id.is_null()).then_some(*id))),
        Some((_, Expected::TrackingRef)) => true,
        None => lease.all_tracking,
    };
    if !use_tracking_ref {
        return Ok(None);
    }
    Ok(Some(tracking_ref_id(repo, remote, name)))
}

/// The object the remote-tracking reference of the remote reference `name` points to, or `None` if there is no such
/// reference, as `remote_tracking()` in Git's `remote.c` reads it.
fn tracking_ref_id(repo: &Repository, remote: &Remote<'_>, name: &BStr) -> Option<ObjectId> {
    let (tracking, _) = find_match(remote.refspecs(remote::Direction::Fetch), name)?;
    let mut reference = repo.try_find_reference(tracking.as_bstr()).ok()??;
    if reference.name().as_bstr() != tracking.as_bstr() {
        return None;
    }
    reference.follow_to_object().ok().map(crate::Id::detach)
}

/// Set the status of `update`, which `force` allows to not be a fast-forward and for which a lease `expected` the
/// remote reference to point to the given object if it is `Some`, as `set_ref_status_for_push()` in Git's `remote.c`
/// does.
fn set_status(repo: &Repository, update: &mut Update, mut force: bool, expected: Option<Option<ObjectId>>) -> Result {
    if update.new_id.is_some() && update.old_id == update.new_id {
        update.status = Status::UpToDate;
        return Ok(());
    }
    let mut rejection = None;
    if let Some(expected) = expected {
        if update.old_id == expected {
            force = true;
        } else {
            rejection = Some(Rejection::Stale);
        }
    }
    if rejection.is_none()
        && let (Some(old_id), Some(new_id)) = (update.old_id, update.new_id)
    {
        rejection = if update.name.starts_with(b"refs/tags/") {
            Some(Rejection::AlreadyExists)
        } else if !repo.has_object(old_id) {
            Some(Rejection::FetchFirst)
        } else {
            match (peel_to_commit(repo, old_id), peel_to_commit(repo, new_id)) {
                (Some(old_commit), Some(new_commit)) => {
                    (!is_ancestor(repo, old_commit, new_commit)?).then_some(Rejection::NonFastForward)
                }
                _ => Some(Rejection::NeedsForce),
            }
        };
    }
    match rejection {
        Some(rejection) if !force => update.status = Status::Rejected(rejection),
        Some(_) => update.forced_update = true,
        None => {}
    }
    Ok(())
}

/// Return `true` if the commit `ancestor` is reachable from the commit `descendant`, as `ref_newer()` in Git's
/// `commit-reach.c` decides if an update is a fast-forward.
fn is_ancestor(repo: &Repository, ancestor: ObjectId, descendant: ObjectId) -> Result<bool> {
    if ancestor == descendant {
        return Ok(true);
    }
    // The commits that have the ancestor as parent are among those reachable from the descendant but not from the
    // ancestor, and no such commit names it as parent otherwise.
    for info in repo.rev_walk([descendant]).with_hidden([ancestor]).all()? {
        if info?.parent_ids.contains(&ancestor) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Find what the first of `specs` that matches `name` on its source side maps it to on its destination side, along
/// with whether that refspec forces updates, as `refspec_find_match()` in Git's `refspec.c` finds it. Nothing is
/// found if a negative refspec excludes `name`.
pub(super) fn find_match(specs: &[RefSpec], name: &BStr) -> Option<(BString, bool)> {
    if matches_negative(specs, name) {
        return None;
    }
    specs.iter().find_map(|spec| {
        let force = spec.allow_non_fast_forward();
        let spec = spec.to_ref();
        if negative_source(spec.instruction()).is_some() {
            return None;
        }
        let (Some(source), Some(destination)) = (spec.source(), spec.destination()) else {
            return None;
        };
        if source.contains(&b'*') {
            replace_glob(source, name, destination).map(|destination| (destination, force))
        } else {
            (source == name).then(|| (destination.to_owned(), force))
        }
    })
}

/// Return `true` if a negative refspec of `specs` excludes what `name` is matched with, as
/// `refspec_find_negative_match()` in Git's `refspec.c` decides it for a query by source.
fn matches_negative(specs: &[RefSpec], name: &BStr) -> bool {
    let negatives: Vec<_> = specs
        .iter()
        .filter_map(|spec| negative_source(spec.to_ref().instruction()))
        .collect();
    if negatives.is_empty() {
        return false;
    }
    let mut matched = Vec::new();
    for spec in specs.iter().map(RefSpec::to_ref) {
        if negative_source(spec.instruction()).is_some() {
            continue;
        }
        match (spec.source(), spec.destination()) {
            (Some(source), destination) if source.contains(&b'*') => {
                matched.extend(replace_glob(destination.unwrap_or(source), name, source));
            }
            (None, None) => matched.push(name.to_owned()),
            (Some(source), _) if source == name => matched.push(source.to_owned()),
            _ => {}
        }
    }
    matched.iter().any(|name| {
        negatives
            .iter()
            .any(|negative| source_matches(negative, name.as_bstr()))
    })
}

/// Return `true` if a negative refspec of `specs` excludes the remote reference `name`, as
/// `apply_negative_refspecs()` in Git's `refspec.c` excludes it.
fn is_excluded(specs: &[RefSpec], name: &BStr) -> bool {
    specs
        .iter()
        .filter_map(|spec| negative_source(spec.to_ref().instruction()))
        .any(|negative| source_matches(negative, name))
}

/// The source of `instruction` if it is that of a negative refspec.
fn negative_source(instruction: Instruction<'_>) -> Option<&BStr> {
    match instruction {
        Instruction::Push(instruction::Push::Exclude { src })
        | Instruction::Fetch(instruction::Fetch::Exclude { src }) => Some(src),
        _ => None,
    }
}

/// Return `true` if the source of a refspec, a pattern or a name, matches `name`, as `refspec_match()` in Git's
/// `refspec.c` matches them.
fn source_matches(source: &BStr, name: &BStr) -> bool {
    if source.contains(&b'*') {
        replace_glob(source, name, "*".into()).is_some()
    } else {
        source == name
    }
}

/// Substitute the `*` of `replacement` with what the `*` of `pattern` matches in `name`, if `pattern` matches `name`,
/// as `match_refname_with_pattern()` in Git's `refspec.c` does.
fn replace_glob(pattern: &BStr, name: &BStr, replacement: &BStr) -> Option<BString> {
    let star = pattern.find_byte(b'*')?;
    let (prefix, suffix) = (&pattern.as_bytes()[..star], &pattern.as_bytes()[star + 1..]);
    let matched = name.strip_prefix(prefix).and_then(|rest| rest.strip_suffix(suffix))?;
    let star = replacement.find_byte(b'*')?;
    let mut out = BString::from(&replacement[..star]);
    out.push_str(matched);
    out.push_str(&replacement[star + 1..]);
    Some(out)
}

/// Answers what matching push refspecs needs to know about the local repository, as Git finds it out.
struct Resolver<'repo> {
    repo: &'repo Repository,
}

impl Resolve for Resolver<'_> {
    fn object(&mut self, spec: &BStr) -> Option<ObjectId> {
        #[cfg(feature = "revision")]
        {
            self.repo.rev_parse_single(spec).ok().map(crate::Id::detach)
        }
        #[cfg(not(feature = "revision"))]
        {
            // Without revision parsing, objects can only be named by their full id or by references.
            match ObjectId::from_hex(spec) {
                Ok(id) if id.kind() == self.repo.object_hash() => Some(id),
                _ => self
                    .repo
                    .try_find_reference(spec)
                    .ok()??
                    .follow_to_object()
                    .ok()
                    .map(crate::Id::detach),
            }
        }
    }

    fn reference(&mut self, name: &BStr) -> Option<ResolvedReference> {
        // Like `refs_resolve_ref_unsafe()`, take the name as is, and follow at most five symbolic references.
        let mut reference = self.repo.try_find_reference(name).ok()??;
        if reference.name().as_bstr() != name {
            return None;
        }
        let mut is_symbolic = false;
        let mut depth = 0;
        while let Some(next) = reference.follow() {
            depth += 1;
            if depth > 5 {
                return None;
            }
            reference = next.ok()?;
            is_symbolic = true;
        }
        Some(ResolvedReference {
            name: reference.name().as_bstr().to_owned(),
            is_symbolic,
        })
    }
}
