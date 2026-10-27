use std::ffi::OsString;

use crate::error::{ResultExt, message};
use gix_sec::Permission;

use super::{StageOne, interpolate_context, util};
use crate::{
    Result,
    bstr::BString,
    config,
    config::{
        Cache,
        cache::util::ApplyLeniency,
        tree::{Author, Committer, Core, Gitoxide, Http, Notes, gitoxide},
    },
    open,
    repository::init::setup_objects,
};

/// Initialization
impl Cache {
    #[expect(clippy::too_many_arguments)]
    pub fn from_stage_one(
        StageOne {
            git_dir_config,
            mut buf,
            lossy,
            is_bare,
            object_hash,
            reflog: _,
            precompose_unicode: _,
            protect_windows: _,
        }: StageOne,
        git_dir: &std::path::Path,
        branch_name: Option<&gix_ref::FullNameRef>,
        filter_config_section: fn(&gix_config::file::Metadata) -> bool,
        git_install_dir: Option<&std::path::Path>,
        home: Option<&std::path::Path>,
        git_installation_config_path: Option<&std::path::Path>,
        system_config_path: Option<&std::path::Path>,
        environment: open::permissions::Environment,
        attributes: open::permissions::Attributes,
        config_permissions: open::permissions::Config,
        lenient_config: bool,
        api_config_overrides: &[BString],
        cli_config_overrides: &[BString],
        use_repository_local_environment: bool,
    ) -> Result<Self> {
        let config = load(
            Some(git_dir_config),
            &mut buf,
            Some(git_dir),
            branch_name,
            git_install_dir,
            home,
            git_installation_config_path,
            system_config_path,
            environment,
            config_permissions,
            lossy,
            lenient_config,
            api_config_overrides,
            cli_config_overrides,
            use_repository_local_environment,
        )?;

        let hex_len = util::parse_core_abbrev(&config, object_hash).with_leniency(lenient_config)?;

        use util::config_bool;
        let reflog = util::query_refupdates(&config, lenient_config)?;
        let refs_namespace = util::query_refs_namespace(&config, lenient_config)?;
        let ignore_case = config_bool(&config, &Core::IGNORE_CASE, "core.ignoreCase", false, lenient_config)?;
        let use_multi_pack_index = config_bool(
            &config,
            &Core::MULTIPACK_INDEX,
            "core.multiPackIndex",
            true,
            lenient_config,
        )?;
        #[cfg(feature = "revision")]
        let object_kind_hint = util::disambiguate_hint(&config, lenient_config)?;
        let (static_pack_cache_limit_bytes, pack_cache_bytes, object_cache_bytes, alloc_limit_bytes) =
            util::parse_object_caches(&config, lenient_config, filter_config_section)?;
        let loose_compression = super::access::loose_compression(&config, lenient_config, filter_config_section)?;
        // NOTE: When adding a new initial cache, consider adjusting `reread_values_and_clear_caches()` as well.
        Ok(Cache {
            resolved: config.into(),
            use_multi_pack_index,
            object_hash,
            #[cfg(feature = "revision")]
            object_kind_hint,
            static_pack_cache_limit_bytes,
            pack_cache_bytes,
            object_cache_bytes,
            alloc_limit_bytes,
            loose_compression,
            reflog,
            refs_namespace,
            is_bare,
            ignore_case,
            hex_len,
            filter_config_section,
            environment,
            lenient_config,
            attributes,
            user_agent: Default::default(),
            personas: Default::default(),
            url_rewrite: Default::default(),
            #[cfg(feature = "blob-diff")]
            diff_renames: Default::default(),
            #[cfg(any(feature = "blocking-network-client", feature = "async-network-client"))]
            url_scheme: Default::default(),
            #[cfg(feature = "blob-diff")]
            diff_algorithm: Default::default(),
        })
    }

    /// Call this with new `config` to update values and clear caches. Note that none of the values will be applied if a single
    /// one is invalid.
    /// However, those that are lazily read won't be re-evaluated right away and might thus pass now but fail later.
    ///
    /// Note that we unconditionally re-read all values.
    pub fn reread_values_and_clear_caches_replacing_config(&mut self, config: crate::Config) -> Result<()> {
        let prev = std::mem::replace(&mut self.resolved, config);
        match self.reread_values_and_clear_caches() {
            Err(err) => {
                drop(std::mem::replace(&mut self.resolved, prev));
                Err(err)
            }
            Ok(()) => Ok(()),
        }
    }

    /// Similar to `reread_values_and_clear_caches_replacing_config()`, but works on the existing configuration instead of a passed
    /// in one that it them makes the default.
    pub fn reread_values_and_clear_caches(&mut self) -> Result<()> {
        let config = &self.resolved;
        let hex_len = util::parse_core_abbrev(config, self.object_hash).with_leniency(self.lenient_config)?;

        use util::config_bool;
        let ignore_case = config_bool(
            config,
            &Core::IGNORE_CASE,
            "core.ignoreCase",
            false,
            self.lenient_config,
        )?;

        #[cfg(feature = "revision")]
        {
            let object_kind_hint = util::disambiguate_hint(config, self.lenient_config)?;
            self.object_kind_hint = object_kind_hint;
        }
        let reflog = util::query_refupdates(config, self.lenient_config)?;
        let refs_namespace = util::query_refs_namespace(config, self.lenient_config)?;

        self.hex_len = hex_len;
        self.ignore_case = ignore_case;
        self.reflog = reflog;
        self.refs_namespace = refs_namespace;

        self.user_agent = Default::default();
        self.personas = Default::default();
        self.url_rewrite = Default::default();
        #[cfg(feature = "blob-diff")]
        {
            self.diff_renames = Default::default();
            self.diff_algorithm = Default::default();
        }
        (
            self.static_pack_cache_limit_bytes,
            self.pack_cache_bytes,
            self.object_cache_bytes,
            self.alloc_limit_bytes,
        ) = util::parse_object_caches(config, self.lenient_config, self.filter_config_section)?;
        let loose_compression =
            super::access::loose_compression(config, self.lenient_config, self.filter_config_section)?;
        self.loose_compression = loose_compression;
        #[cfg(any(feature = "blocking-network-client", feature = "async-network-client"))]
        {
            self.url_scheme = Default::default();
        }

        Ok(())
    }

    pub(crate) fn make_source_env(
        crate::open::permissions::Environment {
            xdg_config_home,
            git_prefix,
            home,
            ..
        }: open::permissions::Environment,
    ) -> impl FnMut(&str) -> Option<OsString> {
        move |name| {
            match name {
                git_ if git_.starts_with("GIT_") => Some(git_prefix),
                "XDG_CONFIG_HOME" => Some(xdg_config_home),
                "HOME" => {
                    return if home.is_allowed() {
                        gix_path::env::home_dir().map(Into::into)
                    } else {
                        None
                    };
                }
                _ => None,
            }
            .and_then(|perm| perm.check_opt(name).and_then(gix_path::env::var))
        }
    }
}

/// Resolve a permitted repository-independent configuration source, including explicit and environment path overrides.
pub(crate) fn source_path(
    source: gix_config::Source,
    git_installation_config_path: Option<&std::path::Path>,
    system_config_path: Option<&std::path::Path>,
    permissions: open::permissions::Config,
    source_env: &mut dyn FnMut(&str) -> Option<OsString>,
) -> Option<std::path::PathBuf> {
    if !is_permitted(source, permissions) {
        return None;
    }
    locate(source, git_installation_config_path, system_config_path, source_env)
}

/// Return whether `permissions` allow loading the repository-independent configuration `source` in full.
fn is_permitted(source: gix_config::Source, permissions: open::permissions::Config) -> bool {
    match source {
        gix_config::Source::GitInstallation => permissions.git_binary,
        gix_config::Source::System => permissions.system,
        gix_config::Source::Git => permissions.git,
        gix_config::Source::User => permissions.user,
        _ => false,
    }
}

/// Locate the file of the repository-independent configuration `source` with the environment of `source_env`, including
/// explicit and environment path overrides.
fn locate(
    source: gix_config::Source,
    git_installation_config_path: Option<&std::path::Path>,
    system_config_path: Option<&std::path::Path>,
    source_env: &mut dyn FnMut(&str) -> Option<OsString>,
) -> Option<std::path::PathBuf> {
    if matches!(source, gix_config::Source::GitInstallation | gix_config::Source::System) && is_nosystem(source_env) {
        return None;
    }
    match source {
        gix_config::Source::GitInstallation => git_installation_config_path,
        gix_config::Source::System => system_config_path,
        _ => None,
    }
    .map(ToOwned::to_owned)
    .or_else(|| source.storage_location(source_env))
}

/// Return whether `GIT_CONFIG_NOSYSTEM`, as obtained with `source_env`, leaves out the installation's and the system
/// configuration.
fn is_nosystem(source_env: &mut dyn FnMut(&str) -> Option<OsString>) -> bool {
    source_env("GIT_CONFIG_NOSYSTEM")
        .and_then(|value| gix_config::Boolean::try_from(value).ok())
        .is_some_and(|value| value.0)
}

/// Locate the file of the system or global configuration `source` like [`locate()`], to load the identity it
/// configures, with the environment of [`identity_source_env()`].
///
/// Git isn't started to find it, which only `git_binary` permits: the installation's configuration isn't located, and
/// the system configuration only if its location is known without starting Git.
fn locate_identity(
    source: gix_config::Source,
    system_config_path: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    match source {
        gix_config::Source::System => {
            if is_nosystem(&mut identity_source_env) {
                return None;
            }
            system_config_path
                .map(ToOwned::to_owned)
                .or_else(|| identity_source_env("GIT_CONFIG_SYSTEM").map(std::path::PathBuf::from))
                .or_else(|| gix_path::env::system_config_without_starting_git().map(ToOwned::to_owned))
        }
        gix_config::Source::Git | gix_config::Source::User => source.storage_location(&mut identity_source_env),
        _ => None,
    }
}

/// Provide the environment variables that locate Git's system and global configuration files, whatever the environment
/// permissions, to load the identity that these files configure.
fn identity_source_env(name: &str) -> Option<OsString> {
    match name {
        "HOME" | "XDG_CONFIG_HOME" | "GIT_CONFIG_GLOBAL" | "GIT_CONFIG_SYSTEM" | "GIT_CONFIG_NOSYSTEM" => {
            gix_path::env::var(name)
        }
        _ => None,
    }
}

/// A repository-independent configuration file, loaded in full or only for the identity it configures.
struct GlobalFile {
    meta: gix_config::file::Metadata,
    identity_only: bool,
}

/// List the repository-independent configuration files in the order of their sources. Each is loaded in full if
/// `permissions` allow its source, or only for the identity it configures if they allow loading the identity.
///
/// A file that several sources name is loaded once, at the position of the first, and in full if any of them loads it
/// in full.
fn global_files(
    git_installation_config_path: Option<&std::path::Path>,
    system_config_path: Option<&std::path::Path>,
    permissions: open::permissions::Config,
    source_env: &mut dyn FnMut(&str) -> Option<OsString>,
) -> Vec<GlobalFile> {
    let mut files = Vec::<GlobalFile>::new();
    for &source in [
        gix_config::source::Kind::GitInstallation,
        gix_config::source::Kind::System,
        gix_config::source::Kind::Global,
    ]
    .iter()
    .flat_map(|kind| kind.sources())
    {
        let (path, identity_only) = if is_permitted(source, permissions) {
            match locate(source, git_installation_config_path, system_config_path, source_env) {
                Some(path) => (path, false),
                None => continue,
            }
        } else if permissions.identity {
            match locate_identity(source, system_config_path) {
                Some(path) => (path, true),
                None => continue,
            }
        } else {
            continue;
        };
        match files
            .iter_mut()
            .find(|file| file.meta.path.as_deref() == Some(path.as_path()))
        {
            Some(file) => {
                if file.identity_only && !identity_only {
                    file.meta.source = source;
                    file.identity_only = false;
                }
            }
            None => files.push(GlobalFile {
                meta: gix_config::file::Metadata::from(source)
                    .at(path)
                    .with(gix_sec::Trust::Full),
                identity_only,
            }),
        }
    }
    files
}

/// The sections of the identity, which consists of their `name` and `email`.
const IDENTITY_SECTIONS: &[&str] = &["user", "author", "committer"];

/// Load the configuration files of `metas` only for the identity they configure, the `name` and `email` of their
/// `user`, `author` and `committer` sections, following their includes with `options`.
///
/// The `remote` sections of `others`, the configuration loaded in full, take part in including, as Git matches
/// `hasconfig:remote.*.url` conditions with the remotes of all configuration.
fn load_identity<'a>(
    mut metas: impl Iterator<Item = gix_config::file::Metadata>,
    others: impl Iterator<Item = &'a gix_config::File>,
    buf: &mut Vec<u8>,
    options: gix_config::file::init::Options<'_>,
) -> Result<Option<gix_config::File>> {
    let err_on_nonexisting_paths = false;
    let Some(mut identity) = gix_config::File::from_paths_metadata_buf(
        &mut metas,
        buf,
        err_on_nonexisting_paths,
        gix_config::file::init::Options {
            includes: gix_config::file::includes::Options::no_follow(),
            ..options
        },
    )?
    else {
        return Ok(None);
    };
    for config in others {
        for remote in config.sections_by_name("remote").into_iter().flatten() {
            identity.push_section(remote.to_owned())?;
        }
    }
    identity.resolve_includes(options)?;

    let (identity_sections, other_sections): (Vec<_>, Vec<_>) = identity
        .sections_and_ids()
        .map(|(section, id)| {
            let header = section.header();
            let is_identity = header.subsection_name().is_none()
                && IDENTITY_SECTIONS
                    .iter()
                    .any(|name| header.name().eq_ignore_ascii_case(name.as_bytes()));
            (id, is_identity)
        })
        .partition(|(_, is_identity)| *is_identity);
    for (id, _) in other_sections {
        identity.remove_section_by_id(id);
    }
    for (id, _) in identity_sections {
        let mut section = identity
            .section_mut_by_id(id)
            .expect("the section was listed and not removed");
        let other_names: Vec<_> = section
            .value_names()
            .filter(|name| !["name", "email"].iter().any(|key| name.eq_ignore_ascii_case(key)))
            .collect();
        for name in other_names {
            while section.remove(&name).is_some() {}
        }
    }
    Ok(Some(identity))
}

/// Load global configuration and optional repository-local configuration using the same ordering and overrides as
/// repository opening.
#[expect(clippy::too_many_arguments)]
pub(crate) fn load(
    git_dir_config: Option<gix_config::File>,
    buf: &mut Vec<u8>,
    git_dir: Option<&std::path::Path>,
    branch_name: Option<&gix_ref::FullNameRef>,
    git_install_dir: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
    git_installation_config_path: Option<&std::path::Path>,
    system_config_path: Option<&std::path::Path>,
    environment @ open::permissions::Environment {
        git_prefix,
        other,
        ssh_prefix: _,
        xdg_config_home: _,
        home: _,
        http_transport,
        identity,
        objects,
    }: open::permissions::Environment,
    config_permissions @ open::permissions::Config {
        env: use_env,
        includes: use_includes,
        ..
    }: open::permissions::Config,
    lossy: bool,
    lenient: bool,
    api_config_overrides: &[BString],
    cli_config_overrides: &[BString],
    use_repository_local_environment: bool,
) -> Result<gix_config::File> {
    let options = gix_config::file::init::Options {
        includes: if use_includes {
            gix_config::file::includes::Options::follow(
                interpolate_context(git_install_dir, home),
                gix_config::file::includes::conditional::Context { git_dir, branch_name },
            )
        } else {
            gix_config::file::includes::Options::no_follow()
        },
        ..util::base_options(lossy, lenient)
    };
    let git_prefix = &git_prefix;
    let mut source_env = Cache::make_source_env(environment);
    let files = global_files(
        git_installation_config_path,
        system_config_path,
        config_permissions,
        &mut source_env,
    );

    let err_on_nonexisting_paths = false;
    let mut configs = Vec::with_capacity(files.len());
    for file in &files {
        configs.push(if file.identity_only {
            None
        } else {
            gix_config::File::from_paths_metadata_buf(
                &mut std::iter::once(file.meta.clone()),
                buf,
                err_on_nonexisting_paths,
                gix_config::file::init::Options {
                    includes: gix_config::file::includes::Options::no_follow(),
                    ..options
                },
            )?
        });
    }
    let identity_config = if files.iter().any(|file| file.identity_only) {
        // Like the files themselves, the home directory of their includes is found whatever the permissions.
        let home = gix_path::env::home_dir();
        load_identity(
            files
                .iter()
                .filter(|file| file.identity_only)
                .map(|file| file.meta.clone()),
            configs.iter().flatten().chain(git_dir_config.as_ref()),
            buf,
            gix_config::file::init::Options {
                includes: gix_config::file::includes::Options::follow(
                    interpolate_context(git_install_dir, home.as_deref()),
                    gix_config::file::includes::conditional::Context { git_dir, branch_name },
                ),
                ..options
            },
        )?
    } else {
        None
    };

    let mut globals = None::<gix_config::File>;
    for (file, config) in files.iter().zip(configs) {
        let config = match (config, identity_config.as_ref()) {
            (Some(config), _) => config,
            (None, Some(identity_config)) if file.identity_only => {
                let mut config = gix_config::File::new(file.meta.clone());
                for section in identity_config
                    .sections()
                    .filter(|section| section.meta().source == file.meta.source)
                {
                    config.push_section(section.to_owned())?;
                }
                if config.sections().next().is_none() {
                    continue;
                }
                config
            }
            _ => continue,
        };
        match globals.as_mut() {
            Some(globals) => {
                globals.append(config)?;
            }
            None => globals = Some(config),
        }
    }
    let mut globals = globals.unwrap_or_default();

    let local_meta = git_dir_config.as_ref().map(gix_config::File::meta_owned);
    if let Some(git_dir_config) = git_dir_config {
        globals.append(git_dir_config)?;
    }
    globals.resolve_includes(options)?;
    if use_env {
        globals.append(gix_config::File::from_env(options)?.unwrap_or_default())?;
    }
    if !cli_config_overrides.is_empty() {
        config::overrides::append(&mut globals, cli_config_overrides, gix_config::Source::Cli, |_| None).or_raise(
            || {
                message!(
                    "{:?} configuration overrides at open or init time could not be applied.",
                    gix_config::Source::Cli
                )
            },
        )?;
    }
    if !api_config_overrides.is_empty() {
        config::overrides::append(&mut globals, api_config_overrides, gix_config::Source::Api, |_| None).or_raise(
            || {
                message!(
                    "{:?} configuration overrides at open or init time could not be applied.",
                    gix_config::Source::Api
                )
            },
        )?;
    }
    apply_environment_overrides(
        &mut globals,
        *git_prefix,
        other,
        http_transport,
        identity,
        objects,
        use_repository_local_environment,
    )?;
    if let Some(local_meta) = local_meta {
        globals.set_meta(local_meta);
    }
    Ok(globals)
}

impl crate::Repository {
    /// Replace our own configuration with `config` and re-read all cached values, and apply them to select in-memory instances.
    pub(crate) fn reread_values_and_clear_caches_replacing_config(&mut self, config: crate::Config) -> Result<()> {
        let (
            previous_static_pack_cache_limit_bytes,
            previous_pack_cache_bytes,
            previous_object_cache_bytes,
            previous_loose_compression,
        ) = (
            self.config.static_pack_cache_limit_bytes,
            self.config.pack_cache_bytes,
            self.config.object_cache_bytes,
            self.config.loose_compression,
        );
        self.config.reread_values_and_clear_caches_replacing_config(config)?;
        self.apply_changed_values();
        if previous_static_pack_cache_limit_bytes != self.config.static_pack_cache_limit_bytes
            || previous_pack_cache_bytes != self.config.pack_cache_bytes
            || previous_object_cache_bytes != self.config.object_cache_bytes
        {
            setup_objects(&mut self.objects, &self.config);
        }
        if previous_loose_compression != self.config.loose_compression {
            // This will only affect newly opened object databases.
            // This is fine for now, it's not expected to be changed at runtime.
            self.objects.loose_compression = self.config.loose_compression;
        }
        Ok(())
    }

    fn apply_changed_values(&mut self) {
        self.refs.write_reflog = util::reflog_or_default(self.config.reflog, self.workdir().is_some());
        self.refs.namespace.clone_from(&self.config.refs_namespace);
    }
}

fn apply_environment_overrides(
    config: &mut gix_config::File,
    git_prefix: Permission,
    other: Permission,
    http_transport: Permission,
    identity: Permission,
    objects: Permission,
    use_repository_local_environment: bool,
) -> Result<()> {
    fn env(key: &'static dyn config::tree::Key) -> &'static str {
        key.the_environment_override()
    }
    fn var_as_bstring(var: &str, perm: Permission) -> Option<BString> {
        perm.check_opt(var)
            .and_then(std::env::var_os)
            .and_then(|val| gix_path::os_string_into_bstring(val).ok())
    }

    let mut env_override = gix_config::File::new(gix_config::file::Metadata::from(gix_config::Source::EnvOverride));
    for (section_name, subsection_name, permission, data) in [
        (
            "core",
            None,
            if use_repository_local_environment {
                git_prefix
            } else {
                Permission::Deny
            },
            &[{
                let key = &Core::WORKTREE;
                (env(key), key.name)
            }][..],
        ),
        (
            "http",
            None,
            http_transport,
            &[
                ("GIT_HTTP_LOW_SPEED_LIMIT", "lowSpeedLimit"),
                ("GIT_HTTP_LOW_SPEED_TIME", "lowSpeedTime"),
                ("GIT_HTTP_USER_AGENT", "userAgent"),
                {
                    let key = &Http::SSL_CA_INFO;
                    (env(key), key.name)
                },
                {
                    let key = &Http::SSL_VERSION;
                    (env(key), key.name)
                },
            ][..],
        ),
        (
            "notes",
            None,
            git_prefix,
            &[{
                let key = &Notes::DISPLAY_REF;
                (env(key), key.name)
            }][..],
        ),
        (
            "gitoxide",
            None,
            other,
            &[
                {
                    let key = &Gitoxide::TERM;
                    (env(key), key.name)
                },
                {
                    let key = &Gitoxide::VISUAL;
                    (env(key), key.name)
                },
                {
                    let key = &Gitoxide::EDITOR;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            None,
            git_prefix,
            &[
                {
                    let key = &Gitoxide::TRACE_PACKET;
                    (env(key), key.name)
                },
                {
                    let key = &Gitoxide::PARSE_PRECIOUS;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("https"),
            http_transport,
            &[
                ("HTTPS_PROXY", gitoxide::Https::PROXY.name),
                ("https_proxy", gitoxide::Https::PROXY.name),
            ],
        ),
        (
            "gitoxide",
            Some("http"),
            http_transport,
            &[
                ("ALL_PROXY", "allProxy"),
                {
                    let key = &gitoxide::Http::ALL_PROXY;
                    (env(key), key.name)
                },
                ("NO_PROXY", "noProxy"),
                {
                    let key = &gitoxide::Http::NO_PROXY;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Http::PROXY;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Http::VERBOSE;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Http::PROXY_AUTH_METHOD;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("http"),
            git_prefix,
            &[{
                let key = &gitoxide::Http::SSL_NO_VERIFY;
                (env(key), key.name)
            }],
        ),
        (
            "gitoxide",
            Some("credentials"),
            git_prefix,
            &[
                {
                    let key = &gitoxide::Credentials::TERMINAL_PROMPT;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Credentials::HELPER_STDERR;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "committer",
            None,
            identity,
            &[
                {
                    let key = &Committer::NAME;
                    (env(key), key.name)
                },
                {
                    let key = &Committer::EMAIL;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("core"),
            if use_repository_local_environment {
                git_prefix
            } else {
                Permission::Deny
            },
            &[
                {
                    let key = &gitoxide::Core::SHALLOW_FILE;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Core::REFS_NAMESPACE;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("core"),
            if use_repository_local_environment {
                git_prefix
            } else {
                Permission::Deny
            },
            &[{
                let key = &gitoxide::Core::INDEX_FILE;
                (env(key), key.name)
            }],
        ),
        (
            "gitoxide",
            Some("core"),
            git_prefix,
            &[{
                let key = &gitoxide::Core::EXTERNAL_COMMAND_STDERR;
                (env(key), key.name)
            }],
        ),
        (
            "author",
            None,
            identity,
            &[
                {
                    let key = &Author::NAME;
                    (env(key), key.name)
                },
                {
                    let key = &Author::EMAIL;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("commit"),
            git_prefix,
            &[
                {
                    let key = &gitoxide::Commit::COMMITTER_DATE;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Commit::AUTHOR_DATE;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("allow"),
            git_prefix,
            &[
                {
                    let key = &gitoxide::Allow::PROTOCOL;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Allow::PROTOCOL_FROM_USER;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("user"),
            identity,
            &[{
                let key = &gitoxide::User::EMAIL_FALLBACK;
                (env(key), key.name)
            }],
        ),
        (
            "gitoxide",
            Some("objects"),
            objects,
            &[
                {
                    let key = &gitoxide::Objects::REPLACE_REF_BASE;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Objects::CACHE_LIMIT;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Objects::ALLOC_LIMIT;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "gitoxide",
            Some("ssh"),
            git_prefix,
            &[{
                let key = &gitoxide::Ssh::COMMAND_WITHOUT_SHELL_FALLBACK;
                (env(key), key.name)
            }],
        ),
        (
            "gitoxide",
            Some("pathspec"),
            git_prefix,
            &[
                {
                    let key = &gitoxide::Pathspec::LITERAL;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Pathspec::GLOB;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Pathspec::NOGLOB;
                    (env(key), key.name)
                },
                {
                    let key = &gitoxide::Pathspec::ICASE;
                    (env(key), key.name)
                },
            ],
        ),
        (
            "ssh",
            None,
            git_prefix,
            &[{
                let key = &config::tree::Ssh::VARIANT;
                (env(key), key.name)
            }],
        ),
        #[cfg(feature = "blob-diff")]
        (
            "diff",
            None,
            git_prefix,
            &[{
                let key = &config::tree::Diff::EXTERNAL;
                (env(key), key.name)
            }],
        ),
    ] {
        let mut section = env_override
            .new_section(section_name, subsection_name.map(BString::from))
            .expect("statically known valid section name");
        for (var, key) in data {
            if let Some(value) = var_as_bstring(var, permission) {
                section.push_with_comment(*key, value, format!("from {var}"))?;
            }
        }
        if section.num_values() == 0 {
            let id = section.id();
            env_override.remove_section_by_id(id);
        }
    }

    // These should be independent of the use_repository_local_environment condition.
    {
        let mut section = env_override
            .new_section("core", None)
            .expect("statically known valid section name");

        for (var, key, permission) in [
            {
                let key = &Core::DELTA_BASE_CACHE_LIMIT;
                (env(key), key.name, objects)
            },
            {
                let key = &Core::EDITOR;
                (env(key), key.name, git_prefix)
            },
            {
                let key = &Core::NOTES_REF;
                (env(key), key.name, git_prefix)
            },
            {
                let key = &Core::SSH_COMMAND;
                (env(key), key.name, git_prefix)
            },
        ] {
            if let Some(value) = var_as_bstring(var, permission) {
                section.push_with_comment(key, value, format!("from {var}"))?;
            }
        }

        let key = &Core::USE_REPLACE_REFS;
        let var = env(key);
        if var_as_bstring(var, objects).is_some() {
            // Git disables replacements whenever this negative environment switch is present.
            section.push_with_comment(key.name, "false", format!("from {var}"))?;
        }
        if section.num_values() == 0 {
            let id = section.id();
            env_override.remove_section_by_id(id);
        }
    }

    if !env_override.is_void() {
        config.append(env_override)?;
    }
    Ok(())
}
