//! Stream Deck SDK plugin host (see docs/superpowers/specs/2026-11-12-plugin-host-design.md).
//! Each task adds its own `pub mod` line below.
pub mod catalog;
pub mod host;
#[cfg(test)]
mod host_tests;
pub mod image;
pub mod install;
pub mod link;
pub mod manifest;
pub mod protocol;
pub mod scheme;
pub mod server;
pub mod webview;

/// `%APPDATA%\GerzysDeck\plugins`
pub fn plugins_dir() -> std::path::PathBuf {
    crate::store::data_dir().join("plugins")
}
