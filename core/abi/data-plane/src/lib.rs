#![forbid(unsafe_code)]

//! Application-neutral data-plane contracts for Hoplite.
//!
//! Large request and response bytes remain outside Hara values. Applications
//! receive bounded streaming handles instead of filesystem paths, upstream
//! URLs, bearer sessions, or administrator credentials.

mod body;
mod range;
mod resource;

pub use body::*;
pub use range::*;
pub use resource::*;
