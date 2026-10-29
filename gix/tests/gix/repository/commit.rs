//! Committing the index with `gix`, compared with what `git commit` does.

/// The scenarios of the fixtures of committing, resetting and applying merges, in which `gix` changes a copy of a
/// repository as Git changed another copy of it, and the comparison of the two.
#[cfg(feature = "index")]
pub(crate) mod baseline {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use gix::bstr::BString;
    use gix_testtools::tempfile;

    /// The configuration of the identities and dates with which Git acted in the fixtures.
    const IDENTITIES: [&str; 6] = [
        "author.name=author",
        "author.email=author@example.com",
        "committer.name=committer",
        "committer.email=committer@example.com",
        "gitoxide.commit.authorDate=946684800 +0000",
        "gitoxide.commit.committerDate=946771200 +0000",
    ];

    /// The path, stage, mode and id of an entry of the index.
    type Entry = (BString, u32, gix::index::entry::Mode, gix::ObjectId);

    /// A scenario of a fixture: a writable copy of its repository `<name>`, in which `gix` acts, and its repository
    /// `<name>-git`, in which Git acted.
    pub(crate) struct Scenario {
        /// The repository in which `gix` acts.
        pub(crate) repo: gix::Repository,

        /// The repository in which Git acted.
        pub(crate) git: PathBuf,
        /// The directory of `repo`, which goes away with the scenario.
        dir: tempfile::TempDir,
    }

    impl Scenario {
        /// Copy the repository `name` of the fixture `script` for `gix` to act in it.
        pub(crate) fn new(script: &str, name: &str) -> crate::Result<Self> {
            let fixture = gix_testtools::scripted_fixture_read_only(script)?;
            let dir = tempfile::TempDir::new()?;
            gix_testtools::copy_recursively_into_existing_dir(fixture.join(name), dir.path())?;
            Ok(Scenario {
                repo: open(dir.path())?,
                git: fixture.join(format!("{name}-git")),
                dir,
            })
        }

        /// The directory of the repository in which `gix` acts.
        pub(crate) fn path(&self) -> &Path {
            self.dir.path()
        }

        /// Return the lines of the standard error of Git, without the `error: ` or `fatal: ` in front of them and
        /// without hints.
        #[cfg(feature = "reset")]
        pub(crate) fn git_messages(&self) -> crate::Result<Vec<String>> {
            let mut path = self.git.clone().into_os_string();
            path.push(".err");
            Ok(std::fs::read_to_string(path)?
                .lines()
                .filter(|line| !line.starts_with("hint:"))
                .map(|line| {
                    line.strip_prefix("error: ")
                        .or_else(|| line.strip_prefix("fatal: "))
                        .unwrap_or(line)
                        .to_owned()
                })
                .collect())
        }

        /// Assert that `gix` left its repository as Git left its own: with the same references, reflogs, entries of the
        /// index and files of the worktree, and the same `state`, files of the Git directory that are missing in both or
        /// have the same content.
        pub(crate) fn assert_same_as_git(&self, state: &[&str]) -> crate::Result {
            let actual = open(self.path())?;
            let expected = open(&self.git)?;
            assert_eq!(references(&actual)?, references(&expected)?, "references");
            assert_eq!(
                files(&actual.git_dir().join("logs"), None)?,
                files(&expected.git_dir().join("logs"), None)?,
                "reflogs"
            );
            assert_eq!(
                index_entries(&actual)?,
                index_entries(&expected)?,
                "entries of the index"
            );
            if let (Some(actual), Some(expected)) = (actual.workdir(), expected.workdir()) {
                assert_eq!(
                    files(actual, Some(".git"))?,
                    files(expected, Some(".git"))?,
                    "files of the worktree"
                );
            }
            for name in state {
                assert_eq!(
                    read(&actual.git_dir().join(name))?,
                    read(&expected.git_dir().join(name))?,
                    "{name}"
                );
            }
            Ok(())
        }

        /// Assert that the index of `gix` has the cache tree of the index of Git.
        pub(crate) fn assert_same_cache_tree_as_git(&self) -> crate::Result {
            let actual = open(self.path())?.open_index()?;
            let expected = open(&self.git)?.open_index()?;
            assert_eq!(actual.tree(), expected.tree(), "cache tree");
            Ok(())
        }
    }

    /// Open the repository at `path` with the identities and dates of Git in the fixtures.
    pub(crate) fn open(path: &Path) -> crate::Result<gix::Repository> {
        open_with(path, None::<BString>)
    }

    /// Open the repository at `path` with the identities and dates of Git in the fixtures and `config`.
    pub(crate) fn open_with(
        path: &Path,
        config: impl IntoIterator<Item = impl Into<BString>>,
    ) -> crate::Result<gix::Repository> {
        let config: Vec<BString> = IDENTITIES
            .iter()
            .map(|value| BString::from(*value))
            .chain(config.into_iter().map(Into::into))
            .collect();
        Ok(gix::open_opts(
            path,
            gix::open::Options::isolated().config_overrides(config),
        )?)
    }

    /// Return the references of `repo` with their targets.
    fn references(repo: &gix::Repository) -> crate::Result<Vec<(BString, String)>> {
        let mut references = Vec::new();
        for reference in repo.references()?.all()? {
            let reference = reference?;
            let target = match reference.target() {
                gix::refs::TargetRef::Object(id) => id.to_string(),
                gix::refs::TargetRef::Symbolic(name) => format!("ref: {}", name.as_bstr()),
            };
            references.push((reference.name().as_bstr().to_owned(), target));
        }
        Ok(references)
    }

    /// Return the entries of the index of `repo`, of which there are none without index file.
    fn index_entries(repo: &gix::Repository) -> crate::Result<Vec<Entry>> {
        if !repo.index_path().exists() {
            return Ok(Vec::new());
        }
        let index = repo.open_index()?;
        Ok(index
            .entries()
            .iter()
            .map(|entry| (entry.path(&index).to_owned(), entry.stage_raw(), entry.mode, entry.id))
            .collect())
    }

    /// Return the content of the files below `root` by their path relative to it, without the directory `skip` there,
    /// or nothing if `root` doesn't exist.
    fn files(root: &Path, skip: Option<&str>) -> crate::Result<BTreeMap<String, BString>> {
        let mut files = BTreeMap::new();
        if !root.is_dir() {
            return Ok(files);
        }
        let mut directories = vec![root.to_owned()];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                let relative = path.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
                if skip == Some(relative.as_str()) {
                    continue;
                }
                if entry.file_type()?.is_dir() {
                    directories.push(path);
                } else {
                    files.insert(relative, std::fs::read(&path)?.into());
                }
            }
        }
        Ok(files)
    }

    /// Return the content of the file at `path`, or `None` if it doesn't exist.
    fn read(path: &Path) -> crate::Result<Option<BString>> {
        match std::fs::read(path) {
            Ok(content) => Ok(Some(content.into())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(feature = "index")]
mod commit_index {
    use gix::{
        bstr::{BString, ByteSlice},
        commit::{
            index::{Error, Options, Outcome},
            message::{Cleanup, CommentPrefix},
        },
    };

    use super::baseline::{Scenario, open, open_with};

    /// Return the files of the Git directory that a commit writes or removes, which `gix` has to leave as Git does,
    /// with the records of the hooks if they run.
    fn state() -> Vec<&'static str> {
        let mut state = vec![
            "HEAD",
            "COMMIT_EDITMSG",
            "MERGE_HEAD",
            "MERGE_MSG",
            "MERGE_MODE",
            "SQUASH_MSG",
            "AUTO_MERGE",
            "ORIG_HEAD",
        ];
        if cfg!(feature = "hooks") {
            state.push("hook-records");
        }
        state
    }

    fn scenario(name: &str) -> crate::Result<Scenario> {
        Scenario::new("make_commit_repos.sh", name)
    }

    /// Return the options of a commit with `message`, as `-m` gives it.
    fn message(message: &str) -> Options {
        Options {
            message: Some(message.into()),
            ..Default::default()
        }
    }

    /// Return the message of the scenarios of cleanups.
    fn cleanup_message() -> crate::Result<BString> {
        Ok(std::fs::read(gix_testtools::scripted_fixture_read_only("make_commit_repos.sh")?.join("message"))?.into())
    }

    /// Commit in the scenario `name` with `options`, assert that `gix` made the commit of Git and left the repository as
    /// Git did, and return what it did.
    fn commits_as_git(name: &str, options: Options) -> crate::Result<Outcome> {
        let scenario = scenario(name)?;
        let outcome = scenario.repo.commit_index(options)?;
        assert_eq!(
            outcome.commit_id,
            open(&scenario.git)?.head_id()?.detach(),
            "{name}: the commit of Git"
        );
        scenario.assert_same_as_git(&state())?;
        scenario.assert_same_cache_tree_as_git()?;
        Ok(outcome)
    }

    /// Commit in the scenario `name` with `options`, assert that `gix` refused as Git did and left the repository as Git
    /// did, and return why it refused.
    fn refuses_as_git(name: &str, options: Options) -> crate::Result<Error> {
        let scenario = scenario(name)?;
        let err = scenario
            .repo
            .commit_index(options)
            .expect_err("Git refused to commit as well");
        scenario.assert_same_as_git(&state())?;
        scenario.assert_same_cache_tree_as_git()?;
        Ok(err
            .downcast_any_ref::<Error>()
            .cloned()
            .unwrap_or_else(|| panic!("{name}: a refusal to commit, not {err:?}")))
    }

    #[test]
    fn initial_commit() -> crate::Result {
        let outcome = commits_as_git("initial", message("initial"))?;
        assert!(outcome.parents.is_empty(), "the first commit has no parent");
        assert_eq!(outcome.previous, None);
        assert_eq!(outcome.reference.as_bstr(), "refs/heads/main");
        assert_eq!(
            outcome.message, "initial\n",
            "the message ends with a newline, as Git ends it"
        );
        Ok(())
    }

    #[test]
    fn subsequent_commit() -> crate::Result {
        let outcome = commits_as_git("subsequent", message("subsequent\n\nwith a body"))?;
        assert_eq!(outcome.parents.len(), 1);
        assert_eq!(outcome.previous, outcome.parents.first().copied());
        Ok(())
    }

    #[test]
    fn detached_head() -> crate::Result {
        let outcome = commits_as_git("detached", message("detached"))?;
        assert_eq!(outcome.reference.as_bstr(), "HEAD", "the detached HEAD moves");
        Ok(())
    }

    #[test]
    fn reflog_action() -> crate::Result {
        commits_as_git(
            "reflog-action",
            Options {
                reflog_action: Some("custom".into()),
                ..message("custom action")
            },
        )?;
        Ok(())
    }

    #[test]
    fn cleanup_modes() -> crate::Result {
        for (cleanup, name) in [
            (Cleanup::Default, "cleanup-default"),
            (Cleanup::Strip, "cleanup-strip"),
            (Cleanup::Whitespace, "cleanup-whitespace"),
            (Cleanup::Verbatim, "cleanup-verbatim"),
            (Cleanup::Scissors, "cleanup-scissors"),
        ] {
            let outcome = commits_as_git(
                name,
                Options {
                    message: Some(cleanup_message()?),
                    cleanup: Some(cleanup),
                    ..Default::default()
                },
            )?;
            assert_eq!(
                outcome.message.contains_str("# hash comment"),
                cleanup != Cleanup::Strip,
                "{name}: without editor, only `strip` removes comments"
            );
        }
        Ok(())
    }

    #[test]
    fn cleanup_and_comment_prefix_from_configuration() -> crate::Result {
        for (name, kept) in [
            ("cleanup-config", "; semicolon comment"),
            ("comment-char", "# hash comment"),
            ("comment-string", "; semicolon comment"),
            ("comment-auto", "# hash comment"),
            ("comment-last", "// slash comment"),
        ] {
            let outcome = commits_as_git(
                name,
                Options {
                    message: Some(cleanup_message()?),
                    ..Default::default()
                },
            )?;
            assert!(outcome.message.contains_str(kept), "{name}: {kept:?} isn't a comment");
        }
        Ok(())
    }

    #[test]
    fn comment_prefix_and_cleanup_are_configured() -> crate::Result {
        let repo = scenario("comment-last")?.repo;
        assert_eq!(
            repo.comment_prefix()?,
            CommentPrefix::Fixed(";".into()),
            "the last of core.commentChar and core.commentString wins"
        );
        assert_eq!(repo.commit_cleanup()?, Cleanup::Strip);
        assert_eq!(scenario("comment-auto")?.repo.comment_prefix()?, CommentPrefix::Auto);
        let repo = scenario("initial")?.repo;
        assert_eq!(repo.comment_prefix()?, CommentPrefix::Fixed("#".into()));
        assert_eq!(repo.commit_cleanup()?, Cleanup::Default);
        Ok(())
    }

    #[test]
    fn configuration_keys() -> crate::Result {
        use gix::config::tree::{Commit, Core, Key};

        for (value, cleanup) in [
            ("default", Cleanup::Default),
            ("verbatim", Cleanup::Verbatim),
            ("whitespace", Cleanup::Whitespace),
            ("strip", Cleanup::Strip),
            ("scissors", Cleanup::Scissors),
        ] {
            assert_eq!(Commit::CLEANUP.try_into_cleanup(value)?, cleanup);
        }
        assert!(
            Commit::CLEANUP.validate("Strip".into()).is_err(),
            "modes are case-sensitive, as in Git"
        );
        assert_eq!(
            Core::COMMENT_CHAR.try_into_comment_prefix(Some("AUTO"))?,
            CommentPrefix::Auto
        );
        assert_eq!(
            Core::COMMENT_STRING.try_into_comment_prefix(Some("//"))?,
            CommentPrefix::Fixed("//".into())
        );
        assert!(
            Core::COMMENT_CHAR.validate("".into()).is_err(),
            "a prefix can't be empty"
        );
        assert!(
            Core::COMMENT_CHAR.validate("a\nb".into()).is_err(),
            "a prefix can't have a newline"
        );
        assert!(
            Core::COMMENT_CHAR.try_into_comment_prefix(None::<&str>).is_err(),
            "a key without value isn't a prefix"
        );
        Ok(())
    }

    #[test]
    fn amend_keeps_author_and_message() -> crate::Result {
        let outcome = commits_as_git(
            "amend",
            Options {
                amend: true,
                ..Default::default()
            },
        )?;
        assert_eq!(outcome.message, "to be amended\n\nwith a body\n");
        Ok(())
    }

    #[test]
    fn amend_with_message() -> crate::Result {
        commits_as_git(
            "amend-message",
            Options {
                amend: true,
                ..message("amended message")
            },
        )?;
        Ok(())
    }

    #[test]
    #[cfg(feature = "merge")]
    fn merge_commit() -> crate::Result {
        let outcome = commits_as_git("merge", Options::default())?;
        assert_eq!(outcome.parents.len(), 2, "HEAD and MERGE_HEAD");
        Ok(())
    }

    #[test]
    #[cfg(feature = "merge")]
    fn merge_commit_after_resolved_conflict() -> crate::Result {
        commits_as_git("merge-resolved", Options::default())?;
        Ok(())
    }

    #[test]
    fn squash_message() -> crate::Result {
        let outcome = commits_as_git("squash", Options::default())?;
        assert_eq!(outcome.parents.len(), 1, "a squash has no other parent");
        Ok(())
    }

    #[test]
    #[cfg(feature = "hooks")]
    fn failing_hooks_stop_the_commit() -> crate::Result {
        for event in ["pre-commit", "prepare-commit-msg", "commit-msg"] {
            assert_eq!(
                refuses_as_git(&format!("{event}-fails"), message(&format!("{event} fails")))?,
                Error::HookFailed { event, code: 1 }
            );
        }
        Ok(())
    }

    #[test]
    #[cfg(feature = "hooks")]
    fn failing_post_commit_is_ignored() -> crate::Result {
        commits_as_git("post-commit-fails", message("post-commit fails"))?;
        Ok(())
    }

    #[test]
    #[cfg(feature = "hooks")]
    fn commit_msg_changes_the_message() -> crate::Result {
        let outcome = commits_as_git("hook-edits-message", message("edited by commit-msg"))?;
        assert_eq!(outcome.message, "edited by commit-msg\n\nHook-Trailer: commit-msg\n");
        Ok(())
    }

    #[test]
    #[cfg(not(feature = "hooks"))]
    fn hooks_do_not_run_without_the_hooks_feature() -> crate::Result {
        let scenario = scenario("pre-commit-fails")?;
        scenario.repo.commit_index(message("pre-commit fails"))?;
        assert!(
            !scenario.repo.git_dir().join("hook-records").exists(),
            "no hook ran, not even the failing one"
        );
        Ok(())
    }

    #[test]
    fn no_verify_skips_pre_commit_and_commit_msg() -> crate::Result {
        commits_as_git(
            "no-verify",
            Options {
                no_verify: true,
                ..message("no verify")
            },
        )?;
        Ok(())
    }

    #[test]
    fn nothing_to_commit() -> crate::Result {
        assert_eq!(
            refuses_as_git("nothing-to-commit", message("nothing"))?,
            Error::NothingToCommit
        );
        Ok(())
    }

    #[test]
    fn allow_empty() -> crate::Result {
        let outcome = commits_as_git(
            "allow-empty",
            Options {
                allow_empty: true,
                ..message("empty")
            },
        )?;
        let repo = open(&gix_testtools::scripted_fixture_read_only("make_commit_repos.sh")?.join("allow-empty-git"))?;
        assert_eq!(
            outcome.tree_id,
            repo.find_commit(outcome.parents[0])?.tree_id()?.detach(),
            "the commit records the tree of its parent"
        );
        Ok(())
    }

    #[test]
    fn empty_message() -> crate::Result {
        assert_eq!(refuses_as_git("empty-message", message(""))?, Error::EmptyMessage);
        Ok(())
    }

    #[test]
    fn allow_empty_message() -> crate::Result {
        let outcome = commits_as_git(
            "allow-empty-message",
            Options {
                allow_empty_message: true,
                ..message("")
            },
        )?;
        assert_eq!(outcome.message, "");
        Ok(())
    }

    #[test]
    #[cfg(feature = "merge")]
    fn unmerged_entries() -> crate::Result {
        assert_eq!(refuses_as_git("unmerged", Options::default())?, Error::UnmergedEntries);
        Ok(())
    }

    #[test]
    fn nothing_to_amend() -> crate::Result {
        assert_eq!(
            refuses_as_git(
                "nothing-to-amend",
                Options {
                    amend: true,
                    ..message("amended")
                }
            )?,
            Error::NothingToAmend
        );
        Ok(())
    }

    #[test]
    #[cfg(feature = "merge")]
    fn amend_in_merge() -> crate::Result {
        assert_eq!(
            refuses_as_git(
                "amend-in-merge",
                Options {
                    amend: true,
                    ..Default::default()
                }
            )?,
            Error::AmendInMerge
        );
        Ok(())
    }

    #[test]
    fn bare_repositories_are_refused() -> crate::Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        let repo = crate::util::init_repo_isolated(dir.path(), gix::create::Kind::Bare)?.to_thread_local();
        let err = repo.commit_index(message("bare")).expect_err("there is no worktree");
        assert_eq!(err.downcast_any_ref::<Error>(), Some(&Error::BareRepository));
        assert!(err.is_validation());
        Ok(())
    }

    #[test]
    #[cfg(feature = "command")]
    fn signs_as_configured() -> crate::Result {
        use gix::config::tree::{Commit, Gpg, Key, User, gpg};
        use gix_testtools::signature;

        let (_key_home, key) = signature::ssh_private_key()?;
        let scenario = scenario("subsequent")?;
        let repo = open_with(
            scenario.path(),
            [
                Commit::GPG_SIGN.validated_assignment_fmt(&"true")?,
                Gpg::FORMAT.validated_assignment_fmt(&"ssh")?,
                User::SIGNING_KEY.validated_assignment_fmt(&key.display())?,
                gpg::Ssh::ALLOWED_SIGNERS_FILE
                    .validated_assignment_fmt(&signature::fixture("ssh-allowed-signers").display())?,
            ],
        )?;
        if !signature::program_available(&repo.commit_signing_options()?.program) {
            return Ok(());
        }
        let outcome = repo.commit_index(message("subsequent\n\nwith a body"))?;
        let commit = repo.find_commit(outcome.commit_id)?;
        assert!(
            commit.verify_signature()?.expect("the commit is signed").is_valid(),
            "commit.gpgSign signs the commit with the configured key"
        );
        // The signature is in `gpgsig`, or in `gpgsig-sha256` in repositories of SHA-256, as Git writes it.
        let mut unsigned = Vec::new();
        let mut in_signature = false;
        let mut in_headers = true;
        for line in commit.data.lines_with_terminator() {
            if in_headers {
                if line == b"\n" {
                    in_headers = false;
                } else if line.starts_with(b"gpgsig ")
                    || line.starts_with(b"gpgsig-sha256 ")
                    || (in_signature && line.starts_with(b" "))
                {
                    in_signature = true;
                    continue;
                } else {
                    in_signature = false;
                }
            }
            unsigned.extend_from_slice(line);
        }
        assert_eq!(
            gix_object::compute_hash(repo.object_hash(), gix_object::Kind::Commit, &unsigned)?,
            open(&scenario.git)?.head_id()?.detach(),
            "without its signature, the commit is the one of Git"
        );
        Ok(())
    }
}
