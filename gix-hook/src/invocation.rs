//! What Git passes to the hooks of an event, with a constructor for each client-side hook Git runs.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use bstr::{BStr, ByteSlice};

/// What Git passes to the hooks of an event: their arguments, environment variables and standard input, where their
/// standard output goes and the directory they run in.
///
/// All hooks of the event receive the same. The constructors for the client-side hooks pass what Git passes, and their
/// documentation says what Git does when the hook fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The event whose hooks run, like `pre-commit`.
    pub event: &'a str,
    /// The arguments of the hooks.
    pub args: Vec<OsString>,
    /// Environment variables the hooks receive in addition to those of this process and those Git exports for the
    /// repository, each with its value, or `None` to remove it.
    pub env: Vec<(OsString, Option<OsString>)>,
    /// The standard input of the hooks.
    pub stdin: Stdin<'a>,
    /// Where the standard output of the hooks goes. Their standard error is that of this process.
    pub stdout: Stdout,
    /// The directory the hooks run in, or `None` for the [directory hooks usually run in](crate::Hooks::working_dir()).
    pub working_dir: Option<&'a Path>,
}

/// The standard input of hooks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Stdin<'a> {
    /// Nothing: the null device, as for hooks Git gives no input.
    #[default]
    Null,
    /// These bytes, written to each hook. A hook that doesn't read all of them doesn't fail because of it.
    Bytes(Vec<u8>),
    /// The content of this file, opened for each hook.
    File(&'a Path),
}

/// Where the standard output of hooks goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Stdout {
    /// To the standard error of this process, where Git sends the standard output of all hooks but `pre-push`.
    #[default]
    Stderr,
    /// To the standard output of this process, as Git keeps it for `pre-push`.
    Inherit,
}

/// Whether an editor is started for the message of a commit, which Git tells the hooks of the commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    /// An editor is started for the message.
    Used,
    /// No editor is started, and the hooks receive `GIT_EDITOR=:`, so that the commands they run start none either.
    NotUsed,
}

/// What Git tells the hooks of a commit in their environment, as its `run_commit_hook()` does for `pre-commit`,
/// `prepare-commit-msg`, `commit-msg`, `post-commit` and `pre-merge-commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit<'a> {
    /// The index the commit is made from, which hooks receive as `GIT_INDEX_FILE`.
    ///
    /// `git commit` passes the index of the repository, unless it commits the index it prepared for `--all` or
    /// `--include`, `$GIT_DIR/index.lock`, or for paths given to it, a temporary `$GIT_DIR/next-index-<pid>.lock`.
    /// `post-commit`, `git merge` and the commits of `git rebase` receive the index of the repository. Git names it
    /// relative to the root of the worktree if the Git directory is the `.git` directory there, as `.git/index`.
    pub index_file: &'a Path,
    /// Whether an editor is started for the message.
    pub editor: Editor,
}

impl Commit<'_> {
    fn env(&self) -> Vec<(OsString, Option<OsString>)> {
        let mut env = vec![("GIT_INDEX_FILE".into(), Some(self.index_file.into()))];
        if self.editor == Editor::NotUsed {
            env.push(("GIT_EDITOR".into(), Some(":".into())));
        }
        env
    }
}

/// Where the message of a commit comes from, which `prepare-commit-msg` receives as its second and third argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageSource<'a> {
    /// `message`: the message was given with `-m` or `-F`.
    Message,
    /// `message` and an empty argument: `--squash=<commit>` put the subject for squashing that commit in front of
    /// the message, if other options gave one.
    SquashSubject,
    /// `template`: the message comes from the file `-t` or `commit.template` names.
    Template,
    /// `merge`: the commit is a merge, or `$GIT_DIR/MERGE_MSG` exists.
    Merge,
    /// `squash`: `$GIT_DIR/SQUASH_MSG` exists.
    Squash,
    /// `commit` and the commit as given: the message of a commit is reused, with `-c` or `-C`, or with `--amend`, which
    /// passes `HEAD`. A cherry-pick being committed passes `CHERRY_PICK_HEAD`.
    Commit(&'a OsStr),
}

/// The command that rewrote commits, which `post-rewrite` receives as argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rewrite {
    /// `amend`: `git commit --amend`.
    Amend,
    /// `rebase`: `git rebase`.
    Rebase,
}

/// What was checked out, which `post-checkout` receives as its third argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkout {
    /// `1`: a branch or a commit was checked out, which may change `HEAD`, as by `git switch`, `git clone`,
    /// `git worktree add` and `git rebase`.
    Branch,
    /// `0`: files were checked out, as by `git checkout -- <paths>`, which leaves `HEAD`.
    Files,
}

/// The state of a reference transaction, which `reference-transaction` receives as argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// `preparing`: all updates are queued, and no reference is locked yet.
    Preparing,
    /// `prepared`: all updates are queued, and the references are locked.
    Prepared,
    /// `committed`: all references have their new values.
    Committed,
    /// `aborted`: nothing changed, and the locks are released.
    Aborted,
}

/// The old or new value of a reference in a reference transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefValue<'a> {
    /// An object, written as its hexadecimal name. The null object stands for a reference that doesn't exist, or
    /// whose value isn't checked.
    Object(&'a gix_hash::oid),
    /// A symbolic reference to the reference of this name, written as `ref:<name>`.
    Symbolic(&'a BStr),
}

/// One update of a reference transaction, a line of the standard input of `reference-transaction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefUpdate<'a> {
    /// The value the transaction expects the reference to have, or the null object if it doesn't check it, as when
    /// forcing an update or creating the reference.
    pub old: RefValue<'a>,
    /// The value the reference gets, or the null object if it's deleted.
    pub new: RefValue<'a>,
    /// The full name of the reference.
    pub name: &'a BStr,
}

/// One reference a push updates, a line of the standard input of `pre-push`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushUpdate<'a> {
    /// The local reference, `(delete)` if the remote reference is deleted, or what was given if it doesn't name a
    /// reference, like `HEAD~`.
    pub local_ref: &'a BStr,
    /// The object pushed, or the null object if the remote reference is deleted.
    pub local_object: &'a gix_hash::oid,
    /// The full name of the remote reference.
    pub remote_ref: &'a BStr,
    /// The object the remote reference has, or the null object if it doesn't exist.
    pub remote_object: &'a gix_hash::oid,
}

impl<'a> Invocation<'a> {
    /// Prepare to run the hooks of `event` without arguments, environment variables or input, with their standard
    /// output sent to the standard error of this process.
    pub fn new(event: &'a str) -> Self {
        Invocation {
            event,
            args: Vec::new(),
            env: Vec::new(),
            stdin: Stdin::Null,
            stdout: Stdout::Stderr,
            working_dir: None,
        }
    }

    /// `pre-commit`, which `git commit` runs before it obtains the message and makes the commit, without arguments.
    ///
    /// If it fails, Git aborts the commit. `git commit --no-verify` doesn't run it.
    pub fn pre_commit(commit: Commit<'_>) -> Self {
        Invocation {
            env: commit.env(),
            ..Invocation::new("pre-commit")
        }
    }

    /// `prepare-commit-msg`, which `git commit` runs after it prepared the message in `message_file` and before it
    /// starts the editor, with `message_file` and the `source` of the message, if any, as arguments.
    ///
    /// `git commit` passes `$GIT_DIR/COMMIT_EDITMSG`, and `git merge` passes `$GIT_DIR/MERGE_MSG` with
    /// [`MessageSource::Merge`]. The hook may change the message in the file. If it fails, Git aborts the commit, and
    /// `--no-verify` doesn't skip it.
    pub fn prepare_commit_msg(commit: Commit<'_>, message_file: &Path, source: Option<MessageSource<'_>>) -> Self {
        let mut args: Vec<OsString> = vec![message_file.into()];
        match source {
            None => {}
            Some(MessageSource::Message) => args.push("message".into()),
            Some(MessageSource::SquashSubject) => args.extend(["message".into(), OsString::new()]),
            Some(MessageSource::Template) => args.push("template".into()),
            Some(MessageSource::Merge) => args.push("merge".into()),
            Some(MessageSource::Squash) => args.push("squash".into()),
            Some(MessageSource::Commit(revision)) => args.extend(["commit".into(), revision.to_owned()]),
        }
        Invocation {
            args,
            env: commit.env(),
            ..Invocation::new("prepare-commit-msg")
        }
    }

    /// `commit-msg`, which `git commit` and `git merge` run with the file of the message, `message_file`, as
    /// argument, after the editor, if any, was closed.
    ///
    /// The hook may change the message in the file. If it fails, Git aborts the commit. `--no-verify` doesn't run it.
    pub fn commit_msg(commit: Commit<'_>, message_file: &Path) -> Self {
        Invocation {
            args: vec![message_file.into()],
            env: commit.env(),
            ..Invocation::new("commit-msg")
        }
    }

    /// `post-commit`, which `git commit` runs after it made the commit, without arguments, and `git rebase` after
    /// each commit it makes.
    ///
    /// The hook is meant for notification, and Git ignores whether it fails.
    pub fn post_commit(commit: Commit<'_>) -> Self {
        Invocation {
            env: commit.env(),
            ..Invocation::new("post-commit")
        }
    }

    /// `pre-merge-commit`, which `git merge` runs after it merged without conflicts and before it obtains the message
    /// of the merge commit, without arguments.
    ///
    /// If it fails, Git aborts the merge before it makes the commit. `git merge --no-verify` doesn't run it. When
    /// conflicts were resolved and the merge is committed with `git commit`, that runs `pre-commit` instead.
    ///
    /// For all of its hooks, `git merge` exports `GIT_REFLOG_ACTION` as `merge <name>...` with the names of the
    /// merged commits as given, unless it's set, and `GITHEAD_<object>=<name>` for each of them. Add them to
    /// [`Invocation::env`] to pass them as well.
    pub fn pre_merge_commit(commit: Commit<'_>) -> Self {
        Invocation {
            env: commit.env(),
            ..Invocation::new("pre-merge-commit")
        }
    }

    /// `post-merge`, which `git merge` runs after a merge, also for `git pull`, with `1` as argument if it was a squash
    /// merge, and `0` otherwise.
    ///
    /// Git ignores whether it fails, and doesn't run it if the merge stopped because of conflicts. The variables
    /// `git merge` exports are listed for [`Invocation::pre_merge_commit()`].
    pub fn post_merge(squash: bool) -> Self {
        Invocation {
            args: vec![if squash { "1" } else { "0" }.into()],
            ..Invocation::new("post-merge")
        }
    }

    /// `pre-rebase`, which `git rebase` runs before it rebases, with the `upstream` the branch is rebased onto as given,
    /// or `--root` when the root commits are rebased, and the `branch` that is rebased, if it was given.
    ///
    /// If it fails, Git doesn't rebase ("The pre-rebase hook refused to rebase."). `git rebase --no-verify` doesn't
    /// run it.
    pub fn pre_rebase(upstream: &OsStr, branch: Option<&OsStr>) -> Self {
        Invocation {
            args: std::iter::once(upstream).chain(branch).map(ToOwned::to_owned).collect(),
            ..Invocation::new("pre-rebase")
        }
    }

    /// `post-rewrite`, which runs after `git commit --amend` and `git rebase` rewrote commits, with the `command` that
    /// rewrote them as argument, and each `rewritten` commit with the commit it became on standard input, as
    /// `<old-object> <new-object>` lines.
    ///
    /// `git rebase` lists commits in the order it processed them, and the commits it squashed or fixed up into
    /// another with the commit they became together. Git ignores whether the hook fails.
    pub fn post_rewrite<'b>(
        command: Rewrite,
        rewritten: impl IntoIterator<Item = (&'b gix_hash::oid, &'b gix_hash::oid)>,
    ) -> Self {
        let mut stdin = Vec::new();
        for (old, new) in rewritten {
            push_line(&mut stdin, &[hex(old).as_bytes(), hex(new).as_bytes()]);
        }
        Invocation {
            args: vec![
                match command {
                    Rewrite::Amend => "amend",
                    Rewrite::Rebase => "rebase",
                }
                .into(),
            ],
            stdin: Stdin::Bytes(stdin),
            ..Invocation::new("post-rewrite")
        }
    }

    /// `post-checkout`, which runs after `git checkout` or `git switch` updated the worktree, with the `previous` and
    /// the `new` commit of `HEAD`, which may be the same, and what was checked out as arguments.
    ///
    /// `git rebase` runs it when it checks out the commit it starts from. The exit status of the hook becomes the exit
    /// status of the command, but what was checked out stays. For `git clone` and `git worktree add`, see
    /// [`Invocation::post_checkout_after_clone()`] and [`Invocation::post_checkout_in_new_worktree()`].
    pub fn post_checkout(previous: &gix_hash::oid, new: &gix_hash::oid, checkout: Checkout) -> Self {
        Invocation {
            args: vec![
                hex(previous).into(),
                hex(new).into(),
                match checkout {
                    Checkout::Branch => "1",
                    Checkout::Files => "0",
                }
                .into(),
            ],
            ..Invocation::new("post-checkout")
        }
    }

    /// `post-checkout` as `git clone` runs it after it checked out `new` in the worktree of the new repository, unless
    /// `--no-checkout` is given.
    ///
    /// The hook runs in the worktree of the new repository, with the null object as previous commit, with `GIT_DIR` set
    /// to `git_dir`, for which Git passes the real path of the new Git directory, written with slashes on Windows, and
    /// without `GIT_PREFIX`, which `git clone` doesn't export. Its exit status becomes the exit status of `git clone`,
    /// which keeps the clone.
    pub fn post_checkout_after_clone(new: &gix_hash::oid, git_dir: &Path) -> Self {
        Invocation {
            env: vec![("GIT_DIR".into(), Some(git_dir.into())), ("GIT_PREFIX".into(), None)],
            ..Invocation::post_checkout(&new.kind().null(), new, Checkout::Branch)
        }
    }

    /// `post-checkout` as `git worktree add` runs it after it checked out `new` in the new linked worktree at
    /// `worktree`, unless `--no-checkout` is given.
    ///
    /// The hook runs in `worktree`, with the null object as previous commit, and without `GIT_DIR` and
    /// `GIT_WORK_TREE`, so that Git commands it runs find the new worktree. Its exit status becomes the exit status of
    /// `git worktree add`, which keeps the worktree.
    pub fn post_checkout_in_new_worktree(new: &gix_hash::oid, worktree: &'a Path) -> Self {
        Invocation {
            env: vec![("GIT_DIR".into(), None), ("GIT_WORK_TREE".into(), None)],
            working_dir: Some(worktree),
            ..Invocation::post_checkout(&new.kind().null(), new, Checkout::Branch)
        }
    }

    /// `pre-push`, which `git push` runs before it pushes anything, with the name of the `remote` and its `url` as
    /// arguments, or the URL twice if no named remote is used, and on standard input each reference it updates as
    /// `<local-ref> <local-object> <remote-ref> <remote-object>` lines.
    ///
    /// Git leaves out the references it doesn't push: those that are up to date, and those it already rejected. The
    /// standard output of the hook is that of Git, unlike that of other hooks. If the hook fails, Git pushes nothing.
    /// `git push --no-verify` doesn't run it.
    pub fn pre_push<'b>(remote: &OsStr, url: &OsStr, updates: impl IntoIterator<Item = PushUpdate<'b>>) -> Self {
        let mut stdin = Vec::new();
        for update in updates {
            push_line(
                &mut stdin,
                &[
                    update.local_ref.as_bytes(),
                    hex(update.local_object).as_bytes(),
                    update.remote_ref.as_bytes(),
                    hex(update.remote_object).as_bytes(),
                ],
            );
        }
        Invocation {
            args: vec![remote.to_owned(), url.to_owned()],
            stdin: Stdin::Bytes(stdin),
            stdout: Stdout::Inherit,
            ..Invocation::new("pre-push")
        }
    }

    /// `reference-transaction`, which Git runs whenever a reference transaction reaches the given `state`, with the
    /// state as argument and on standard input each reference it updates as `<old-value> <new-value> <ref-name>`
    /// lines.
    ///
    /// Git leaves out updates that only write the reflog. If the hook fails while the transaction is
    /// [preparing](TransactionState::Preparing) or [prepared](TransactionState::Prepared), Git aborts the
    /// transaction, which runs the hook as [aborted](TransactionState::Aborted), and stops the command. Otherwise Git
    /// ignores whether it fails. Git doesn't run it as [committed](TransactionState::Committed) for the transaction
    /// that writes the references of a new clone. When the `files` backend of Git deletes references, it also runs the
    /// hook as aborted between preparing and prepared, for the transaction of `packed-refs` it doesn't need if none of
    /// them is packed, with the deletions as updates of the null object to the null object.
    pub fn reference_transaction<'b>(
        state: TransactionState,
        updates: impl IntoIterator<Item = RefUpdate<'b>>,
    ) -> Self {
        let mut stdin = Vec::new();
        for update in updates {
            push_line(
                &mut stdin,
                &[
                    value(update.old).as_slice(),
                    value(update.new).as_slice(),
                    update.name.as_bytes(),
                ],
            );
        }
        Invocation {
            args: vec![
                match state {
                    TransactionState::Preparing => "preparing",
                    TransactionState::Prepared => "prepared",
                    TransactionState::Committed => "committed",
                    TransactionState::Aborted => "aborted",
                }
                .into(),
            ],
            stdin: Stdin::Bytes(stdin),
            ..Invocation::new("reference-transaction")
        }
    }
}

fn hex(id: &gix_hash::oid) -> String {
    id.to_hex().to_string()
}

fn value(value: RefValue<'_>) -> Vec<u8> {
    match value {
        RefValue::Object(id) => hex(id).into_bytes(),
        RefValue::Symbolic(name) => [b"ref:".as_slice(), name.as_bytes()].concat(),
    }
}

/// Append `fields` to `out`, separated by spaces and ended by a newline, as the lines Git writes to hooks.
fn push_line(out: &mut Vec<u8>, fields: &[&[u8]]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(field);
    }
    out.push(b'\n');
}
