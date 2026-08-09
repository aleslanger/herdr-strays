//! Read-only git access.
//!
//! Every command issued from these modules is a query. There is no write path.

pub mod base;
pub mod blame;
pub mod branch;
pub mod diff;
pub mod graph;
pub mod history;
pub mod run;
pub mod stash;
pub mod status;
