//! Bidirectional byte transport between host and client.
//!
//! Implementations shuttle bincode-encoded `RemoteMessage` bytes across the
//! wire. The trait is deliberately minimal — send / recv / close — so the
//! iroh implementation (Phase 4) and future backends (in-memory test pair,
//! browser WebSocket) slot in cleanly.
//!
//! The trait uses `async fn` directly (Rust 2024 edition supports it in
//! traits). Calls from synchronous code go through the tokio worker in
//! `app::workers`, which owns the runtime.

use anyhow::Result;

/// A framed, bidirectional byte transport.
///
/// Implementations are responsible for framing — a `recv()` call returns one
/// complete message payload, not a raw byte stream.
#[allow(async_fn_in_trait)]
pub trait Transport: Send + 'static {
    /// Send one framed payload. Each call corresponds to one `recv()` on the
    /// peer.
    async fn send(&mut self, bytes: &[u8]) -> Result<()>;

    /// Receive one framed payload. Blocks (asynchronously) until a full
    /// message arrives or the connection closes.
    async fn recv(&mut self) -> Result<Vec<u8>>;

    /// Shut the transport down cleanly.
    async fn close(&mut self) -> Result<()>;
}
