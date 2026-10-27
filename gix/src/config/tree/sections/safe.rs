use crate::{
    config,
    config::tree::{Key, Safe, Section, keys},
};

impl Safe {
    /// The `safe.directory` key
    pub const DIRECTORY: keys::Any = keys::Any::new("directory", &config::Tree::SAFE);
}

impl Safe {
    /// Implements the directory filter to trust only protected configuration, for use with `safe.directory`.
    ///
    /// Like in Git, that's all configuration except for the repository-local one: the system and global configuration
    /// along with their includes, and the command scope, which includes the overrides from environment variables like
    /// `GIT_CONFIG_COUNT`, from the command line and from the API. The configuration of the Git installation is part of
    /// the system configuration in Git, even if it reports another scope like Apple Git does.
    ///
    /// Note that Git reads `safe.directory` before it knows the repository, so conditional includes don't apply to it.
    pub fn directory_filter(meta: &gix_config::file::Metadata) -> bool {
        meta.source.kind() != gix_config::source::Kind::Repository
    }
}

impl Section for Safe {
    fn name(&self) -> &str {
        "safe"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[&Self::DIRECTORY]
    }
}
