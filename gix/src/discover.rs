use std::{borrow::Cow, path::Path};

use gix_error::{ErrorExt, ResultExt, message};

pub use gix_discover::*;

use crate::{Result, ThreadSafeRepository, bstr::BString};

impl ThreadSafeRepository {
    /// Try to open a git repository in `directory` and search upwards through its parents until one is found,
    /// using default trust options which matters in case the found repository isn't owned by the current user.
    pub fn discover(directory: impl AsRef<Path>) -> Result<Self> {
        Self::discover_opts(directory, Default::default(), Default::default())
    }

    /// Try to open a git repository in `directory` and search upwards through its parents until one is found,
    /// while applying `options`. Then use the `trust_map` to determine which of our own repository options to use
    /// for instantiations.
    ///
    /// Note that [trust overrides](crate::open::Options::with()) in the `trust_map` are not effective here and we will
    /// always override it with the determined trust value as per [gix_discover::upwards::Options::trust].
    /// This value, however, can be set to [assume a given trust level](gix_discover::upwards::TrustPolicy::Assume) to let
    /// callers control the trust level without re-determining it.
    ///
    /// Like in Git, `safe.directory` decides about repositories that aren't owned by the current user. It's compared with
    /// the directory in which the `.git` directory or file was found, or with the git directory if discovery started
    /// within it. Thus, [`Required(Full)`](gix_discover::upwards::TrustPolicy::Required) also finds repositories that
    /// `safe.directory` allows. Other repositories fail with [`NoTrustedGitRepository`](upwards::Error::NoTrustedGitRepository),
    /// raised over a [`DubiousOwnership`](crate::open::DubiousOwnership) which names the directory to allow.
    pub fn discover_opts(
        directory: impl AsRef<Path>,
        mut options: upwards::Options<'_>,
        trust_map: gix_sec::trust::Mapping<crate::open::Options>,
    ) -> Result<Self> {
        let _span = gix_trace::coarse!("ThreadSafeRepository::discover()");
        let directory = directory.as_ref();
        let policy = options.trust;
        let requires_full_trust = policy == upwards::TrustPolicy::Required(gix_sec::Trust::Full);
        if requires_full_trust {
            // `safe.directory` may still make the repository trusted, which is only known once it's opened.
            options.trust = upwards::TrustPolicy::Required(gix_sec::Trust::Reduced);
        }
        let (path, mut trust) = upwards_opts(directory, options)?;
        let (git_dir, worktree_dir) = path.into_repository_and_work_tree_directories();
        // Note that we will adjust the `current_dir` later so it matches the value of `core.precomposeUnicode`.
        let current_dir =
            gix_fs::current_dir(false).or_raise(|| message("Could not obtain the current working directory"))?;
        // Like Git, consider nothing owned by the current user if `GIT_TEST_ASSUME_DIFFERENT_OWNER` is set.
        if matches!(policy, upwards::TrustPolicy::Required(_))
            && crate::open::safe_directory::assume_different_owner(&trust_map.by_level(trust).permissions.env)?
        {
            trust = gix_sec::Trust::Reduced;
        }
        let safe_directory_path = crate::open::safe_directory::discovered_directory(
            directory,
            &git_dir,
            worktree_dir.as_deref(),
            &current_dir,
        );
        let mut open_options = trust_map.into_value_by_level(if requires_full_trust {
            gix_sec::Trust::Full
        } else {
            trust
        });
        let bail_if_untrusted = open_options.bail_if_untrusted;
        open_options.bail_if_untrusted |= requires_full_trust;
        open_options.git_dir_trust = trust.into();
        open_options.current_dir = Some(current_dir.clone());
        let candidate = git_dir.clone();
        match Self::open_from_paths(git_dir, worktree_dir, open_options, None, Some(safe_directory_path)) {
            Ok(mut repo) => {
                repo.linked_worktree_options.bail_if_untrusted = bail_if_untrusted;
                Ok(repo)
            }
            Err(err) if requires_full_trust && err.downcast_any_ref::<crate::open::DubiousOwnership>().is_some() => {
                Err(err.and_raise(upwards::Error::NoTrustedGitRepository {
                    path: gix_path::normalize(directory.into(), &current_dir)
                        .map_or_else(|| directory.to_owned(), Cow::into_owned),
                    candidate,
                    required: gix_sec::Trust::Full,
                    trust: gix_sec::Trust::Reduced,
                }))
            }
            Err(err) => Err(err),
        }
    }

    /// Try to open a git repository directly from the environment.
    /// If that fails, discover upwards from `directory` until one is found,
    /// while applying discovery options from the environment.
    ///
    /// For more, see [`ThreadSafeRepository::discover_with_environment_overrides_opts()`].
    pub fn discover_with_environment_overrides(directory: impl AsRef<Path>) -> Result<Self> {
        Self::discover_with_environment_overrides_opts(directory, Default::default(), Default::default())
    }

    /// Try to open a git repository directly from the environment, which reads `GIT_DIR`
    /// if it is set. Once selected, the repository also honors these primary environment
    /// overrides if permitted by its options:
    ///
    /// - `GIT_WORK_TREE`
    /// - `GIT_INDEX_FILE`
    /// - `GIT_SHALLOW_FILE`
    /// - `GIT_NAMESPACE`
    ///
    /// If `GIT_DIR` is unset, discover upwards from `directory` until one is found,
    /// while applying `options` with discovery overrides from the environment which includes:
    ///
    /// - `GIT_DISCOVERY_ACROSS_FILESYSTEM`
    /// - `GIT_CEILING_DIRECTORIES`
    ///
    /// This is particularly useful for hooks, as Git exports repository-local environment variables
    /// to them. Honoring these variables preserves the repository and worktree context established by
    /// Git, including temporary overrides, instead of relying only on discovery from `directory`.
    ///
    /// Finally, use the `trust_map` to determine which of our own repository options to use
    /// based on the trust level of the effective repository directory.
    ///
    /// ### Note
    ///
    /// Consider to set [`match_ceiling_dir_or_error = false`](gix_discover::upwards::Options::match_ceiling_dir_or_error)
    /// to allow discovery if an outside environment variable sets non-matching ceiling directories for greater
    /// compatibility with Git.
    pub fn discover_with_environment_overrides_opts(
        directory: impl AsRef<Path>,
        mut options: upwards::Options<'_>,
        trust_map: gix_sec::trust::Mapping<crate::open::Options>,
    ) -> Result<Self> {
        fn apply_additional_environment(mut opts: upwards::Options<'_>) -> upwards::Options<'_> {
            use crate::bstr::ByteVec;

            if let Some(cross_fs) = std::env::var_os("GIT_DISCOVERY_ACROSS_FILESYSTEM")
                .and_then(|v| Vec::from_os_string(v).ok().map(BString::from))
                && let Ok(b) = gix_config::Boolean::try_from(cross_fs)
            {
                opts.cross_fs = b.into();
            }
            opts
        }

        if std::env::var_os("GIT_DIR").is_some() {
            return Self::open_with_environment_overrides(directory.as_ref(), trust_map);
        }

        options = apply_additional_environment(options.apply_environment());
        Self::discover_opts(directory, options, trust_map)
    }
}
