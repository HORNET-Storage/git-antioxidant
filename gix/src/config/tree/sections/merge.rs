use crate::{
    config,
    config::{
        Tree,
        tree::{Key, Merge, Section, SubSectionRequirement, keys},
    },
};

impl Merge {
    /// The `merge.renameLimit` key.
    pub const RENAME_LIMIT: keys::UnsignedInteger = keys::UnsignedInteger::new_unsigned_integer(
        "renameLimit",
        &config::Tree::MERGE,
    )
    .with_note(
        "The limit is actually squared, so 1000 stands for up to 1 million diffs if fuzzy rename tracking is enabled",
    );
    /// The `merge.renames` key.
    #[cfg(feature = "merge")]
    pub const RENAMES: super::diff::Renames = super::diff::Renames::new_renames("renames", &config::Tree::MERGE);
    /// The `merge.directoryRenames` key.
    ///
    /// If [`merge.renames`](Self::RENAMES) is `false`, this setting is ignored and directory
    /// rename detection is disabled. Setting this key to `false` does not disable explicit
    /// file rename detection, which remains controlled by `merge.renames`.
    #[cfg(feature = "merge")]
    pub const DIRECTORY_RENAMES: DirectoryRenames =
        DirectoryRenames::new_with_validate("directoryRenames", &config::Tree::MERGE, validate::DirectoryRenames)
            .with_default(b"conflict")
            .with_note("Ignored when rename tracking is disabled; explicit file renames are tracked independently");
    /// The `merge.renormalize` key
    pub const RENORMALIZE: keys::Boolean = keys::Boolean::new_boolean("renormalize", &Tree::MERGE);
    /// The `merge.default` key
    pub const DEFAULT: keys::String = keys::String::new_string("default", &Tree::MERGE);
    /// The `merge.<driver>.name` key.
    pub const DRIVER_NAME: keys::String = keys::String::new_string("name", &config::Tree::MERGE)
        .with_subsection_requirement(Some(SubSectionRequirement::Parameter("driver")));
    /// The `merge.<driver>.driver` key.
    pub const DRIVER_COMMAND: keys::Program = keys::Program::new_program("driver", &config::Tree::MERGE)
        .with_subsection_requirement(Some(SubSectionRequirement::Parameter("driver")));
    /// The `merge.<driver>.recursive` key.
    pub const DRIVER_RECURSIVE: keys::String = keys::String::new_string("recursive", &config::Tree::MERGE)
        .with_subsection_requirement(Some(SubSectionRequirement::Parameter("driver")));
    /// The `merge.conflictStyle` key.
    #[cfg(feature = "merge")]
    pub const CONFLICT_STYLE: ConflictStyle =
        ConflictStyle::new_with_validate("conflictStyle", &config::Tree::MERGE, validate::ConflictStyle);
    /// The `merge.ff` key, which says whether `git merge` fast-forwards: a boolean, or `only`.
    #[cfg(feature = "merge")]
    pub const FF: FastForward = FastForward::new_with_validate("ff", &config::Tree::MERGE, validate::FastForward);
    /// The `merge.suppressDest` key, a glob pattern of the branches into which the default message of a merge doesn't name
    /// the branch it merges into, which can be given more than once.
    ///
    /// An empty value removes the patterns before it, and without any value, Git suppresses `main` and `master`.
    pub const SUPPRESS_DEST: keys::String = keys::String::new_string("suppressDest", &config::Tree::MERGE);
}

impl Section for Merge {
    fn name(&self) -> &str {
        "merge"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[
            &Self::RENAME_LIMIT,
            #[cfg(feature = "merge")]
            &Self::RENAMES,
            #[cfg(feature = "merge")]
            &Self::DIRECTORY_RENAMES,
            &Self::RENORMALIZE,
            &Self::DEFAULT,
            &Self::DRIVER_NAME,
            &Self::DRIVER_COMMAND,
            &Self::DRIVER_RECURSIVE,
            #[cfg(feature = "merge")]
            &Self::CONFLICT_STYLE,
            #[cfg(feature = "merge")]
            &Self::FF,
            &Self::SUPPRESS_DEST,
        ]
    }
}

/// The `merge.directoryRenames` key.
#[cfg(feature = "merge")]
pub type DirectoryRenames = keys::Any<validate::DirectoryRenames>;

#[cfg(feature = "merge")]
mod directory_renames {
    use crate::error::{ErrorExt, MetadataValue, bail};
    use crate::{
        Result,
        config::{key, tree::Key, tree::sections::merge::DirectoryRenames},
    };
    use gix_merge::tree::DirectoryRenames as Tracking;

    impl DirectoryRenames {
        /// Convert a boolean or the case-insensitive value `conflict` into directory rename handling.
        ///
        /// `value` is expected to be provided by [`gix_config::File::boolean()`], which also handles
        /// implicit boolean values. If the key is absent, use its declared default value.
        pub fn try_into_directory_renames(&'static self, value: Result<Option<bool>>) -> Result<Tracking> {
            Ok(match value {
                Ok(Some(true)) => Tracking::Enabled,
                Ok(Some(false)) => Tracking::Disabled,
                Ok(None) => {
                    return self.try_into_directory_renames(
                        gix_config::Boolean::try_from(self.default_value_or_panic()).map(|b| Some(b.0)),
                    );
                }
                Err(err) => {
                    let Some(MetadataValue::Bytes(value)) = err.metadata().find_map(|metadata| metadata.get("input"))
                    else {
                        return Err(err);
                    };
                    if value.as_slice().eq_ignore_ascii_case(b"conflict") {
                        Tracking::Conflict
                    } else {
                        bail!(err.and_raise(key::error(self, "Invalid configuration value")));
                    }
                }
            })
        }
    }
}

/// The `merge.conflictStyle` key.
#[cfg(feature = "merge")]
pub type ConflictStyle = keys::Any<validate::ConflictStyle>;

/// The `merge.ff` key.
#[cfg(feature = "merge")]
pub type FastForward = keys::Any<validate::FastForward>;

#[cfg(all(feature = "merge", feature = "reset", feature = "revision"))]
mod fast_forward {
    use crate::{bstr::BStr, config::tree::sections::merge::FastForward, merge::into_head};

    impl FastForward {
        /// Return how `git merge` fast-forwards for `value`, as `git_merge_config()` in `builtin/merge.c` reads `merge.ff`:
        /// [`Allow`](into_head::FastForward::Allow) for a true boolean or a key without value, which is `None`,
        /// [`Never`](into_head::FastForward::Never) for a false boolean and [`Only`](into_head::FastForward::Only) for
        /// `only`. Other values are `None`, as Git ignores them.
        pub fn try_into_fast_forward(&self, value: Option<&BStr>) -> Option<into_head::FastForward> {
            let Some(value) = value else {
                return Some(into_head::FastForward::Allow);
            };
            match gix_config::Boolean::try_from(value) {
                Ok(gix_config::Boolean(true)) => Some(into_head::FastForward::Allow),
                Ok(gix_config::Boolean(false)) => Some(into_head::FastForward::Never),
                Err(_) if value == "only" => Some(into_head::FastForward::Only),
                Err(_) => None,
            }
        }
    }
}

#[cfg(feature = "merge")]
mod conflict_style {
    use gix_error::bail;
    use gix_merge::blob::builtin_driver::text;

    use crate::{Result, bstr::ByteSlice, config, config::tree::sections::merge::ConflictStyle};

    impl ConflictStyle {
        /// Derive the diff algorithm identified by `name`, case-insensitively.
        pub fn try_into_conflict_style(&'static self, name: impl gix_utils::AsBStr) -> Result<text::ConflictStyle> {
            let name = name.as_bstr();
            let style = if name.as_bstr() == "merge" {
                text::ConflictStyle::Merge
            } else if name.as_bstr() == "diff3" {
                text::ConflictStyle::Diff3
            } else if name.as_bstr() == "zdiff3" {
                text::ConflictStyle::ZealousDiff3
            } else {
                bail!(config::key::error_with_value(self, "Invalid configuration value", name));
            };
            Ok(style)
        }
    }
}

#[cfg(feature = "merge")]
mod validate {

    use crate::{
        Result,
        bstr::BStr,
        config::tree::{Merge, keys},
    };

    #[derive(Clone, Copy)]
    pub struct DirectoryRenames;
    impl keys::Validate for DirectoryRenames {
        fn validate(&self, value: &BStr) -> Result {
            let boolean = gix_config::Boolean::try_from(value).map(|b| Some(b.0));
            Merge::DIRECTORY_RENAMES.try_into_directory_renames(boolean)?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct ConflictStyle;
    impl keys::Validate for ConflictStyle {
        fn validate(&self, value: &BStr) -> Result {
            Merge::CONFLICT_STYLE.try_into_conflict_style(value)?;
            Ok(())
        }
    }

    /// Git accepts any value of `merge.ff` and ignores those it doesn't know.
    #[derive(Clone, Copy)]
    pub struct FastForward;
    impl keys::Validate for FastForward {
        fn validate(&self, _value: &BStr) -> Result {
            Ok(())
        }
    }
}
