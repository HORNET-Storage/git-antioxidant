//! Send the updates of a push along with the objects they need, and record what happened, as `transport_push()` in
//! Git's `transport.c` and `send_pack()` in its `send-pack.c` do.
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

use gix_hash::ObjectId;
use gix_protocol::push::response;
use gix_ref::{
    FullName,
    transaction::{PreviousValue, RefEdit},
};
#[cfg(feature = "async-network-client")]
use gix_transport::client::async_io::Transport;
#[cfg(feature = "blocking-network-client")]
use gix_transport::client::blocking_io::Transport;

use super::{Outcome, Prepare, Rejection, Status, Update, pack, refs};
use crate::{
    Remote, Result,
    bstr::{BStr, ByteSlice},
    error::{ResultExt, message},
    remote,
};

impl<T> Prepare<'_, '_, '_, T>
where
    T: Transport,
{
    /// Push as prepared: run the `pre-push` hook, send the [pending](Status::Pending) updates along with a pack of the
    /// objects the remote lacks, and update the remote-tracking references of the remote references that were updated
    /// or are up to date, as `git push` does.
    ///
    /// * The `pre-push` hook receives the updates that aren't up to date, nor rejected as they aren't fast-forwards or
    ///   as a lease doesn't hold. If it fails, nothing is pushed and an error is returned, as it is if the Git directory
    ///   isn't fully trusted, since `Repository::hooks()` only returns the hooks of trusted ones. Without the `hooks`
    ///   feature, no hook runs.
    /// * The push fails if the remote doesn't support the hash of the repository, [atomic](super::Options::atomic)
    ///   pushes or [push options](super::Options::push_options) when they are used. Deleting a reference is
    ///   [rejected](Rejection::DeleteUnsupported) if the remote doesn't support it, and an atomic push sends nothing if
    ///   one of its updates is rejected, which [rejects](Rejection::AtomicPushFailed) its pending updates.
    /// * The pack has the objects the pushed objects reach that the remote doesn't have as far as its references and the
    ///   objects it advertised with `.have` show, and isn't sent if every update deletes a reference. Objects are copied
    ///   from the packs of the repository where possible, along with the deltas whose base is in the pack or, unless
    ///   [`thin`](super::Options::thin) is disabled or the remote asks for `no-thin`, is one of the objects that the
    ///   remote has in the trees of the commits at the edge of what is sent. The other objects are searched for deltas
    ///   as `git pack-objects` searches them, with its defaults and the `pack.window`, `pack.depth`,
    ///   `pack.windowMemory`, `pack.deltaCacheSize`, `pack.deltaCacheLimit`, `pack.threads` and `core.bigFileThreshold`
    ///   keys of the repository, and stored whole where no delta is found.
    /// * The remote-tracking references are found with the fetch refspecs of the remote, or, for a remote made from
    ///   a URL, of the only configured remote that pushes to that URL, as Git does since 2.56. They are updated with
    ///   the reflog message `update by push`, or deleted along with the remote reference. They are left alone in a
    ///   [dry-run](Prepare::with_dry_run()), and if their update fails, an error is returned although the remote
    ///   accepted the push.
    ///
    /// What happened to each update is in the returned [`Outcome`], which says if the push succeeded as a whole.
    ///
    /// ### Deviation
    ///
    /// Git sends the objects of updates it rejected before pushing as well, while here only the objects of the updates
    /// that are sent are in the pack.
    #[gix_protocol::bisync::bisync]
    pub async fn push<P>(self, progress: P, should_interrupt: &AtomicBool) -> Result<Outcome>
    where
        P: gix_utils::progress::NestedProgress,
        P::SubProgress: 'static,
    {
        let Prepare {
            mut con,
            handshake,
            remote_refs,
            mut updates,
            atomic,
            push_options,
            thin,
            dry_run,
            pre_push_hook,
        } = self;
        let remote = con.remote;
        let repo = remote.repo;
        #[cfg(feature = "hooks")]
        if pre_push_hook {
            let url = match remote.url(remote::Direction::Push) {
                Some(url) => url.to_bstring(),
                None => con.transport.inner.to_url().into_owned(),
            };
            run_pre_push_hook(remote, url.as_bstr(), &updates)?;
        }
        #[cfg(not(feature = "hooks"))]
        let _ = pre_push_hook;

        let user_agent = repo.config.user_agent_tuple();
        let options = gix_protocol::push::Options {
            object_hash: repo.object_hash(),
            atomic,
            push_options,
            quiet: false,
        };
        // Fail as `send_pack()` does if the remote doesn't support what the push needs, also in a dry-run.
        let arguments = gix_protocol::push::Arguments::new(&handshake.capabilities, &user_agent, &options)?;
        if !arguments.delete_refs {
            for update in updates.iter_mut().filter(|update| update.new_id.is_none()) {
                update.status = Status::Rejected(Rejection::DeleteUnsupported);
            }
        }
        if atomic
            && updates
                .iter()
                .any(|update| matches!(update.status, Status::Rejected(_)))
        {
            for update in updates.iter_mut().filter(|update| update.status == Status::Pending) {
                update.status = Status::Rejected(Rejection::AtomicPushFailed);
            }
        }

        let pending: Vec<_> = (0..updates.len())
            .filter(|index| updates[*index].status == Status::Pending)
            .collect();
        let null_id = repo.object_hash().null();
        let commands: Vec<_> = if dry_run {
            Vec::new()
        } else {
            pending
                .iter()
                .map(|index| {
                    let update = &updates[*index];
                    gix_protocol::push::Command {
                        name: update.name.clone(),
                        old_id: update.old_id.unwrap_or(null_id),
                        new_id: update.new_id.unwrap_or(null_id),
                    }
                })
                .collect()
        };
        let selection = if commands.iter().any(|command| !command.is_delete()) {
            let tips = commands
                .iter()
                .filter(|command| !command.is_delete())
                .map(|command| command.new_id);
            let haves = remote_refs
                .refs
                .iter()
                .filter_map(|reference| reference.unpack().1.map(ToOwned::to_owned))
                .chain(remote_refs.extra_have.iter().copied());
            Some(pack::select(
                repo,
                tips,
                haves,
                thin && arguments.thin_pack,
                should_interrupt,
            )?)
        } else {
            None
        };

        let outcome = gix_protocol::push(
            &commands,
            |out, arguments, progress, should_interrupt| {
                let Some(selection) = &selection else {
                    crate::error::bail!(message("A pack was to be written although no object is pushed"));
                };
                pack::write(repo, selection, out, arguments, progress, should_interrupt)
            },
            progress,
            should_interrupt,
            gix_protocol::push::Context {
                handshake: &handshake,
                transport: &mut con.transport.inner,
                user_agent,
                trace_packetlines: con.trace,
            },
            &options,
        )
        .await?;

        if dry_run {
            for index in pending {
                updates[index].status = Status::Ok;
            }
        } else {
            for (index, sent) in pending.into_iter().zip(outcome.refs) {
                let update = &mut updates[index];
                update.status = match sent.status {
                    response::Status::Ok => Status::Ok,
                    response::Status::Rejected { reason } => Status::RemoteRejected { reason },
                    response::Status::UpToDate => Status::UpToDate,
                    response::Status::DeleteUnsupported => Status::Rejected(Rejection::DeleteUnsupported),
                    response::Status::AtomicPushFailed => Status::Rejected(Rejection::AtomicPushFailed),
                    response::Status::NotReported => Status::NotReported,
                };
                update.reports = sent.reports;
            }
        }
        let tracking_refs = if dry_run {
            Vec::new()
        } else {
            update_tracking_refs(remote, &updates)?
        };
        Ok(Outcome {
            handshake,
            updates,
            unpack: outcome.unpack,
            dry_run,
            tracking_refs,
        })
    }
}

/// Run the `pre-push` hook of the repository of `remote`, which is pushed to at `url`, with the `updates` that are to be
/// pushed, and fail if it fails, as `run_pre_push_hook()` in Git's `transport.c` does.
#[cfg(feature = "hooks")]
fn run_pre_push_hook(remote: &Remote<'_>, url: &BStr, updates: &[Update]) -> Result {
    let repo = remote.repo;
    let null_id = repo.object_hash().null();
    let name = gix_path::from_bstr(remote.name().map_or(url, |name| name.as_bstr()))?;
    let url = gix_path::from_bstr(url)?;
    // Git leaves out what it won't push as the remote reference is up to date or what it points to isn't expected.
    let pushed = updates.iter().filter(|update| {
        !matches!(
            update.status,
            Status::UpToDate | Status::Rejected(Rejection::NonFastForward | Rejection::Stale)
        )
    });
    let outcome = repo.hooks()?.pre_push(
        name.as_os_str(),
        url.as_os_str(),
        pushed.map(|update| gix_hook::invocation::PushUpdate {
            local_ref: update.source.name(),
            local_object: update.new_id.as_ref().unwrap_or(&null_id),
            remote_ref: update.name.as_bstr(),
            remote_object: update.old_id.as_ref().unwrap_or(&null_id),
        }),
    )?;
    if !outcome.success() {
        crate::error::bail!(
            "The pre-push hook failed with code {}, so nothing was pushed",
            outcome.code()
        );
    }
    Ok(())
}

/// Update the remote-tracking references of the remote references that `updates` updated or found up to date, as
/// `transport_update_tracking_ref()` in Git's `transport.c` does, with the reflog message `update by push`, and
/// return the edits that were made. The fetch refspecs that find them are those of `remote`, or of the configured
/// remote that `Remote::push_tracking_remote()` returns for it.
fn update_tracking_refs(remote: &Remote<'_>, updates: &[Update]) -> Result<Vec<RefEdit>> {
    let repo = remote.repo;
    let tracking_remote = remote.push_tracking_remote();
    let fetch_specs = tracking_remote
        .as_ref()
        .unwrap_or(remote)
        .refspecs(remote::Direction::Fetch);
    let mut names = BTreeSet::new();
    let mut edits = Vec::new();
    for update in updates
        .iter()
        .filter(|update| matches!(update.status, Status::Ok | Status::UpToDate))
    {
        let mut updated: Vec<(&BStr, Option<ObjectId>)> = update
            .reports
            .iter()
            .map(|report| {
                (
                    report
                        .name
                        .as_ref()
                        .map_or(update.name.as_bstr(), |name| name.as_bstr()),
                    report.new_id.or(update.new_id),
                )
            })
            .collect();
        if updated.is_empty() {
            updated.push((update.name.as_bstr(), update.new_id));
        }
        for (name, new_id) in updated {
            let Some((tracking_name, _)) = refs::find_match(fetch_specs, name) else {
                continue;
            };
            let Ok(tracking_name) = FullName::try_from(tracking_name) else {
                continue;
            };
            if !names.insert(tracking_name.clone()) {
                continue;
            }
            let current_id = repo
                .try_find_reference(tracking_name.as_bstr())?
                .and_then(|mut reference| reference.follow_to_object().ok().map(crate::Id::detach));
            match new_id.filter(|id| !id.is_null()) {
                Some(new_id) if current_id != Some(new_id) => edits.push(
                    RefEdit::update(tracking_name, new_id, PreviousValue::Any, "update by push").with_deref(true),
                ),
                None if current_id.is_some() => {
                    edits.push(RefEdit::delete(tracking_name, PreviousValue::Any).with_deref(true));
                }
                _ => {}
            }
        }
    }
    if edits.is_empty() {
        return Ok(edits);
    }
    repo.edit_references(edits)
        .or_raise(|| message("The remote accepted the push, but its remote-tracking references couldn't be updated"))
}
