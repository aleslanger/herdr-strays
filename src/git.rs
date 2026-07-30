//! Read-only git access.
//!
//! Every command issued from these modules is a query. There is no write path.

pub mod diff;
pub mod run;
pub mod status;
