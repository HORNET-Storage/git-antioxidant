use crate::{
    config,
    config::tree::{Commit, Key, Section, keys},
};

impl Commit {
    /// The `commit.cleanup` key, how `git commit` cleans the message of a commit unless `--cleanup` says otherwise.
    pub const CLEANUP: Cleanup = Cleanup::new_with_validate("cleanup", &config::Tree::COMMIT, validate::Cleanup);
    /// The `commit.gpgSign` key.
    pub const GPG_SIGN: keys::Boolean =
        keys::Boolean::new_boolean("gpgSign", &config::Tree::COMMIT).with_default(b"false");
}

impl Section for Commit {
    fn name(&self) -> &str {
        "commit"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[&Self::CLEANUP, &Self::GPG_SIGN]
    }
}

/// The `commit.cleanup` key.
pub type Cleanup = keys::Any<validate::Cleanup>;

mod cleanup {
    use crate::{Result, bstr::ByteSlice, commit::message, config, config::tree::commit::Cleanup, error::bail};

    impl Cleanup {
        /// Return the cleanup that `value` names, as `get_cleanup_mode()` in `sequencer.c` reads `--cleanup` and
        /// `commit.cleanup`: `default`, `verbatim`, `whitespace`, `strip` or `scissors`, in lower case.
        pub fn try_into_cleanup(&'static self, value: impl gix_utils::AsBStr) -> Result<message::Cleanup> {
            let value = value.as_bstr();
            Ok(match value.as_bytes() {
                b"default" => message::Cleanup::Default,
                b"verbatim" => message::Cleanup::Verbatim,
                b"whitespace" => message::Cleanup::Whitespace,
                b"strip" => message::Cleanup::Strip,
                b"scissors" => message::Cleanup::Scissors,
                _ => bail!(config::key::error_with_value(self, "Invalid cleanup mode", value)),
            })
        }
    }
}

mod validate {
    use crate::{
        Result,
        bstr::BStr,
        config::tree::{Commit, keys},
    };

    #[derive(Clone, Copy)]
    pub struct Cleanup;
    impl keys::Validate for Cleanup {
        fn validate(&self, value: &BStr) -> Result {
            Commit::CLEANUP.try_into_cleanup(value)?;
            Ok(())
        }
    }
}
