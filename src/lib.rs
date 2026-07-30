//! herdr-strays — which files strayed from HEAD.
//!
//! The logic lives in a library so integration tests can drive the parser,
//! the diff scope and the editor argv builder directly. The binary is a thin
//! terminal shell over these modules.
//!
//! Read-only by construction: every git invocation in this crate is a query.
//! Nothing here stages, commits, checks out, stashes, or writes to `refs/`.

pub mod agent;
pub mod app;
pub mod discover;
pub mod editor;
pub mod git;
pub mod model;
pub mod tree;
pub mod ui;
pub mod watch;
pub mod worktree;
