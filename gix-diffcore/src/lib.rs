//! Find renames among the changes between two trees as git's diffcore does.
//!
//! `git diff -M` turns pairs of a deleted and an added file into renames. [`rename::detect()`] does that as
//! `diffcore-rename.c` and `diffcore-delta.c` do, so that it pairs the same files with the same similarity as git, and
//! [`rename::merge::detect()`] does it for one side of a merge as merge-ort has diffcore-rename do it. The caller
//! provides the deleted and added files, which a diff of the two trees such as that of `gix-diff` yields, and the
//! blobs whose similarity is estimated, for example from an object database. [`strmap::iteration_order()`] gives the
//! order in which git's string-keyed hash maps visit their entries, on which some of git's choices between equally
//! good candidates depend.
#![deny(unsafe_code, missing_docs)]

pub mod rename;
mod similarity;
pub mod strmap;
