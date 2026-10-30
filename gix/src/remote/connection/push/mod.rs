//! Push references and the objects they need to a remote over an established connection, as `git push` does.
#[cfg(feature = "async-network-client")]
use gix_transport::client::async_io::Transport;
#[cfg(feature = "blocking-network-client")]
use gix_transport::client::blocking_io::Transport;

use gix_hash::ObjectId;
use gix_protocol::push::response::Report;

use crate::{
    Progress, Repository, Result,
    bstr::{BStr, BString, ByteSlice, ByteVec},
    config::{cache::util::ApplyLeniency, tree::Push},
    error::{ResultExt, bail, message},
    ext::ObjectIdExt,
    remote::Connection,
};

mod pack;
mod refs;
mod send;

/// Options for [`Connection::prepare_push()`], named after the options of `git push` they correspond to.
///
/// What they leave open is read from the configuration of the repository, as `git push` reads it.
#[derive(Debug, Clone)]
pub struct Options {
    /// The refspecs to push, like `main`, `HEAD:refs/heads/topic`, `+refs/heads/*:refs/heads/*` or `:refs/heads/old`,
    /// as `git push <remote> <refspec>…` receives them.
    ///
    /// A refspec without `:` that names a single local reference is mapped as `git push` maps it: with the push
    /// refspecs of the remote, or with `push.default=upstream` to the upstream branch of the branch it names. The
    /// refspec `tag` followed by `<tag>` stands for `refs/tags/<tag>`. Without refspecs, those of `remote.<name>.push`
    /// are used, and without these, the current branch is pushed as `push.default` says.
    pub refspecs: Vec<BString>,
    /// If `true`, delete the remote references that the [refspecs](Self::refspecs) name, as `--delete` does.
    pub delete: bool,
    /// If `true`, push all tags along with what the refspecs push, as `--tags` does.
    pub tags: bool,
    /// If `Some(true)`, also push the annotated tags that point into what the remote has after the push, unless it
    /// has them already, as `--follow-tags` does, and don't with `Some(false)`. With `None`, `push.followTags` decides.
    pub follow_tags: Option<bool>,
    /// If `true`, update remote references even if the update isn't a fast-forward or the remote reference isn't what
    /// the [lease](Self::force_with_lease) expects, as `--force` does.
    pub force: bool,
    /// What remote references are expected to point to, to update them only then, but even if the update isn't a
    /// fast-forward, as `--force-with-lease` does.
    pub force_with_lease: Lease,
    /// If `true`, update all remote references or none of them, as `--atomic` does.
    pub atomic: bool,
    /// The strings to pass to the hooks of the remote, as `--push-option` does. With `None`, the values of
    /// `push.pushOption` are used.
    pub push_options: Option<Vec<BString>>,
    /// If `true`, the pack may contain deltas against objects the remote has but the pack doesn't, as `git push` sends
    /// them unless the remote asks for `no-thin`. Set it to `false` for `--no-thin`.
    pub thin: bool,
    /// Parameters in the form of `(name, optional value)` to add to the handshake.
    ///
    /// This is useful in case of custom servers.
    pub handshake_parameters: Vec<(String, Option<String>)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            refspecs: Vec::new(),
            delete: false,
            tags: false,
            follow_tags: None,
            force: false,
            force_with_lease: Lease::default(),
            atomic: false,
            push_options: None,
            thin: true,
            handshake_parameters: Vec::new(),
        }
    }
}

/// What remote references are expected to point to before a push updates them, as `--force-with-lease` sets it.
///
/// A remote reference that points to what is expected is updated even if it isn't a fast-forward, and one that
/// doesn't isn't updated, unless [`force`](Options::force) is set as well.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lease {
    /// The names of references along with what they are expected to point to, as given in order with
    /// `--force-with-lease=<refname>[:<expect>]`.
    ///
    /// A remote reference uses the first entry whose name expands to its full name as `git rev-parse` expands names,
    /// so `main` applies to `refs/heads/main`.
    pub refs: Vec<(BString, Expected)>,
    /// If `true`, the remote references that no entry of [`refs`](Self::refs) applies to are expected to point to what
    /// their remote-tracking references point to, as `--force-with-lease` without value expects it.
    pub all_tracking: bool,
}

/// What a [`Lease`] expects a remote reference to point to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    /// What its remote-tracking reference points to, or that it doesn't exist if there is no remote-tracking reference,
    /// as `--force-with-lease=<refname>` expects.
    TrackingRef,
    /// The given object, or that it doesn't exist if the id is null, as `--force-with-lease=<refname>:<expect>`
    /// expects, where an empty `<expect>` stands for the null id.
    Id(ObjectId),
}

/// What a remote reference is updated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The object a local reference points to, by the full name of the reference, like `refs/heads/main`.
    Reference(gix_ref::FullName),
    /// The object that the source of a refspec names without being a local reference, like `HEAD` or `main~1`,
    /// spelled as in the refspec.
    Object(BString),
    /// Nothing, as the remote reference is deleted.
    Delete,
}

impl Source {
    /// The name `git push` gives the source: the full name of the local reference, the source as the refspec spells it,
    /// or `(delete)`, as the `pre-push` hook receives it.
    pub fn name(&self) -> &BStr {
        match self {
            Source::Reference(name) => name.as_bstr(),
            Source::Object(spec) => spec.as_bstr(),
            Source::Delete => "(delete)".into(),
        }
    }
}

/// Why an update was rejected before anything was sent, as `git push` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// The update isn't a fast-forward and isn't forced.
    NonFastForward,
    /// The update would change a tag, which only happens by force.
    AlreadyExists,
    /// The remote reference points to an object that isn't available locally, so it isn't known whether the update is a
    /// fast-forward.
    FetchFirst,
    /// The remote reference or what it is updated with isn't a commit, nor a tag pointing to one, so the update only
    /// happens by force.
    NeedsForce,
    /// The remote reference doesn't point to what the [lease](Options::force_with_lease) expects.
    Stale,
    /// The remote doesn't support deleting references.
    DeleteUnsupported,
    /// The push is [atomic](Options::atomic) and another update was rejected.
    AtomicPushFailed,
}

impl Rejection {
    /// The reason `git push` gives for the rejection.
    pub fn reason(&self) -> &'static str {
        match self {
            Rejection::NonFastForward => "non-fast-forward",
            Rejection::AlreadyExists => "already exists",
            Rejection::FetchFirst => "fetch first",
            Rejection::NeedsForce => "needs force",
            Rejection::Stale => "stale info",
            Rejection::DeleteUnsupported => "remote does not support deleting refs",
            Rejection::AtomicPushFailed => "atomic push failed",
        }
    }
}

/// What happened to an update, with the statuses `git push --porcelain` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The update passed the checks made before pushing and is to be sent, which only the updates of a
    /// [`Prepare`] can be.
    Pending,
    /// The remote updated its reference, or would have in a dry-run.
    Ok,
    /// The remote reference already points to the object, so no update was sent.
    UpToDate,
    /// The update wasn't sent, for the given reason.
    Rejected(Rejection),
    /// The remote didn't update its reference, for the reason it gave.
    RemoteRejected {
        /// The reason the remote gave.
        reason: BString,
    },
    /// The remote didn't report what it did with the update.
    NotReported,
}

/// An update of a remote reference, as `git push` decides and reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The full name of the remote reference, like `refs/heads/main`.
    pub name: BString,
    /// What the remote reference is updated with.
    pub source: Source,
    /// The object the remote reference points to, or `None` if the remote doesn't have it.
    pub old_id: Option<ObjectId>,
    /// The object the remote reference is updated with, or `None` if it is deleted.
    pub new_id: Option<ObjectId>,
    /// If `true`, the update isn't a fast-forward and only happens as it is forced, which `git push` reports as
    /// `forced update`.
    pub forced_update: bool,
    /// What happened to the update.
    pub status: Status,
    /// What the remote did instead of what the update asked for, once for each reference it updated, as it reports
    /// with `report-status-v2`.
    pub reports: Vec<Report>,
}

impl Update {
    /// The lines `git push --porcelain` prints for this update, without their trailing newline, with object ids
    /// abbreviated as `repo` abbreviates them: one line, or one for each of the [reports](Self::reports), and none
    /// while the update is [pending](Status::Pending).
    pub fn porcelain(&self, repo: &Repository) -> Vec<BString> {
        if self.reports.is_empty() {
            self.porcelain_line(repo, None).into_iter().collect()
        } else {
            self.reports
                .iter()
                .filter_map(|report| self.porcelain_line(repo, Some(report)))
                .collect()
        }
    }

    /// The line that `print_one_push_report()` in Git's `transport.c` prints for this update, as changed by `report`.
    fn porcelain_line(&self, repo: &Repository, report: Option<&Report>) -> Option<BString> {
        let deletion = self.new_id.is_none();
        let to = report
            .and_then(|report| report.name.as_ref())
            .map_or(self.name.as_bstr(), |name| name.as_bstr());
        let from = self.source.name();
        Some(match &self.status {
            Status::Pending => return None,
            Status::Ok => self.porcelain_ok_line(repo, report, to),
            Status::UpToDate => porcelain_line('=', Some(from), to, "[up to date]".into(), None),
            Status::Rejected(rejection @ Rejection::DeleteUnsupported) => {
                porcelain_line('!', None, to, "[rejected]".into(), Some(rejection.reason().into()))
            }
            Status::Rejected(rejection) => porcelain_line(
                '!',
                Some(from),
                to,
                "[rejected]".into(),
                Some(rejection.reason().into()),
            ),
            Status::RemoteRejected { reason } => porcelain_line(
                '!',
                (!deletion).then_some(from),
                to,
                "[remote rejected]".into(),
                Some(reason.as_bstr()),
            ),
            Status::NotReported => porcelain_line(
                '!',
                (!deletion).then_some(from),
                to,
                "[remote failure]".into(),
                Some("remote failed to report status".into()),
            ),
        })
    }

    /// The line that `print_ok_ref_status()` in Git's `transport.c` prints for this update, as changed by `report`.
    fn porcelain_ok_line(&self, repo: &Repository, report: Option<&Report>, to: &BStr) -> BString {
        let new_id = report.and_then(|report| report.new_id).or(self.new_id);
        let Some(new_id) = new_id.filter(|_| self.new_id.is_some()) else {
            return porcelain_line('-', None, to, "[deleted]".into(), None);
        };
        let from = Some(self.source.name());
        let old_id = report
            .and_then(|report| report.old_id)
            .or(self.old_id)
            .filter(|id| !id.is_null());
        let Some(old_id) = old_id else {
            let summary = if to.starts_with(b"refs/tags/") {
                "[new tag]"
            } else if to.starts_with(b"refs/heads/") {
                "[new branch]"
            } else {
                "[new reference]"
            };
            return porcelain_line('*', from, to, summary.into(), None);
        };
        let forced_update = report.is_some_and(|report| report.forced_update) || self.forced_update;
        let range = format!(
            "{}{}{}",
            abbreviate(repo, old_id),
            if forced_update { "..." } else { ".." },
            abbreviate(repo, new_id)
        );
        if forced_update {
            porcelain_line('+', from, to, range.as_str().into(), Some("forced update".into()))
        } else {
            porcelain_line(' ', from, to, range.as_str().into(), None)
        }
    }
}

/// A line as `print_ref_status()` in Git's `transport.c` prints it for `git push --porcelain`.
fn porcelain_line(flag: char, from: Option<&BStr>, to: &BStr, summary: &BStr, reason: Option<&BStr>) -> BString {
    let mut line = BString::from(format!("{flag}\t"));
    if let Some(from) = from {
        line.push_str(from);
    }
    line.push_byte(b':');
    line.push_str(to);
    line.push_byte(b'\t');
    line.push_str(summary);
    if let Some(reason) = reason {
        line.push_str(" (");
        line.push_str(reason);
        line.push_byte(b')');
    }
    line
}

/// Abbreviate `id` as Git abbreviates it when it reports a push: as short as `core.abbrev` allows while unambiguous
/// among the objects of `repo`, also if the object itself isn't available.
fn abbreviate(repo: &Repository, id: ObjectId) -> String {
    match id.attach(repo).shorten() {
        Ok(prefix) => prefix.to_string(),
        Err(_) => id.to_hex_with_len(repo.config.hex_len.unwrap_or(7)).to_string(),
    }
}

/// The outcome of [`Prepare::push()`].
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The outcome of the handshake with the remote.
    pub handshake: gix_protocol::Handshake,
    /// The updates of the remote references: of those the remote has, in the order it advertised them, followed by
    /// those the push creates.
    pub updates: Vec<Update>,
    /// Whether the remote unpacked the objects it received, or `None` if nothing was sent or the remote doesn't report
    /// it.
    pub unpack: Option<gix_protocol::push::response::Unpack>,
    /// If `true`, nothing was sent as it was a dry-run.
    pub dry_run: bool,
    /// The edits of remote-tracking references made for the remote references the push updated or found up to date,
    /// which is none in a dry-run.
    pub tracking_refs: Vec<gix_ref::transaction::RefEdit>,
}

impl Outcome {
    /// Return `true` if the push succeeded as `git push` sees it: all updates are [done](Status::Ok) or were
    /// [up to date](Status::UpToDate), and the remote unpacked the objects it received.
    pub fn is_success(&self) -> bool {
        !matches!(self.unpack, Some(gix_protocol::push::response::Unpack::Failed { .. }))
            && self
                .updates
                .iter()
                .all(|update| matches!(update.status, Status::Ok | Status::UpToDate))
    }

    /// The lines `git push --porcelain` prints for the [updates](Self::updates), in the order it prints them and
    /// without `To <url>` and `Done`: the updates that were up to date, then those that were done, then the others, as
    /// [`Update::porcelain()`] produces them.
    pub fn porcelain(&self, repo: &Repository) -> Vec<BString> {
        let up_to_date = self.updates.iter().filter(|update| update.status == Status::UpToDate);
        let done = self.updates.iter().filter(|update| update.status == Status::Ok);
        let others = self
            .updates
            .iter()
            .filter(|update| !matches!(update.status, Status::UpToDate | Status::Ok));
        up_to_date
            .chain(done)
            .chain(others)
            .flat_map(|update| update.porcelain(repo))
            .collect()
    }
}

impl<'remote, 'auth, 'repo, T> Connection<'remote, 'auth, 'repo, T>
where
    T: Transport,
{
    /// Decide what to push with `options`, perform the handshake with `git receive-pack` on the remote, and check the
    /// updates of its references as `git push` checks them before it sends anything, so the updates of the returned
    /// [`Prepare`] can be inspected before [pushing](Prepare::push()).
    ///
    /// The refspecs are those of `options`, else the push refspecs of the remote, else the one that `push.default`
    /// implies for the current branch, failing with Git's messages where `git push` fails. A refspec whose source
    /// matches no local reference and names no object, or matches more than one local reference, fails the push before
    /// the handshake, and the remaining problems of the refspecs are found once the references of the remote are
    /// known, as `git push` reports them.
    ///
    /// An update is [rejected](Status::Rejected) if it isn't a fast-forward or changes a tag and isn't forced, if the
    /// object of the remote reference isn't available locally, or if a lease doesn't hold, and is
    /// [up to date](Status::UpToDate) if the remote reference points to the object already. The other updates are
    /// [pending](Status::Pending).
    ///
    /// ### Configuration
    ///
    /// - `push.default` and `branch.<name>.merge` decide what to push if no refspec is given, and `remote.pushDefault`,
    ///   `branch.<name>.remote` and the remotes that are configured decide which remote is the upstream of the current
    ///   branch. Only values that we deem [trustworthy][crate::open::Options::filter_config_section()] are used.
    /// - `push.followTags` and `push.pushOption` are used unless `options` set them.
    /// - `gitoxide.userAgent` is read to obtain the application user agent for git servers and for HTTP servers as well.
    ///
    /// # Async Experimental
    ///
    /// Note that this implementation is currently limited correctly in blocking mode only as it relies on Drop semantics
    /// to close the connection should the push not be performed. Furthermore, there the code writing the pack is
    /// inherently blocking and it's not offloaded to a thread, making the push block the executor.
    #[gix_protocol::bisync::bisync]
    pub async fn prepare_push(
        mut self,
        mut progress: impl Progress,
        options: Options,
    ) -> Result<Prepare<'remote, 'auth, 'repo, T>> {
        let _span = gix_trace::coarse!("remote::Connection::prepare_push()");
        let repo = self.remote.repo;
        let local = refs::local_refs(repo)?;
        let specs = refs::refspecs(repo, self.remote, &options, &local)?;
        refs::check_sources(repo, &specs, &local)?;
        let follow_tags = match options.follow_tags {
            Some(enabled) => enabled,
            None => follow_tags(repo)?,
        };
        let push_options = match &options.push_options {
            Some(push_options) => push_options.clone(),
            None => push_options(repo),
        };
        if push_options.iter().any(|option| option.contains(&b'\n')) {
            bail!(gix_error::validation("push options must not have new line characters"));
        }

        let mut credentials_storage;
        let url = self.transport.inner.to_url();
        let authenticate = match self.authenticate.as_mut() {
            Some(f) => f,
            None => {
                credentials_storage = self.configured_credentials_for_current_url();
                &mut credentials_storage
            }
        };
        if self.transport_options.is_none() {
            self.transport_options = repo
                .transport_options(url.as_ref(), self.remote.name().map(crate::remote::Name::as_bstr))
                .or_raise(|| message!("Failed to configure the transport before connecting to {url:?}"))?;
        }
        if let Some(config) = self.transport_options.as_ref() {
            self.transport
                .inner
                .configure(&**config)
                .or_raise(|| message("Failed to configure the transport layer"))?;
        }
        let handshake = gix_protocol::handshake(
            &mut self.transport.inner,
            gix_transport::Service::ReceivePack,
            authenticate,
            options.handshake_parameters.clone(),
            &mut progress,
        )
        .await?;
        let remote_refs = gix_protocol::push::RemoteRefs::from_handshake(&handshake)?;
        let updates = refs::updates(repo, self.remote, &specs, &local, &remote_refs, &options, follow_tags)?;
        Ok(Prepare {
            con: self,
            handshake,
            remote_refs,
            updates,
            atomic: options.atomic,
            push_options,
            thin: options.thin,
            dry_run: false,
            pre_push_hook: true,
        })
    }
}

/// Return `push.followTags`, or `false` if it isn't set.
fn follow_tags(repo: &Repository) -> Result<bool> {
    Ok(Push::FOLLOW_TAGS
        .enrich_error(
            repo.config
                .resolved
                .boolean_filter(Push::FOLLOW_TAGS, &mut repo.filter_config_section()),
        )
        .with_leniency(repo.config.lenient_config)?
        .unwrap_or(false))
}

/// Return the values of `push.pushOption` in order, where an empty value removes the values before it, as
/// `parse_transport_option()` in Git's `transport.c` collects them.
fn push_options(repo: &Repository) -> Vec<BString> {
    let mut out = Vec::new();
    for value in repo
        .config
        .resolved
        .strings_filter(Push::PUSH_OPTION, &mut repo.filter_config_section())
        .unwrap_or_default()
    {
        if value.is_empty() {
            out.clear();
        } else {
            out.push(value);
        }
    }
    out
}

/// A push whose updates are decided and checked, as returned by [`Connection::prepare_push()`], to be performed with
/// [`push()`](Prepare::push()).
pub struct Prepare<'remote, 'auth, 'repo, T>
where
    T: Transport,
{
    con: Connection<'remote, 'auth, 'repo, T>,
    handshake: gix_protocol::Handshake,
    remote_refs: gix_protocol::push::RemoteRefs,
    updates: Vec<Update>,
    atomic: bool,
    push_options: Vec<BString>,
    thin: bool,
    dry_run: bool,
    pre_push_hook: bool,
}

/// Access
impl<T> Prepare<'_, '_, '_, T>
where
    T: Transport,
{
    /// The outcome of the handshake with the remote.
    pub fn handshake(&self) -> &gix_protocol::Handshake {
        &self.handshake
    }

    /// The references the remote advertised, and the objects it has without naming references for them.
    pub fn remote_refs(&self) -> &gix_protocol::push::RemoteRefs {
        &self.remote_refs
    }

    /// The updates of remote references the push makes, of those the remote has in the order it advertised them,
    /// followed by those the push creates, with the status the checks made before pushing gave them.
    pub fn updates(&self) -> &[Update] {
        &self.updates
    }
}

/// Builder
impl<T> Prepare<'_, '_, '_, T>
where
    T: Transport,
{
    /// If enabled, do everything but send the updates and the pack, as `--dry-run` does: the `pre-push` hook runs, and
    /// the updates that would be sent are reported as [done](Status::Ok), but no remote-tracking reference is changed.
    pub fn with_dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// If disabled, don't run the `pre-push` hook, as `--no-verify` does. It's enabled by default, but only runs if the
    /// `hooks` feature is enabled. While it's enabled, pushing fails in repositories whose Git directory isn't fully
    /// trusted, as `Repository::hooks()` only returns the hooks of those.
    pub fn with_pre_push_hook(mut self, enabled: bool) -> Self {
        self.pre_push_hook = enabled;
        self
    }
}
