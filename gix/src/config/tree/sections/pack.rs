use crate::{
    config,
    config::tree::{Key, Pack, Section, keys},
};

impl Pack {
    /// The `pack.threads` key.
    pub const THREADS: keys::UnsignedInteger =
        keys::UnsignedInteger::new_unsigned_integer("threads", &config::Tree::PACK)
            .with_deviation("Leaving this key unspecified uses all available cores, instead of 1");

    /// The `pack.indexVersion` key.
    pub const INDEX_VERSION: IndexVersion =
        IndexVersion::new_with_validate("indexVersion", &config::Tree::PACK, validate::IndexVersion);

    /// The `pack.compression` key.
    pub const COMPRESSION: keys::Compression = keys::Compression::new_compression("compression", &config::Tree::PACK);

    /// The `pack.window` key.
    pub const WINDOW: Window = Window::new_with_validate("window", &config::Tree::PACK, validate::Window);

    /// The `pack.depth` key.
    pub const DEPTH: Depth = Depth::new_with_validate("depth", &config::Tree::PACK, validate::Depth);

    /// The `pack.windowMemory` key.
    pub const WINDOW_MEMORY: WindowMemory =
        WindowMemory::new_with_validate("windowMemory", &config::Tree::PACK, validate::WindowMemory).with_deviation(
            "Values of 4 GiB or more are read on every platform, as Git reads them on 64-bit Linux and macOS, while Git for Windows and Git on 32-bit platforms, whose `unsigned long` has 32 bits, refuse them",
        );

    /// The `pack.deltaCacheSize` key.
    pub const DELTA_CACHE_SIZE: DeltaCacheSize =
        DeltaCacheSize::new_with_validate("deltaCacheSize", &config::Tree::PACK, validate::DeltaCacheSize).with_deviation(
            "A negative value is converted to an unsigned number of 64 bits on every platform, as Git converts it on 64-bit Linux and macOS, while Git for Windows and Git on 32-bit platforms, whose `unsigned long` has 32 bits, convert it to one of 32 bits",
        );

    /// The `pack.deltaCacheLimit` key.
    pub const DELTA_CACHE_LIMIT: DeltaCacheLimit =
        DeltaCacheLimit::new_with_validate("deltaCacheLimit", &config::Tree::PACK, validate::DeltaCacheLimit);
}

/// The `pack.indexVersion` key.
pub type IndexVersion = keys::Any<validate::IndexVersion>;

/// The `pack.window` key.
pub type Window = keys::Any<validate::Window>;

/// The `pack.depth` key.
pub type Depth = keys::Any<validate::Depth>;

/// The `pack.windowMemory` key.
pub type WindowMemory = keys::Any<validate::WindowMemory>;

/// The `pack.deltaCacheSize` key.
pub type DeltaCacheSize = keys::Any<validate::DeltaCacheSize>;

/// The `pack.deltaCacheLimit` key.
pub type DeltaCacheLimit = keys::Any<validate::DeltaCacheLimit>;

mod delta_search {
    use gix_error::ResultExt;
    use gix_pack::data::delta::search::{MAX_DELTA_CACHE_LIMIT, MAX_DEPTH};

    use crate::{
        Result, config,
        config::tree::sections::pack::{DeltaCacheLimit, DeltaCacheSize, Depth, Window, WindowMemory},
    };

    impl Window {
        /// The amount of objects that the delta search tries as bases of each object, as `value` configures it, which
        /// is 0 for negative values, as in `git pack-objects`.
        pub fn try_into_window(&'static self, value: Result<Option<i64>>) -> Result<Option<usize>> {
            Ok(int(self, value)?.map(|window| usize::try_from(window).unwrap_or(0)))
        }
    }

    impl Depth {
        /// The longest chain of deltas, as `value` configures it, which is 0 for negative values and 4095 for larger
        /// ones, as in `git pack-objects`.
        pub fn try_into_depth(&'static self, value: Result<Option<i64>>) -> Result<Option<u32>> {
            Ok(int(self, value)?.map(|depth| u32::try_from(depth).map_or(0, |depth| depth.min(MAX_DEPTH))))
        }
    }

    impl WindowMemory {
        /// The most bytes that the objects in the window of each thread of the delta search may take, as `value`
        /// configures it, or 0 for no limit. A negative value is an error, as `git pack-objects` reads an unsigned
        /// number, while values of 4 GiB or more are read on every platform.
        pub fn try_into_window_memory(&'static self, value: Result<Option<i64>>) -> Result<Option<u64>> {
            let value = value.or_raise(|| config::key::error(self, "Could not parse an unsigned integer"))?;
            value
                .map(|value| {
                    u64::try_from(value)
                        .or_raise(|| config::key::error_with_value(self, "Could not parse an unsigned integer", value))
                })
                .transpose()
        }
    }

    impl DeltaCacheSize {
        /// The most bytes of deltas that are kept in memory until they are written, as `value` configures it, or 0 for no
        /// limit. Like `git pack-objects`, a negative value is taken as the unsigned number that C converts it to, which
        /// is larger than any amount of deltas.
        pub fn try_into_delta_cache_size(&'static self, value: Result<Option<i64>>) -> Result<Option<u64>> {
            Ok(int(self, value)?.map(unsigned))
        }
    }

    impl DeltaCacheLimit {
        /// The length below which deltas are kept in memory while they fit, as `value` configures it, which is 1 MiB less
        /// one byte for larger values, and so for negative values, which `git pack-objects` takes as the unsigned numbers
        /// that C converts them to.
        pub fn try_into_delta_cache_limit(&'static self, value: Result<Option<i64>>) -> Result<Option<u64>> {
            Ok(int(self, value)?.map(|limit| unsigned(limit).min(MAX_DELTA_CACHE_LIMIT)))
        }
    }

    /// The `int` that Git reads the `value` of `key` as, or an error if `value` is outside of its range, as Git's
    /// `git_config_int()` fails then.
    fn int(key: &'static dyn config::tree::Key, value: Result<Option<i64>>) -> Result<Option<i32>> {
        let Some(value) = value.or_raise(|| config::key::error(key, "Could not parse an integer"))? else {
            return Ok(None);
        };
        i32::try_from(value)
            .map(Some)
            .or_raise(|| config::key::error_with_value(key, "The integer is out of range", value))
    }

    /// `value` converted to an unsigned number of 64 bits, as C converts an `int` to an `unsigned long`.
    fn unsigned(value: i32) -> u64 {
        i64::from(value) as u64
    }
}

mod index_version {
    use gix_error::{ResultExt, bail};

    use crate::{Result, config, config::tree::sections::pack::IndexVersion};

    impl IndexVersion {
        /// Try to interpret an integer value as index version.
        pub fn try_into_index_version(
            &'static self,
            value: Result<Option<i64>>,
        ) -> Result<Option<gix_pack::index::Version>> {
            let Some(value) = value.or_raise(|| config::key::error(self, "Invalid pack index version"))? else {
                return Ok(None);
            };
            Ok(Some(match value {
                1 => gix_pack::index::Version::V1,
                2 => gix_pack::index::Version::V2,
                _ => {
                    bail!(config::key::error_with_value(self, "Invalid pack index version", value));
                }
            }))
        }
    }
}

impl Section for Pack {
    fn name(&self) -> &str {
        "pack"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[
            &Self::THREADS,
            &Self::INDEX_VERSION,
            &Self::COMPRESSION,
            &Self::WINDOW,
            &Self::DEPTH,
            &Self::WINDOW_MEMORY,
            &Self::DELTA_CACHE_SIZE,
            &Self::DELTA_CACHE_LIMIT,
        ]
    }
}

mod validate {
    use crate::{Result, bstr::BStr, config::tree::keys};

    #[derive(Clone, Copy)]
    pub struct IndexVersion;
    impl keys::Validate for IndexVersion {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::INDEX_VERSION
                .try_into_index_version(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct Window;
    impl keys::Validate for Window {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::WINDOW.try_into_window(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct Depth;
    impl keys::Validate for Depth {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::DEPTH.try_into_depth(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct WindowMemory;
    impl keys::Validate for WindowMemory {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::WINDOW_MEMORY
                .try_into_window_memory(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct DeltaCacheSize;
    impl keys::Validate for DeltaCacheSize {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::DELTA_CACHE_SIZE
                .try_into_delta_cache_size(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct DeltaCacheLimit;
    impl keys::Validate for DeltaCacheLimit {
        fn validate(&self, value: &BStr) -> Result {
            super::Pack::DELTA_CACHE_LIMIT
                .try_into_delta_cache_limit(gix_config::Integer::from_bytes::<i64>(value).map(Some))?;
            Ok(())
        }
    }
}
