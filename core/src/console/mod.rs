//! Separate-process named-command application console.
//!
//! This module is deliberately separate from Hoplite's privileged development
//! REPL. The evaluator process owns only its language state and a pre-opened
//! named-command broker channel; application source and runtime handles never
//! cross that boundary.

pub mod application_broker;
pub mod dispatcher;
pub mod evaluator;
mod os;
pub mod protocol;
pub mod supervisor;
