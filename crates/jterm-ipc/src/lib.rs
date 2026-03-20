//! IPC and keybinding infrastructure for jterm.
//!
//! This crate provides:
//! - A JSON-over-Unix-socket IPC protocol ([`protocol`])
//! - A server that dispatches requests to [`jterm_session::SessionManager`] ([`server`])
//! - A client for sending requests from the `jt` CLI or other tools ([`client`])
//! - A 3-layer keybinding system with TOML configuration ([`keybinding`])
//! - A stdio JSON protocol for external command plugins ([`command_protocol`])
//! - Command discovery and metadata loading ([`command_loader`])
//! - Command signing and verification ([`command_signer`])
//! - A non-blocking command execution engine ([`command_runner`])

pub mod client;
pub mod command_loader;
pub mod command_protocol;
pub mod command_signer;
pub mod command_runner;
pub mod keybinding;
pub mod protocol;
pub mod server;
