//! Shared domain model for OpenDeckN3.
//!
//! This crate is deliberately free of any hardware or transport code so that
//! drivers, the plugin host, the daemon and (later) the UI backend all speak
//! the same language.

pub mod device;
pub mod profile;

pub use device::*;
pub use profile::*;
