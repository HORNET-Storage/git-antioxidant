//! The fixtures record what `git diff -M` reports for the trees of each of their cases, which the renames that this
//! crate finds are compared with. The renames of a side of a merge are found among files kept in memory, and the order
//! of git's hash maps is compared with the order in which git visits them.

mod rename;
mod strmap;
