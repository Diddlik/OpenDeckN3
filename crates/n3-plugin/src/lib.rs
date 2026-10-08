//! Plugin system of OpenDeckN3.
//!
//! Plugins are separate processes that connect back to the host over a local
//! WebSocket and speak a subset of the Elgato Stream Deck SDK protocol (the
//! same one OpenDeck/OpenAction use). This keeps existing Stream Deck plugins
//! usable and lets plugins be written in any language.

pub mod host;
pub mod manifest;
pub mod protocol;

pub use host::{InstalledPlugin, PluginEvent, PluginHost, PluginMessage};
pub use manifest::{ActionManifest, PluginManifest};
pub use protocol::InboundEvent;

/// Pseudo plugin UUID for actions implemented inside the daemon.
pub const BUILTIN_PLUGIN: &str = "opendeckn3.builtin";
