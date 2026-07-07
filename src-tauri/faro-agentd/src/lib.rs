//! `faro-agentd` — the headless daemon that makes a machine controllable from
//! Faro over an encrypted, paired connection (see `docs/remote-agent.md`).
//!
//! The binary ([`main`](../main.rs)) is a thin CLI over this library; the library
//! form exists so the whole stack — TCP, handshake, pin check, ops — can be
//! integration-tested in-process (see `tests/`).

pub mod config;
pub mod discovery;
pub mod ops;
pub mod server;

pub use config::{config_dir, config_path, identity_path, Config, Policy};
pub use server::{handle_paired, pair_connection, pair_handshake, serve, Daemon, PairingWindow};
