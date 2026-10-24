//! Compute the deltas that packs store objects as, and choose the objects they are deltas of, as `git pack-objects`
//! does.
//!
//! A pack stores each object either whole or as a delta of another object, its base: instructions that copy ranges
//! of the base and insert new bytes, as `gitformat-pack(5)` describes them. [`Index`] indexes a base so that the
//! deltas of several objects can be computed against it with [`Index::encode()`], and [`encode()`] computes a single
//! delta. Both write the bytes of Git's `diff-delta.c`. [`search::find_deltas()`] chooses the bases as the delta search
//! of `git pack-objects` does, and documents where it deviates from it.
#![deny(unsafe_code, missing_docs)]

mod encode;
pub use encode::{Index, encode};

pub mod search;
