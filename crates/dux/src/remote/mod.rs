//! Remote share subsystem.
//!
//! The wire format, transport abstraction, pairing, and handshake all live in
//! the sibling `dux-remote` crate so they can compile for both the native
//! TUI and a wasm32 browser build. This module keeps the host-only pieces
//! — the ratatui-backed `TeeBackend`, the in-memory `HeadlessBackend`, the
//! server lifecycle, and the native client renderer — and re-exports the
//! shared protocol types so existing call sites `use crate::remote::X` keep
//! working.

pub mod client;
pub mod headless_backend;
pub mod server;
pub mod tee_backend;
pub mod worker;

pub use client::run_client;
pub use headless_backend::HeadlessBackend;
pub use tee_backend::{CaptureEvent, TeeBackend};
pub use worker::{RemoteCommand, RemoteWorker};

pub use dux_remote::{
    PROTOCOL_VERSION, codec, endpoint, handshake, iroh_transport, messages, pairing, transport,
};
