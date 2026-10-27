use gix_testtools::TestResult;
use std::path::Path;

use gix_sec::Permission;
use gix_testtools::{Env, tempfile};
use serial_test::serial;

use crate::{named_repo, util::named_subrepo_opts};

#[test]
fn custom_committer_fallback_is_only_installed_if_needed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let repo = crate::init_repo_isolated(tmp.path(), gix::create::Kind::Bare)?;
    let git_dir = repo.git_dir().to_owned();
    drop(repo);

    let mut repo = gix::open_opts(
        &git_dir,
        gix::open::Options::isolated().config_overrides(["user.name=Configured User", "user.email=user@example.com"]),
    )?;
    let committer = repo.committer_or_set_fallback("Fallback", "fallback@example.com")?;
    assert_eq!(committer.name, "Configured User", "configured user name must win");
    assert_eq!(committer.email, "user@example.com", "configured user email must win");
    assert_eq!(
        repo.config_snapshot()
            .string(gix::config::tree::gitoxide::Committer::NAME_FALLBACK),
        None,
        "no fallback should be installed when a complete committer resolves"
    );

    let mut repo = gix::open_opts(git_dir, gix::open::Options::isolated())?;
    assert!(repo.committer().is_none(), "the isolated repository has no identity");
    let committer = repo.committer_or_set_fallback("Fallback", "fallback@example.com")?;
    assert_eq!(committer.name, "Fallback", "the supplied fallback name is used");
    assert_eq!(
        committer.email, "fallback@example.com",
        "the supplied fallback email is used"
    );
    let committer = repo.committer().transpose()?.expect("the fallback remains installed");
    assert_eq!(committer.name, "Fallback", "future lookups use the fallback name");
    assert_eq!(
        committer.email, "fallback@example.com",
        "future lookups use the fallback email"
    );
    Ok(())
}

#[test]
fn configured_identity_fallbacks_follow_user_identity() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let repo = crate::init_repo_isolated(tmp.path(), gix::create::Kind::Bare)?;
    let repo = gix::open_opts(
        repo.git_dir(),
        gix::open::Options::isolated().config_overrides([
            "user.name=Configured User",
            "user.email=user@example.com",
            "gitoxide.committer.nameFallback=Fallback",
            "gitoxide.committer.emailFallback=fallback@example.com",
            "gitoxide.author.nameFallback=Fallback Author",
            "gitoxide.author.emailFallback=fallback-author@example.com",
        ]),
    )?;

    let committer = repo
        .committer()
        .transpose()?
        .expect("the user identity supplies a committer");
    assert_eq!(
        committer.name, "Configured User",
        "user.name precedes the committer fallback"
    );
    assert_eq!(
        committer.email, "user@example.com",
        "user.email precedes the committer fallback"
    );
    let author = repo
        .author()
        .transpose()?
        .expect("the user identity supplies an author");
    assert_eq!(author.name, "Configured User", "user.name precedes the author fallback");
    assert_eq!(
        author.email, "user@example.com",
        "user.email precedes the author fallback"
    );
    Ok(())
}

#[test]
#[serial]
fn author_included_by_hasconfig() -> TestResult {
    let _environment = gix_testtools::isolate_git_environment()?;
    let repo = named_subrepo_opts("make_config_repos.sh", "with-hasconfig", gix::open::Options::isolated())?;
    let _environment = _environment
        .set(
            "GIT_CONFIG_SYSTEM",
            repo.git_dir().join("system.config").display().to_string(),
        )
        .unset("GIT_CONFIG_NOSYSTEM")
        .unset("GIT_AUTHOR_NAME")
        .unset("GIT_AUTHOR_EMAIL")
        .unset("GIT_COMMITTER_NAME");
    let repo = gix::open_opts(repo.git_dir(), allow_system_options(repo.open_options().clone()))?;
    let author = repo.author().expect("set in system config via include")?;
    assert_eq!(author.name, "works");
    assert_eq!(author.email, "works@example.com");
    Ok(())
}

#[test]
#[serial]
fn author_and_committer_and_fallback() -> TestResult {
    let _environment = gix_testtools::isolate_git_environment()?;
    for trust in [gix_sec::Trust::Full, gix_sec::Trust::Reduced] {
        let repo = named_repo("make_config_repo.sh")?;
        let work_dir = repo.workdir().expect("present").canonicalize()?;
        let _env = Env::new()
            .set(
                "GIT_CONFIG_SYSTEM",
                work_dir.join("system.config").display().to_string(),
            )
            .unset("GIT_CONFIG_NOSYSTEM")
            .set("GIT_AUTHOR_NAME", "author")
            .set("GIT_AUTHOR_EMAIL", "author@email")
            .set("GIT_AUTHOR_DATE", "Thu, 1 Aug 2022 12:45:06 +0800")
            .set("GIT_COMMITTER_NAME", "committer override")
            .set("GIT_COMMITTER_EMAIL", "committer-override@email")
            .set("GIT_COMMITTER_DATE", "Thu, 1 Aug 2022 12:45:06 -0200")
            .set("EMAIL", "general@email-unused")
            .set("GIT_CONFIG_COUNT", "1")
            .set("GIT_CONFIG_KEY_0", "include.path")
            .set("GIT_CONFIG_VALUE_0", work_dir.join("c.config").display().to_string());
        let repo = gix::open_opts(
            repo.git_dir(),
            allow_system_options(repo.open_options().clone().with(trust)),
        )?;

        assert_eq!(
            repo.author().expect("present")?,
            gix_actor::SignatureRef {
                name: "author".into(),
                email: "author@email".into(),
                time: "1659329106 +0800",
            }
        );

        assert_eq!(
            repo.committer().expect("present")?,
            gix_actor::SignatureRef {
                name: "committer override".into(),
                email: "committer-override@email".into(),
                time: "1659365106 -0200",
            }
        );
        let config = repo.config_snapshot();

        assert_eq!(config.boolean("core.bare"), Some(false));
        assert_eq!(config.boolean("a.bad-bool"), None);
        assert_eq!(config.try_boolean("core.bare")?, Some(false));
        assert!(config.try_boolean("a.bad-bool").is_err());

        assert_eq!(config.integer("a.int"), Some(42));
        assert_eq!(config.integer("a.int-overflowing"), None);
        assert_eq!(config.integer("a.int-overflowing"), None);
        assert!(config.try_integer("a.int-overflowing").is_err());

        assert_eq!(config.string("a.single-string").expect("present"), "hello world");

        assert_eq!(config.string("a.local-override").expect("present"), "from-a.config");
        assert_eq!(config.string("a.system").expect("present"), "from-system.config");
        assert_eq!(config.string("a.system-override").expect("present"), "from-b.config");

        assert_eq!(config.string("a.env-override").expect("present"), "from-c.config");

        assert_eq!(config.boolean("core.missing"), None);
        assert_eq!(config.try_boolean("core.missing")?, None);

        let relative_path_key = "a.relative-path";
        if trust == gix_sec::Trust::Full {
            assert_eq!(
                config.trusted_path(relative_path_key)?.expect("exists"),
                Path::new("./something")
            );
            assert_eq!(
                config.trusted_path("a.absolute-path")?.expect("exists"),
                Path::new("/etc/man.conf")
            );
            assert!(config.trusted_path("a.bad-user-path").is_err());
        } else {
            assert!(
                config.trusted_path(relative_path_key)?.is_none(),
                "trusted path at {relative_path_key} need full trust: {path:?}",
                path = config.string(relative_path_key)
            );
        }
    }
    Ok(())
}

#[test]
#[serial]
fn author_from_different_config_sections() -> TestResult {
    let _environment = gix_testtools::isolate_git_environment()?;
    let repo = named_repo("make_signatures_repo.sh")?;
    let work_dir = repo.workdir().unwrap().canonicalize()?;

    let _environment = _environment
        .set("GIT_CONFIG_GLOBAL", work_dir.join("global.config").to_str().unwrap())
        .set("GIT_CONFIG_SYSTEM", work_dir.join("system.config").to_str().unwrap())
        .unset("GIT_CONFIG_NOSYSTEM")
        .set("GIT_AUTHOR_DATE", "42 +0030")
        .unset("GIT_AUTHOR_NAME")
        .unset("GIT_AUTHOR_EMAIL")
        .set("GIT_COMMITTER_DATE", "1980-02-26 18:30:00 +0000")
        .unset("GIT_COMMITTER_NAME")
        .unset("GIT_COMMITTER_EMAIL")
        .set("EMAIL", "general@email-unused");

    let repo = gix::open_opts(
        repo.git_dir(),
        allow_system_options(
            repo.open_options()
                .clone()
                .config_overrides(None::<&str>)
                .with(gix_sec::Trust::Full),
        ),
    )?;

    assert_eq!(
        repo.author().transpose()?,
        Some(gix_actor::SignatureRef {
            name: "global name".into(),
            email: "local@example.com".into(),
            time: "42 +0030",
        }),
        "author name comes from global config, \
         but email comes from repository-local config",
    );
    assert_eq!(
        repo.committer().transpose()?,
        Some(gix_actor::SignatureRef {
            name: "local committer".into(),
            email: "global-committer@example.com".into(),
            time: "320437800 +0000",
        }),
        "committer name comes from repository-local config, \
         but committer email comes from global config"
    );
    Ok(())
}

#[test]
#[serial]
fn isolated_options_can_load_just_the_configured_identity() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home)?;
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = Global User\n\temail = global@example.com\n\tsigningKey = 1234\n\
         [author]\n\tname = Global Author\n\
         [a]\n\tglobal = yes\n",
    )?;
    let _env = gix_testtools::isolate_git_environment()?
        .set("HOME", home.to_str().expect("UTF-8 temporary path"))
        .unset("GIT_CONFIG_GLOBAL");
    let git_dir = crate::init_repo_isolated(tmp.path().join("repo"), gix::create::Kind::Bare)?
        .git_dir()
        .to_owned();

    let repo = gix::open_opts(&git_dir, isolated_with_identity())?;
    let committer = repo
        .committer()
        .transpose()?
        .expect("the user's configuration has a name and an email");
    assert_eq!(committer.name, "Global User");
    assert_eq!(committer.email, "global@example.com");
    let author = repo
        .author()
        .transpose()?
        .expect("the same configuration names the author");
    assert_eq!(
        author.name, "Global Author",
        "`author.name` takes precedence over `user.name`"
    );
    assert_eq!(author.email, "global@example.com");
    let config = repo.config_snapshot();
    for key in ["a.global", "user.signingKey"] {
        assert_eq!(
            config.string(key),
            None,
            "{key} isn't part of the identity, so it isn't loaded"
        );
    }

    let repo = gix::open_opts(&git_dir, gix::open::Options::isolated())?;
    assert!(
        repo.committer().is_none(),
        "isolated options don't load the identity unless asked to"
    );
    Ok(())
}

#[test]
#[serial]
fn the_configured_identity_follows_includes_and_yields_to_the_repository() -> TestResult {
    use std::io::Write;

    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home)?;
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = Global User\n\temail = global@example.com\n\
         [include]\n\tpath = included.config\n\
         [includeIf \"gitdir:work/\"]\n\tpath = work.config\n\
         [includeIf \"gitdir:elsewhere/\"]\n\tpath = elsewhere.config\n\
         [includeIf \"hasconfig:remote.*.url:https://example.com/**\"]\n\tpath = remote.config\n",
    )?;
    std::fs::write(
        home.join("included.config"),
        "[committer]\n\tname = Included Committer\n",
    )?;
    std::fs::write(home.join("work.config"), "[user]\n\temail = work@example.com\n")?;
    std::fs::write(
        home.join("elsewhere.config"),
        "[user]\n\temail = elsewhere@example.com\n",
    )?;
    std::fs::write(home.join("remote.config"), "[author]\n\tname = Remote Author\n")?;
    let _env = gix_testtools::isolate_git_environment()?
        .set("HOME", home.to_str().expect("UTF-8 temporary path"))
        .unset("GIT_CONFIG_GLOBAL");
    let work = tmp.path().join("work");
    std::fs::create_dir(&work)?;
    let git_dir = crate::init_repo_isolated(work.join("repo"), gix::create::Kind::Bare)?
        .git_dir()
        .to_owned();
    std::fs::OpenOptions::new()
        .append(true)
        .open(git_dir.join("config"))?
        .write_all(b"[remote \"origin\"]\n\turl = https://example.com/project\n[user]\n\tname = Local User\n")?;

    let repo = gix::open_opts(&git_dir, isolated_with_identity())?;
    let committer = repo.committer().transpose()?.expect("configured");
    assert_eq!(
        committer.name, "Included Committer",
        "the includes of the user's configuration are followed"
    );
    assert_eq!(
        committer.email, "work@example.com",
        "`gitdir:` conditions match the location of the repository"
    );
    let author = repo.author().transpose()?.expect("configured");
    assert_eq!(
        author.name, "Remote Author",
        "`hasconfig:remote.*.url:` conditions match the remotes of the repository"
    );
    assert_eq!(
        repo.config_snapshot().string("user.name").expect("configured"),
        "Local User",
        "the configuration of the repository takes precedence"
    );
    Ok(())
}

#[test]
#[serial]
fn the_identity_of_each_file_keeps_the_precedence_of_its_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home)?;
    std::fs::write(home.join(".gitconfig"), "[user]\n\tname = Global User\n")?;
    let system_config = tmp.path().join("system.config");
    std::fs::write(
        &system_config,
        "[user]\n\tname = System User\n\temail = system@example.com\n[a]\n\tsystem = yes\n",
    )?;
    let _env = gix_testtools::isolate_git_environment()?
        .set("HOME", home.to_str().expect("UTF-8 temporary path"))
        .unset("GIT_CONFIG_GLOBAL")
        .unset("GIT_CONFIG_NOSYSTEM");
    let git_dir = crate::init_repo_isolated(tmp.path().join("repo"), gix::create::Kind::Bare)?
        .git_dir()
        .to_owned();

    for (system, user) in [(true, false), (false, true)] {
        let mut options = isolated_with_identity().system_config_path(system_config.clone());
        options.permissions.config.system = system;
        options.permissions.config.user = user;
        // Fully loaded user configuration is only found with `HOME` if the environment permissions allow it.
        options.permissions.env.home = Permission::Allow;
        let repo = gix::open_opts(&git_dir, options)?;
        let committer = repo.committer().transpose()?.expect("configured");
        assert_eq!(
            committer.name, "Global User",
            "the user's configuration takes precedence over the system's, with system: {system}, user: {user}"
        );
        assert_eq!(
            committer.email, "system@example.com",
            "the system's configuration supplies what the user's lacks"
        );
        assert_eq!(
            repo.config_snapshot().string("a.system").is_some(),
            system,
            "only the identity of the system's configuration is loaded unless it's permitted"
        );
    }
    Ok(())
}

#[test]
#[serial]
fn the_identity_is_found_where_git_finds_the_configuration() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home)?;
    std::fs::write(home.join(".gitconfig"), "[user]\n\tname = Home User\n")?;
    let xdg = tmp.path().join("xdg");
    std::fs::create_dir_all(xdg.join("git"))?;
    std::fs::write(
        xdg.join("git").join("config"),
        "[user]\n\tname = XDG User\n\temail = xdg@example.com\n",
    )?;
    let system = tmp.path().join("system.config");
    std::fs::write(&system, "[author]\n\tname = System Author\n")?;
    let global = tmp.path().join("global.config");
    std::fs::write(&global, "[user]\n\tname = Global User\n\temail = global@example.com\n")?;
    let env = gix_testtools::isolate_git_environment()?
        .set("HOME", home.to_str().expect("UTF-8 temporary path"))
        .set("XDG_CONFIG_HOME", xdg.to_str().expect("UTF-8 temporary path"))
        .set("GIT_CONFIG_SYSTEM", system.to_str().expect("UTF-8 temporary path"))
        .unset("GIT_CONFIG_GLOBAL");
    let git_dir = crate::init_repo_isolated(tmp.path().join("repo"), gix::create::Kind::Bare)?
        .git_dir()
        .to_owned();

    let repo = gix::open_opts(&git_dir, isolated_with_identity())?;
    let committer = repo.committer().transpose()?.expect("configured");
    assert_eq!(
        committer.name, "Home User",
        "`~/.gitconfig` takes precedence over the XDG configuration"
    );
    assert_eq!(
        committer.email, "xdg@example.com",
        "the XDG configuration supplies what `~/.gitconfig` lacks"
    );
    let author = repo.author().transpose()?.expect("configured");
    assert_eq!(
        author.name, "Home User",
        "`GIT_CONFIG_NOSYSTEM` leaves the system configuration out"
    );

    let env = env.unset("GIT_CONFIG_NOSYSTEM");
    let repo = gix::open_opts(&git_dir, isolated_with_identity())?;
    let author = repo.author().transpose()?.expect("configured");
    assert_eq!(
        author.name, "System Author",
        "`GIT_CONFIG_SYSTEM` locates the system configuration"
    );

    let _env = env.set("GIT_CONFIG_GLOBAL", global.to_str().expect("UTF-8 temporary path"));
    let repo = gix::open_opts(&git_dir, isolated_with_identity())?;
    let committer = repo.committer().transpose()?.expect("configured");
    assert_eq!(committer.name, "Global User", "`GIT_CONFIG_GLOBAL` replaces both files");
    assert_eq!(committer.email, "global@example.com");
    Ok(())
}

#[test]
#[serial]
fn loading_the_identity_never_starts_git() -> TestResult {
    if gix_testtools::run_in_isolated_process()? {
        return Ok(());
    }
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home)?;
    let _env = gix_testtools::isolate_git_environment()?
        .set("HOME", home.to_str().expect("UTF-8 temporary path"))
        .unset("GIT_CONFIG_GLOBAL")
        .unset("GIT_CONFIG_NOSYSTEM");
    let git_dir = crate::init_repo_isolated(tmp.path().join("repo"), gix::create::Kind::Bare)?
        .git_dir()
        .to_owned();

    // Git writes this file if it's started, and this process hasn't located the configuration yet.
    let trace = tmp.path().join("git.trace");
    let _trace = Env::new().set("GIT_TRACE", trace.to_str().expect("UTF-8 temporary path"));
    gix::open_opts(&git_dir, isolated_with_identity())?;
    assert!(
        !trace.exists(),
        "the system and global configuration are located without starting Git"
    );
    Ok(())
}

/// Isolated options that also load the identity configured in the user's configuration files.
fn isolated_with_identity() -> gix::open::Options {
    let mut options = gix::open::Options::isolated();
    options.permissions.config.identity = true;
    options
}

fn allow_system_options(opts: gix::open::Options) -> gix::open::Options {
    opts.permissions(gix::open::Permissions {
        env: gix::open::permissions::Environment {
            xdg_config_home: Permission::Deny,
            home: Permission::Deny,
            ..gix::open::permissions::Environment::all()
        },
        ..Default::default()
    })
}
