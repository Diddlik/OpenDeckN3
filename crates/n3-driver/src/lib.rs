//! Device drivers for OpenDeckN3.
//!
//! Every driver runs as a set of tokio tasks and talks to the daemon only via
//! [`n3_core::DeviceEvent`] (driver → daemon) and [`n3_core::DeviceCommand`]
//! (daemon → driver). Adding a new device family means adding a module here
//! and an entry to [`models::SUPPORTED_MODELS`].

pub mod models;
pub mod n3;
pub mod virtual_deck;
pub mod watcher;

pub use watcher::run_hid_watcher;
