//! herdr-strays — which files strayed from HEAD.
//!
//! The logic lives in a library so integration tests can drive the parser,
//! the diff scope and the editor argv builder directly. The binary is a thin
//! terminal shell over these modules.
//!
//! Read-only by construction: every git invocation in this crate is a query.
//! Nothing here stages, commits, checks out, stashes, or writes to `refs/`.

pub mod agent;
pub mod annotate;
pub mod app;
pub mod config;
pub mod delegate;
pub mod discover;
pub mod editor;
pub mod filter;
pub mod forge;
pub mod git;
pub mod home;
pub mod intraline;
pub mod json;
pub mod marks;
pub mod model;
pub mod pane;
pub mod scan;
pub mod split;
pub mod syntax;
pub mod tree;
pub mod ui;
pub mod watch;
pub mod worktree;
