//! Shared transport + protocol crate for dux remote share.
//!
//! This crate compiles for both the native host (`dux` TUI binary) and the
//! browser (`dux-web-browser` wasm-bindgen crate). Host-only code — e.g.
//! `std::time::Instant`-based TTL tracking, iroh endpoint binding, tokio
//! runtime primitives — sits behind `#[cfg(feature = "host")]`. The browser
//! build pulls a narrower surface gated by `feature = "wasm"`.
//!
//! The wire format is bincode; the rendering layer above reads JSON on the
//! JS↔WASM boundary only. Nothing in this crate knows JSON exists.

pub mod codec;
pub mod endpoint;
pub mod handshake;
pub mod iroh_transport;
#[cfg(feature = "host")]
pub mod key_translate;
pub mod messages;
pub mod pairing;
pub mod transport;

/// Protocol version advertised in `RemoteMessage::Hello`. Bump on any wire
/// format change. Host and client compare on connect and emit
/// `ByeReason::ProtocolMismatch` if they disagree.
///
/// - v1: initial iroh-based release (PR #186).
/// - v2: `RemoteMessage::InputKey` carries `WireKeyEvent` instead of
///   `crossterm::event::KeyEvent`. Wire-incompatible with v1.
pub const PROTOCOL_VERSION: u16 = 2;
