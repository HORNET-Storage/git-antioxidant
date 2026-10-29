//! The todo list and the state of operations that stop for the user and continue later, as Git's sequencer keeps them
//! for `git rebase`, `git cherry-pick` and `git revert`.
//!
//! * [`todo`](mod@todo) parses and writes todo lists as `sequencer.c` does, with the help that Git appends for the
//!   editor and the checks after editing them.
//! * [`rebase`] reads and writes the state of a rebase in `.git/rebase-merge`, and takes the steps of `continue`, `skip`,
//!   `abort` and `quit` on it.
//! * [`sequence`] reads and writes the state of a cherry-pick or revert sequence in `.git/sequencer`, and takes the same
//!   steps on it.
//! * [`refs`] describes the changes to `REBASE_HEAD`, `ORIG_HEAD`, `CHERRY_PICK_HEAD` and the other pseudo-references
//!   that Git makes along the way, for the caller to apply with the references of the repository.
//!
//! The crate decides what comes next and how the state changes. Making commits, merging, resetting the index and the
//! worktree and running hooks or `exec` commands are left to the caller.
#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod rebase;
pub mod refs;
pub mod sequence;
pub mod todo;

mod file;
