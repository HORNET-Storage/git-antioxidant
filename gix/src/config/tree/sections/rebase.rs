use crate::{
    config,
    config::tree::{Key, Rebase, Section, keys},
};

impl Rebase {
    /// The `rebase.abbreviateCommands` key, which makes the todo list of an interactive rebase name each command with
    /// its letter, as `p` for `pick`.
    pub const ABBREVIATE_COMMANDS: keys::Boolean =
        keys::Boolean::new_boolean("abbreviateCommands", &config::Tree::REBASE).with_default(b"false");
    /// The `rebase.autoSquash` key, which makes interactive rebases move the commits whose subjects start with
    /// `fixup! `, `amend! ` or `squash! ` after the commits they name, as `--autosquash` does.
    pub const AUTO_SQUASH: keys::Boolean =
        keys::Boolean::new_boolean("autoSquash", &config::Tree::REBASE).with_default(b"false");
    /// The `rebase.forkPoint` key, which, if `false`, keeps a rebase without an upstream from leaving out the commits
    /// that the configured upstream had before, as `--no-fork-point` does.
    pub const FORK_POINT: keys::Boolean =
        keys::Boolean::new_boolean("forkPoint", &config::Tree::REBASE).with_default(b"true");
    /// The `rebase.instructionFormat` key, the format of the commits in the todo list of a rebase, which follows
    /// `# ` unless it starts with `#`, and is `%s` by default.
    pub const INSTRUCTION_FORMAT: keys::String = keys::String::new_string("instructionFormat", &config::Tree::REBASE);
    /// The `rebase.missingCommitsCheck` key: `ignore`, `warn` or `error` for the commits that an edit of the todo list
    /// of an interactive rebase removes without `drop`. Git ignores other values with a warning.
    pub const MISSING_COMMITS_CHECK: keys::String =
        keys::String::new_string("missingCommitsCheck", &config::Tree::REBASE).with_default(b"ignore");
    /// The `rebase.rebaseMerges` key, which makes rebases keep the merges of the branch as `--rebase-merges` does.
    pub const REBASE_MERGES: RebaseMerges =
        RebaseMerges::new_with_validate("rebaseMerges", &config::Tree::REBASE, validate::RebaseMerges);
    /// The `rebase.rescheduleFailedExec` key, which puts an `exec` command whose command fails back into the todo list,
    /// as `--reschedule-failed-exec` does.
    pub const RESCHEDULE_FAILED_EXEC: keys::Boolean =
        keys::Boolean::new_boolean("rescheduleFailedExec", &config::Tree::REBASE).with_default(b"false");
    /// The `rebase.updateRefs` key, which makes rebases update the branches that point to the commits they replay, as
    /// `--update-refs` does.
    pub const UPDATE_REFS: keys::Boolean =
        keys::Boolean::new_boolean("updateRefs", &config::Tree::REBASE).with_default(b"false");
}

impl Section for Rebase {
    fn name(&self) -> &str {
        "rebase"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[
            &Self::ABBREVIATE_COMMANDS,
            &Self::AUTO_SQUASH,
            &Self::FORK_POINT,
            &Self::INSTRUCTION_FORMAT,
            &Self::MISSING_COMMITS_CHECK,
            &Self::REBASE_MERGES,
            &Self::RESCHEDULE_FAILED_EXEC,
            &Self::UPDATE_REFS,
        ]
    }
}

/// The `rebase.rebaseMerges` key.
pub type RebaseMerges = keys::Any<validate::RebaseMerges>;

mod rebase_merges {
    use crate::{
        Result,
        bstr::{BStr, ByteSlice},
        config,
        config::tree::rebase::RebaseMerges,
        error::bail,
    };

    impl RebaseMerges {
        /// Return what `value` of `rebase.rebaseMerges` asks for, as `rebase_config()` reads it: `None` for a false
        /// boolean, which rebases no merges, and otherwise whether the commits that don't descend from the upstream are
        /// rebased too, as `rebase-cousins` says, where a true boolean and `no-rebase-cousins` keep them in place.
        ///
        /// `None` for `value` is the implicit `true` of a key without value, as
        /// [`gix_config::file::section::BodyRef::value_implicit()`] returns it.
        pub fn try_into_rebase_merges(&'static self, value: Option<impl gix_utils::AsBStr>) -> Result<Option<bool>> {
            let Some(value) = value else {
                return Ok(Some(false));
            };
            let value: &BStr = value.as_bstr();
            if let Ok(boolean) = gix_config::Boolean::try_from(value) {
                return Ok(boolean.0.then_some(false));
            }
            Ok(match value.as_bytes() {
                b"no-rebase-cousins" => Some(false),
                b"rebase-cousins" => Some(true),
                _ => bail!(config::key::error_with_value(self, "Unknown rebase-merges mode", value)),
            })
        }
    }
}

mod validate {
    use crate::{
        Result,
        bstr::BStr,
        config::tree::{Rebase, keys},
    };

    #[derive(Clone, Copy)]
    pub struct RebaseMerges;
    impl keys::Validate for RebaseMerges {
        fn validate(&self, value: &BStr) -> Result {
            Rebase::REBASE_MERGES.try_into_rebase_merges(Some(value))?;
            Ok(())
        }
    }
}
