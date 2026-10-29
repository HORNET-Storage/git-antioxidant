use crate::{
    config,
    config::tree::{Key, Push, Section, keys},
};

impl Push {
    /// The `push.default` key
    pub const DEFAULT: Default = Default::new_with_validate("default", &config::Tree::PUSH, validate::Default);
    /// The `push.followTags` key, which makes pushes send the annotated tags pointing into what they push, as
    /// `git push --follow-tags` does.
    pub const FOLLOW_TAGS: keys::Boolean = keys::Boolean::new_boolean("followTags", &config::Tree::PUSH);
    /// The `push.pushOption` key, with the strings sent to the hooks of the remote when no `--push-option` is given.
    ///
    /// It may be given more than once, and an empty value clears the values given before it.
    pub const PUSH_OPTION: keys::String = keys::String::new_string("pushOption", &config::Tree::PUSH);
}

impl Section for Push {
    fn name(&self) -> &str {
        "push"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[&Self::DEFAULT, &Self::FOLLOW_TAGS, &Self::PUSH_OPTION]
    }
}

/// The `push.default` key type.
pub type Default = keys::Any<validate::Default>;

mod default {
    use gix_error::bail;

    use crate::{Result, bstr::ByteSlice, config, config::tree::push::Default, push};

    impl Default {
        /// Try to interpret `value` as `push.default`.
        pub fn try_into_default(&'static self, value: impl gix_utils::AsBStr) -> Result<push::Default> {
            let value = value.as_bstr();
            Ok(match value.as_bstr().as_bytes() {
                b"nothing" => push::Default::Nothing,
                b"current" => push::Default::Current,
                b"upstream" | b"tracking" => push::Default::Upstream,
                b"simple" => push::Default::Simple,
                b"matching" => push::Default::Matching,
                _ => {
                    bail!(config::key::error_with_value(
                        self,
                        "Invalid configuration value",
                        value,
                    ));
                }
            })
        }
    }
}

mod validate {
    #[derive(Clone, Copy)]
    pub struct Default;

    use crate::{Result, bstr::BStr, config::tree::keys::Validate};

    impl Validate for Default {
        fn validate(&self, value: &BStr) -> Result {
            super::Push::DEFAULT.try_into_default(value)?;
            Ok(())
        }
    }
}
