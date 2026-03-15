//! IPC and keybinding infrastructure for jterm.
//!
//! This crate provides:
//! - A JSON-over-Unix-socket IPC protocol ([`protocol`])
//! - A server that dispatches requests to [`jterm_session::SessionManager`] ([`server`])
//! - A client for sending requests from the `jt` CLI or other tools ([`client`])
//! - A 3-layer keybinding system with TOML configuration ([`keybinding`])

pub mod client;
pub mod keybinding;
pub mod protocol;
pub mod server;
