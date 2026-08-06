#![forbid(unsafe_code)]

//! Application-neutral data-plane contracts for Hoplite.
//!
//! Large request and response bytes remain outside Hara values. Applications
//! receive bounded streaming handles and an allowlisted application identity
//! projection instead of filesystem paths, upstream URLs, bearer sessions, or
//! administrator credentials.

mod auth;
mod body;
mod range;

pub use auth::*;
pub use body::*;
pub use range::*;
