//! A crate to help setting the worktree to a particular state.
#![deny(missing_docs, unsafe_code)]

///
pub mod checkout;
pub use checkout::function::checkout;

#[cfg(feature = "switch")]
pub mod switch;
#[cfg(feature = "switch")]
pub use switch::function::switch;

#[cfg(feature = "reset")]
pub mod reset;
#[cfg(feature = "reset")]
pub use reset::function::reset;
