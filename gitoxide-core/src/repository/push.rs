use gix::bstr::BString;

/// The options of `gix push`, named after those of `git push`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The remote to push to by name or URL, or `None` for the one that `git push` chooses for the current branch.
    pub repository: Option<BString>,
    /// The refspecs to push, or none for those of `remote.<name>.push` or `push.default`.
    pub refspecs: Vec<BString>,
    /// If `true`, update remote references that aren't fast-forwards or don't match a lease, as `--force` does.
    pub force: bool,
    /// The values of `--force-with-lease`, in order, each `<refname>[:<expect>]`, or empty for the option without value.
    pub force_with_lease: Vec<BString>,
    /// If `true`, update all remote references or none, as `--atomic` does.
    pub atomic: bool,
    /// If `true`, push all tags as well, as `--tags` does.
    pub tags: bool,
    /// If `true`, push the annotated tags that point into what is pushed, as `--follow-tags` does, and otherwise do as
    /// `push.followTags` says.
    pub follow_tags: bool,
    /// If `true`, delete the remote references that the refspecs name, as `--delete` does.
    pub delete: bool,
    /// If `true`, do everything but update the remote, as `--dry-run` does.
    pub dry_run: bool,
    /// If `true`, don't run the `pre-push` hook, as `--no-verify` does.
    pub no_verify: bool,
    /// The options to send to the remote, as `--push-option` gives them, or none for those of `push.pushOption`.
    pub push_options: Vec<BString>,
    /// If `true`, send a pack without deltas against objects that only the remote has, as `--no-thin` does.
    pub no_thin: bool,
}

/// The levels of progress to show while pushing.
pub const PROGRESS_RANGE: std::ops::RangeInclusive<u8> = 1..=3;

pub(crate) mod function {
    use gix::{
        Result,
        bstr::{BStr, BString, ByteSlice},
        error::{ErrorExt, ResultExt, bail, message},
        protocol::push::response::Unpack,
        remote::{Direction, push},
    };

    use super::Options;
    use crate::repository::{advice_enabled, advise, advise_if_enabled};

    /// Push as `git push --porcelain` does with `options`, and print what it prints: `To <url>` and a line for each
    /// remote reference as `print_one_push_report()` in Git's `transport.c` prints them, and `Done`, as
    /// `transport_push()` does unless the remote couldn't unpack the objects it received.
    ///
    /// Fail with `failed to push some refs to '<url>'` after reporting, as `push_with_options()` in Git's
    /// `builtin/push.c` does, if a reference was rejected or not updated, or if pushing failed after the updates were
    /// decided, like when the `pre-push` hook fails, unless the remote lacks a capability the push needs, for which
    /// `send_pack()` in Git's `send-pack.c` dies. Before, write the hint it gives for the rejections to `err`.
    pub fn push<P>(
        repo: gix::Repository,
        mut progress: P,
        mut out: impl std::io::Write,
        mut err: impl std::io::Write,
        Options {
            repository,
            refspecs,
            force,
            force_with_lease,
            atomic,
            tags,
            follow_tags,
            delete,
            dry_run,
            no_verify,
            push_options,
            no_thin,
        }: Options,
    ) -> Result<()>
    where
        P: gix::NestedProgress,
        P::SubProgress: 'static,
    {
        let lease = lease(&repo, &force_with_lease)?;
        if let Some(name) = &repository {
            refuse_ref_like_target(&repo, name.as_bstr(), &mut err)?;
        }
        let remote = repo.find_push_remote(repository.as_ref().map(|name| name.as_bstr()))?;
        let url = remote.url(Direction::Push).map(anonymized).unwrap_or_default();
        let outcome = remote
            .connect(Direction::Push)?
            .prepare_push(
                &mut progress,
                push::Options {
                    refspecs,
                    delete,
                    tags,
                    follow_tags: follow_tags.then_some(true),
                    force,
                    force_with_lease: lease,
                    atomic,
                    push_options: (!push_options.is_empty()).then_some(push_options),
                    thin: !no_thin,
                    ..Default::default()
                },
            )?
            .with_dry_run(dry_run)
            .with_pre_push_hook(!no_verify)
            .push(&mut progress, &gix::interrupt::IS_INTERRUPTED)
            .map_err(|err| {
                // Git dies without reporting failed refs if the remote lacks a capability the push needs.
                if err.is_unsupported() {
                    err
                } else {
                    err.and_raise(message!("failed to push some refs to '{url}'"))
                }
            })?;

        let lines = outcome.porcelain(&repo);
        if !lines.is_empty() {
            writeln!(out, "To {url}").or_error()?;
        }
        for line in lines {
            out.write_all(&line).or_error()?;
            out.write_all(b"\n").or_error()?;
        }
        let success = outcome.is_success();
        let unpack_error = match outcome.unpack {
            Some(Unpack::Failed { error }) => Some(error),
            _ => None,
        };
        if unpack_error.is_none() {
            writeln!(out, "Done").or_error()?;
        }
        if success {
            return Ok(());
        }
        advise_on_rejections(&repo, &outcome.updates, &mut err)?;
        match unpack_error {
            Some(error) => {
                Err(message!("remote unpack failed: {error}")
                    .and_raise(message!("failed to push some refs to '{url}'")))
            }
            None => bail!("failed to push some refs to '{url}'"),
        }
    }

    /// The advice that enables each hint of `push_with_options()` in Git's `builtin/push.c` about rejected updates, and the
    /// hint, in the order in which Git chooses among them: for an update of the branch that `HEAD` points to that isn't
    /// a fast-forward, for other updates that aren't, and for updates rejected as the tag exists, as the remote has
    /// unknown objects and as they need force.
    const REJECTION_HINTS: [(&str, &str); 5] = [
        (
            "pushNonFFCurrent",
            "Updates were rejected because the tip of your current branch is behind\nits remote counterpart. If you want \
             to integrate the remote changes,\nuse 'git pull' before pushing again.\nSee the 'Note about fast-forwards' \
             in 'git push --help' for details.",
        ),
        (
            "pushNonFFMatching",
            "Updates were rejected because a pushed branch tip is behind its remote\ncounterpart. If you want to \
             integrate the remote changes, use 'git pull'\nbefore pushing again.\nSee the 'Note about fast-forwards' \
             in 'git push --help' for details.",
        ),
        (
            "pushAlreadyExists",
            "Updates were rejected because the tag already exists in the remote.",
        ),
        (
            "pushFetchFirst",
            "Updates were rejected because the remote contains work that you do not\nhave locally. This is usually \
             caused by another repository pushing to\nthe same ref. If you want to integrate the remote changes, use\n\
             'git pull' before pushing again.\nSee the 'Note about fast-forwards' in 'git push --help' for details.",
        ),
        (
            "pushNeedsForce",
            "You cannot update a remote ref that points at a non-commit object,\nor update a remote ref to make it point \
             at a non-commit object,\nwithout using the '--force' option.",
        ),
    ];

    /// Write the hint that `push_with_options()` in Git's `builtin/push.c` gives about the rejected `updates` to `err`, as
    /// `advise()` in Git's `advice.c` writes it, unless `GIT_ADVICE` turns advice off or `advice.pushUpdateRejected`, its
    /// alias `advice.pushNonFastForward` or the advice of the hint is disabled, as `advice_enabled()` decides. As
    /// `transport_print_push_status()` in Git's `transport.c` decides, an update is of the current branch if the remote
    /// reference has the name of the branch that `HEAD` points to.
    fn advise_on_rejections(
        repo: &gix::Repository,
        updates: &[push::Update],
        mut err: impl std::io::Write,
    ) -> Result<()> {
        let head = repo.head_name()?;
        let hint = updates
            .iter()
            .filter_map(|update| match &update.status {
                push::Status::Rejected(push::Rejection::NonFastForward) => {
                    Some(if head.as_ref().is_some_and(|head| head.as_bstr() == update.name) {
                        0
                    } else {
                        1
                    })
                }
                push::Status::Rejected(push::Rejection::AlreadyExists) => Some(2),
                push::Status::Rejected(push::Rejection::FetchFirst) => Some(3),
                push::Status::Rejected(push::Rejection::NeedsForce) => Some(4),
                _ => None,
            })
            .min();
        let Some((advice, hint)) = hint.map(|index| REJECTION_HINTS[index]) else {
            return Ok(());
        };
        if ["pushUpdateRejected", "pushNonFastForward", advice]
            .iter()
            .all(|key| advice_enabled(repo, key))
        {
            advise(hint, &mut err)?;
        }
        Ok(())
    }

    /// Return the lease of the `values` of `--force-with-lease` as `parse_push_cas_option()` in Git's `remote.c` reads them:
    /// an empty value, given for the option without value, expects all remote references to be where their
    /// remote-tracking references are, `<refname>` expects that of `<refname>`, `<refname>:` expects no reference, and
    /// `<refname>:<expect>` expects the object that `<expect>` names.
    fn lease(repo: &gix::Repository, values: &[BString]) -> Result<push::Lease> {
        let mut lease = push::Lease::default();
        for value in values {
            if value.is_empty() {
                lease.all_tracking = true;
                continue;
            }
            let entry = match value.find_byte(b':') {
                None => (value.clone(), push::Expected::TrackingRef),
                Some(colon) => {
                    let expect = value[colon + 1..].as_bstr();
                    let id = if expect.is_empty() {
                        repo.object_hash().null()
                    } else {
                        repo.rev_parse_single(expect)
                            .or_raise(|| message!("cannot parse expected object name '{expect}'"))?
                            .detach()
                    };
                    (value[..colon].into(), push::Expected::Id(id))
                }
            };
            lease.refs.push(entry);
        }
        Ok(lease)
    }

    /// Refuse `name`, the repository to push to, as `die_if_repo_looks_like_ref()` in Git's `builtin/push.c` refuses it
    /// since Git 2.56 unless the advice `pushRepoLooksLikeRef` is disabled: if it isn't a configured remote and isn't an
    /// existing path, but looks like `<remote>/<branch>` for a configured remote. Write Git's hint to `err` before.
    fn refuse_ref_like_target(repo: &gix::Repository, name: &BStr, mut err: impl std::io::Write) -> Result<()> {
        if !advice_enabled(repo, "pushRepoLooksLikeRef") {
            return Ok(());
        }
        let Some(slash) = name.find_byte(b'/') else {
            return Ok(());
        };
        let (remote, branch) = (name[..slash].as_bstr(), name[slash + 1..].as_bstr());
        let remotes = repo.remote_names();
        if remotes.contains(name)
            || branch.is_empty()
            || !remotes.contains(remote)
            || gix::path::from_bstr(name)?.exists()
        {
            return Ok(());
        }
        advise_if_enabled(
            repo,
            "pushRepoLooksLikeRef",
            &format!("Did you mean to use: git push {remote} {branch}?"),
            &mut err,
        )?;
        bail!("'{name}' is not a valid push target".validation())
    }

    /// Return `url` as `transport_anonymize_url()` in Git's `transport.c` shows it, without the user and the password.
    fn anonymized(url: &gix::Url) -> BString {
        let mut url = url.clone();
        url.set_user(None);
        url.set_password(None);
        url.to_bstring()
    }
}
