//! Pushing with `gix`, compared with what `git push` does in the cases of `make_push_repos.sh`.

/// Choosing the remote to push to as `git push` chooses it.
mod find_push_remote {
    use gix::{bstr::BStr, remote::Direction::Push};

    use crate::util::{init_repo_isolated, named_subrepo_opts, restricted};

    /// The repository `local` of `case` in `make_push_repos.sh`.
    fn local(case: &str) -> gix_testtools::Result<gix::Repository> {
        Ok(named_subrepo_opts(
            "make_push_repos.sh",
            &format!("{case}/before/local"),
            restricted(),
        )?)
    }

    #[test]
    fn without_name_uses_the_configured_remote() -> gix_testtools::Result {
        for (case, expected) in [
            ("fast-forward", "origin"),
            ("detached", "origin"),
            ("push-default-remote", "pub"),
            ("branch-push-remote", "pub"),
        ] {
            let repo = local(case)?;
            let remote = repo.find_push_remote(None)?;
            assert_eq!(
                remote.name().map(gix::remote::Name::as_bstr),
                Some(BStr::new(expected)),
                "{case}: `branch.<name>.pushRemote` and `remote.pushDefault` come before `branch.<name>.remote` and the only remote"
            );
        }
        Ok(())
    }

    #[test]
    fn with_name_uses_that_remote_or_url() -> gix_testtools::Result {
        let repo = local("url-remote")?;
        let remote = repo.find_push_remote(Some("origin".into()))?;
        assert_eq!(
            remote.name().map(gix::remote::Name::as_bstr),
            Some(BStr::new("origin")),
            "a configured remote is found by its name"
        );

        let remote = repo.find_push_remote(Some("../remote.git".into()))?;
        assert!(remote.name().is_none(), "a URL makes a remote without name");
        assert_eq!(
            remote.url(Push).map(gix::Url::to_bstring),
            Some("../remote.git".into()),
            "the remote pushes to the URL"
        );
        Ok(())
    }

    #[test]
    fn fails_without_remote() -> gix_testtools::Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        let repo = init_repo_isolated(dir.path(), gix::create::Kind::WithWorktree)?.to_thread_local();
        let Err(err) = repo.find_push_remote(None) else {
            panic!("a repository without remote has nothing to push to");
        };
        assert!(
            err.to_string().starts_with("No configured push destination."),
            "it fails with the message of `git push`: {err}"
        );
        Ok(())
    }
}

#[cfg(any(feature = "blocking-network-client", feature = "async-network-client-async-std"))]
mod blocking_and_async_io {
    use std::{path::Path, sync::atomic::AtomicBool};

    use gix::{
        bstr::{BString, ByteSlice},
        config::tree::Url,
        remote::{Direction::Push, push},
    };
    use gix_protocol::bisync;
    use gix_testtools::{GitDaemon, Result};

    use crate::{remote::spawn_git_daemon_if_async, util::restricted};

    /// What pushing a case of `make_push_repos.sh` with `gix` has to do.
    enum Expect {
        /// Report on the references and change the repositories as `git push` did.
        AsGit,
        /// Fail with an error containing `gix` and change nothing, where `git push` failed with `git` in its output before
        /// it reported on any reference.
        Fail { git: &'static str, gix: &'static str },
    }
    use Expect::{AsGit, Fail};

    /// Fail like `git push`, with the same message.
    const fn fail(message: &'static str) -> Expect {
        Fail {
            git: message,
            gix: message,
        }
    }

    /// The cases of `make_push_repos.sh` and what pushing them with `gix` has to do.
    const CASES: &[(&str, Expect)] = &[
        ("fast-forward", AsGit),
        ("new-branch", AsGit),
        ("fetch-first", AsGit),
        ("non-fast-forward", AsGit),
        ("force", AsGit),
        ("force-refspec", AsGit),
        ("delete", AsGit),
        ("delete-refspec", AsGit),
        (
            "delete-with-tags",
            fail("options '--delete' and '--tags' cannot be used together"),
        ),
        ("tags", AsGit),
        ("tag-shorthand", AsGit),
        ("tag-exists", AsGit),
        ("tag-force", AsGit),
        ("follow-tags", AsGit),
        ("follow-tags-config", AsGit),
        ("lease", AsGit),
        ("lease-stale", AsGit),
        ("lease-stale-forced", AsGit),
        ("lease-value", AsGit),
        ("lease-value-stale", AsGit),
        ("lease-new", AsGit),
        ("up-to-date", AsGit),
        ("atomic", AsGit),
        ("atomic-rejected", AsGit),
        (
            "atomic-unsupported",
            fail("the receiving end does not support --atomic push"),
        ),
        ("default-simple", AsGit),
        (
            "default-simple-no-upstream",
            fail(
                "The current branch feature has no upstream branch.\nTo push the current branch and set the remote as \
                 upstream, use\n\n    git push --set-upstream origin feature",
            ),
        ),
        (
            "default-simple-mismatch",
            fail(
                "The upstream branch of your current branch does not match\nthe name of your current branch.  To push \
                 to the upstream branch\non the remote, use\n\n    git push origin HEAD:main\n\nTo push to the branch \
                 of the same name on the remote, use\n\n    git push origin HEAD",
            ),
        ),
        ("default-current", AsGit),
        ("default-upstream", AsGit),
        ("default-upstream-mapped", AsGit),
        (
            "default-nothing",
            fail("You didn't specify any refspecs to push, and push.default is \"nothing\"."),
        ),
        ("default-matching", AsGit),
        (
            "detached",
            fail(
                "You are not currently on a branch.\nTo push the history leading to the current (detached HEAD)\nstate \
                 now, use\n\n    git push origin HEAD:<name-of-remote-branch>",
            ),
        ),
        ("remote-push-refspec", AsGit),
        ("push-refspec-mapping", AsGit),
        ("dry-run", AsGit),
        ("push-options", AsGit),
        ("push-options-config", AsGit),
        (
            "push-options-unsupported",
            fail("the receiving end does not support push options"),
        ),
        ("pre-push", AsGit),
        (
            "pre-push-fails",
            Fail {
                git: "failed to push some refs",
                gix: "The pre-push hook failed with code 1, so nothing was pushed",
            },
        ),
        ("no-verify", AsGit),
        ("remote-rejected", AsGit),
        ("empty-remote", AsGit),
        ("packed", AsGit),
        ("packed-no-thin", AsGit),
        ("object-source", AsGit),
        ("push-url", AsGit),
        ("url-remote", AsGit),
        ("push-default-remote", AsGit),
        ("branch-push-remote", AsGit),
        ("src-not-found", fail("src refspec nonexistent does not match any")),
        ("src-ambiguous", fail("src refspec dup matches more than one")),
        ("glob-refspec", AsGit),
        ("negative-refspec", AsGit),
        ("negative-refspec-source", AsGit),
    ];

    #[bisync::bisync]
    #[cfg_attr(feature = "blocking-network-client", test)]
    #[cfg_attr(feature = "async-network-client-async-std", async_std::test)]
    async fn pushes_as_git_push_does() -> Result {
        // Pushing over the local transport starts `git receive-pack`, which inherits the Git configuration of this
        // process, so the test runs in a child process with an isolated environment.
        #[cfg(feature = "blocking-network-client")]
        if gix_testtools::run_in_isolated_process()? {
            return Ok(());
        }
        let fixture = gix_testtools::scripted_fixture_writable("make_push_repos.sh")?;
        let mut cases = Vec::new();
        for entry in std::fs::read_dir(fixture.path())? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                cases.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        cases.sort();
        let mut known: Vec<_> = CASES.iter().map(|(name, _)| (*name).to_owned()).collect();
        known.sort();
        assert_eq!(cases, known, "every case of the fixture is pushed");

        let daemon = spawn_git_daemon_if_async(fixture.path())?;
        for (name, expect) in CASES {
            // Without the `hooks` feature, the `pre-push` hook doesn't run.
            if cfg!(not(feature = "hooks")) && name.starts_with("pre-push") {
                continue;
            }
            push_case(&fixture.path().join(name), name, daemon.as_ref(), expect).await?;
        }
        Ok(())
    }

    /// Push the case `name` at `dir` with `gix`, through `daemon` if there is one, and compare what happens with what
    /// `git push` did as `expect` says.
    #[bisync::bisync]
    async fn push_case(dir: &Path, name: &str, daemon: Option<&GitDaemon>, expect: &Expect) -> Result {
        let before = dir.join("before");
        let mut repo = gix::open_opts(before.join("local"), restricted())?;
        // The URLs of the remotes of the case are relative to `local`, where `git push` ran.
        let base = match daemon {
            Some(daemon) => format!("{}/{name}/before/", daemon.url),
            None => format!(
                "{}/",
                before
                    .to_str()
                    .expect("fixture paths are valid UTF-8")
                    .replace('\\', "/")
            ),
        };
        repo.config_snapshot_mut()
            .set_subsection_value(&Url::INSTEAD_OF, base.as_str(), "../")?;

        let invocation = Invocation::parse(&lines(&dir.join("args"))?, repo.object_hash())?;
        let result = push(&repo, invocation).await;

        let reported: Vec<_> = lines(&dir.join("porcelain"))?
            .into_iter()
            .filter(|line| !line.starts_with(b"To ") && line.as_slice() != b"Done")
            .collect();
        let succeeded = std::fs::read_to_string(dir.join("exit-code"))?.trim() == "0";
        match expect {
            AsGit => {
                let outcome = result?;
                assert_eq!(
                    outcome.porcelain(&repo),
                    reported,
                    "{name}: the updates are reported as `git push --porcelain` reports them"
                );
                assert_eq!(
                    outcome.is_success(),
                    succeeded,
                    "{name}: the push succeeds if `git push` succeeded"
                );
            }
            Fail { git, gix } => {
                let stderr = std::fs::read(dir.join("stderr"))?;
                assert!(
                    reported.is_empty() && !succeeded,
                    "{name}: `git push` failed before it reported on any reference"
                );
                assert!(
                    stderr.contains_str(git),
                    "{name}: `git push` failed with {git:?}, but printed {:?}",
                    stderr.as_bstr()
                );
                let Err(err) = result else {
                    panic!("{name}: pushing with `gix` fails as well");
                };
                // The message of the error, followed by those of its sources.
                let mut message = err.to_string();
                let mut source = std::error::Error::source(&err);
                while let Some(cause) = source {
                    message.push('\n');
                    message.push_str(&cause.to_string());
                    source = cause.source();
                }
                assert!(
                    message.contains(gix),
                    "{name}: pushing with `gix` fails with {gix:?}, but failed with {message:?}"
                );
            }
        }
        assert_eq!(
            remote_refs(&before.join("remote.git"))?,
            lines(&dir.join("remote-refs"))?,
            "{name}: the remote has the references it had after `git push`"
        );
        assert_eq!(
            tracking_refs(&repo)?,
            lines(&dir.join("tracking-refs"))?,
            "{name}: the remote-tracking references are those `git push` left, along with their newest reflog message"
        );
        for (recorded, written) in [
            (dir.join("pre-push"), before.join("local/pre-push")),
            (dir.join("push-options"), before.join("remote.git/push-options")),
        ] {
            if recorded.is_file() {
                assert_eq!(
                    std::fs::read(&written)?.as_bstr(),
                    std::fs::read(&recorded)?.as_bstr(),
                    "{name}: the hook received what it received from `git push`"
                );
            } else {
                assert!(!written.exists(), "{name}: the hook ran only if it ran for `git push`");
            }
        }
        Ok(())
    }

    /// Push from `repo` as `invocation` says.
    #[bisync::bisync]
    async fn push(repo: &gix::Repository, invocation: Invocation) -> gix::Result<push::Outcome> {
        let remote = repo.find_push_remote(invocation.remote.as_ref().map(|name| name.as_bstr()))?;
        remote
            .connect(Push)
            .await?
            .prepare_push(gix::progress::Discard, invocation.options)
            .await?
            .with_dry_run(invocation.dry_run)
            .with_pre_push_hook(invocation.verify)
            .push(gix::progress::Discard, &AtomicBool::default())
            .await
    }

    /// The arguments of `git push`, as `gix` takes them.
    struct Invocation {
        remote: Option<BString>,
        options: push::Options,
        dry_run: bool,
        verify: bool,
    }

    impl Invocation {
        /// Take `args` as `git push` takes them, as far as the cases use them, in a repository with `object_hash`.
        fn parse(args: &[BString], object_hash: gix::hash::Kind) -> Result<Self> {
            let mut invocation = Invocation {
                remote: None,
                options: push::Options::default(),
                dry_run: false,
                verify: true,
            };
            let options = &mut invocation.options;
            let mut args = args.iter();
            while let Some(arg) = args.next() {
                match arg.to_str()? {
                    "--force" => options.force = true,
                    "--delete" => options.delete = true,
                    "--tags" => options.tags = true,
                    "--follow-tags" => options.follow_tags = Some(true),
                    "--atomic" => options.atomic = true,
                    "--no-thin" => options.thin = false,
                    "--force-with-lease" => options.force_with_lease.all_tracking = true,
                    "--dry-run" => invocation.dry_run = true,
                    "--no-verify" => invocation.verify = false,
                    "-o" => {
                        let value = args.next().ok_or("-o needs a value")?;
                        options.push_options.get_or_insert_with(Vec::new).push(value.clone());
                    }
                    other => {
                        if let Some(value) = other.strip_prefix("--push-option=") {
                            options.push_options.get_or_insert_with(Vec::new).push(value.into());
                        } else if let Some(lease) = other.strip_prefix("--force-with-lease=") {
                            let entry = match lease.split_once(':') {
                                None => (lease.into(), push::Expected::TrackingRef),
                                Some((name, "")) => (name.into(), push::Expected::Id(object_hash.null())),
                                Some((name, expect)) => (
                                    name.into(),
                                    push::Expected::Id(gix::ObjectId::from_hex(expect.as_bytes())?),
                                ),
                            };
                            options.force_with_lease.refs.push(entry);
                        } else if other.starts_with('-') {
                            return Err(format!("the option {other} isn't supported here").into());
                        } else if invocation.remote.is_none() {
                            invocation.remote = Some(arg.clone());
                        } else {
                            options.refspecs.push(arg.clone());
                        }
                    }
                }
            }
            Ok(invocation)
        }
    }

    /// The references of the repository at `dir`, as `git for-each-ref --format='%(objectname) %(refname)'` lists them.
    fn remote_refs(dir: &Path) -> Result<Vec<BString>> {
        let repo = gix::open_opts(dir, restricted())?;
        let mut refs = Vec::new();
        for reference in repo.references()?.all()? {
            let mut reference = reference?;
            let name = reference.name().as_bstr().to_owned();
            if name.starts_with(b"refs/") {
                let id = reference.follow_to_object()?.detach();
                refs.push((name.clone(), BString::from(format!("{id} {name}"))));
            }
        }
        refs.sort();
        Ok(refs.into_iter().map(|(_, line)| line).collect())
    }

    /// The remote-tracking references of `repo` with the message of their newest reflog entry, as the fixture lists them.
    fn tracking_refs(repo: &gix::Repository) -> Result<Vec<BString>> {
        let mut refs = Vec::new();
        for reference in repo.references()?.remote_branches()? {
            let mut reference = reference?;
            let name = reference.name().as_bstr().to_owned();
            let message = match reference.log_iter().rev()? {
                Some(mut entries) => entries
                    .next()
                    .transpose()?
                    .map(|entry| entry.message)
                    .unwrap_or_default(),
                None => BString::default(),
            };
            let id = reference.follow_to_object()?.detach();
            refs.push((name.clone(), BString::from(format!("{id} {name} {message}"))));
        }
        refs.sort();
        Ok(refs.into_iter().map(|(_, line)| line).collect())
    }

    /// The lines of the file at `path`.
    fn lines(path: &Path) -> Result<Vec<BString>> {
        Ok(std::fs::read(path)?.lines().map(BString::from).collect())
    }
}
