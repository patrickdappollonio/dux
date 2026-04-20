//! Browser-side iroh client for dux remote share.
//!
//! Compiles to `wasm32-unknown-unknown` via wasm-bindgen. The browser loads
//! the generated JS glue + `.wasm` artifact, constructs a [`Session`] from a
//! pairing code, and then drives the session by awaiting `next_message()`
//! and calling `send_input_key()`.
//!
//! The WebSocket handling and all framing live inside iroh — this crate is
//! only responsible for:
//!
//! 1. Parsing the pairing code (via `dux_remote::pairing::decode_pairing_code`).
//! 2. Binding a browser-side iroh `Endpoint` and dialing the host.
//! 3. Running the `dux_remote::handshake::client_handshake` HKDF-PIN proof.
//! 4. Serialising `RemoteMessage` as JSON for the JS layer and the reverse
//!    for outbound input.
//!
//! The wire format (bincode `RemoteMessage` on the iroh stream) is **not**
//! changed for the browser — JSON is only the JS↔WASM boundary.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use dux_remote::codec::{decode, encode};
use dux_remote::endpoint::{bind_client_endpoint, decode_endpoint_addr};
use dux_remote::handshake::client_handshake;
use dux_remote::iroh_transport::{ALPN, IrohTransport};
use dux_remote::messages::{RemoteMessage, WireKeyEvent};
use dux_remote::pairing::decode_pairing_code;
use dux_remote::transport::Transport;
use tokio::sync::Mutex;
use wasm_bindgen::prelude::*;

/// One-shot initialisation — install the panic hook so Rust panics surface
/// as console errors instead of `unreachable executed`.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// Active session between the browser and a dux host.
///
/// Created via [`Session::connect`]. Drop or call [`close`](Self::close) to
/// tear down the iroh connection.
///
/// The `Arc<Mutex<_>>` wrapping the transport lets JS call `next_message`
/// and `send_input_key` concurrently without the Rust side tripping on
/// single-owner borrow rules — each call briefly acquires the lock, drives
/// one send or one recv, and releases.
#[wasm_bindgen]
pub struct Session {
    transport: Arc<Mutex<IrohTransport>>,
    host_label: String,
}

#[wasm_bindgen]
impl Session {
    /// Decode the pairing code, dial the host over iroh's relay mesh,
    /// complete the HKDF-PIN handshake, and return a live `Session`.
    ///
    /// `relay_url` may be `None` or a URL string; `None` uses iroh's public
    /// relay mesh (matches the host's default configuration).
    #[wasm_bindgen]
    pub async fn connect(
        code: String,
        client_label: String,
        relay_url: Option<String>,
    ) -> std::result::Result<Session, JsValue> {
        connect_inner(&code, &client_label, relay_url.as_deref())
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Label the host advertised during handshake. Useful for the browser
    /// UI ("connected to 'home-server'").
    #[wasm_bindgen(getter)]
    pub fn host_label(&self) -> String {
        self.host_label.clone()
    }

    /// Await the next `RemoteMessage` from the host. Resolves to a JSON
    /// string the JS layer can `JSON.parse` and dispatch on `{type}`.
    ///
    /// Rejects (with an error string) when the connection closes, the
    /// framing is malformed, or the JSON serialisation fails.
    #[wasm_bindgen]
    pub async fn next_message(&self) -> std::result::Result<String, JsValue> {
        next_message_inner(&self.transport)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Forward a key event to the host as `RemoteMessage::InputKey`.
    ///
    /// `wire_key_json` is a JSON-serialised [`WireKeyEvent`] — the JS side
    /// builds it from a DOM `KeyboardEvent`.
    #[wasm_bindgen]
    pub async fn send_input_key(&self, wire_key_json: String) -> std::result::Result<(), JsValue> {
        send_input_key_inner(&self.transport, &wire_key_json)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Request the input lead from the host. The host replies asynchronously
    /// with a `LeaderResponse` — callers should then look for that message
    /// on the incoming stream.
    #[wasm_bindgen]
    pub async fn request_lead(&self) -> std::result::Result<(), JsValue> {
        send_one(&self.transport, &RemoteMessage::LeaderRequest)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Send a graceful `Bye` and close the iroh stream. Further calls
    /// return an error.
    #[wasm_bindgen]
    pub async fn close(&self) -> std::result::Result<(), JsValue> {
        close_inner(&self.transport)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }
}

async fn connect_inner(code: &str, client_label: &str, relay_url: Option<&str>) -> Result<Session> {
    let payload = decode_pairing_code(code).context("pairing code is invalid")?;
    let addr = decode_endpoint_addr(&payload.endpoint_addr)
        .context("pairing code contained a malformed endpoint address")?;

    let endpoint = bind_client_endpoint(relay_url).await?;
    let conn = endpoint
        .connect(addr, ALPN)
        .await
        .context("iroh: failed to connect to host")?;
    let mut transport = IrohTransport::dial(conn).await?;

    let host_label = client_handshake(&mut transport, &payload.pin, client_label)
        .await
        .context("handshake rejected by host")?;

    Ok(Session {
        transport: Arc::new(Mutex::new(transport)),
        host_label,
    })
}

async fn next_message_inner(transport: &Arc<Mutex<IrohTransport>>) -> Result<String> {
    let bytes = {
        let mut guard = transport.lock().await;
        guard.recv().await?
    };
    let msg: RemoteMessage = decode(&bytes)?;
    serde_json::to_string(&msg).map_err(|e| anyhow!("remote: serialize outgoing JSON: {e}"))
}

async fn send_input_key_inner(
    transport: &Arc<Mutex<IrohTransport>>,
    wire_key_json: &str,
) -> Result<()> {
    let key: WireKeyEvent = serde_json::from_str(wire_key_json)
        .map_err(|e| anyhow!("remote: bad WireKeyEvent JSON: {e}"))?;
    send_one(transport, &RemoteMessage::InputKey { key }).await
}

async fn send_one(transport: &Arc<Mutex<IrohTransport>>, msg: &RemoteMessage) -> Result<()> {
    let bytes = encode(msg)?;
    let mut guard = transport.lock().await;
    guard.send(&bytes).await
}

async fn close_inner(transport: &Arc<Mutex<IrohTransport>>) -> Result<()> {
    let mut guard = transport.lock().await;
    // Best-effort Bye; swallow send errors — the peer may already be gone.
    let bye = RemoteMessage::Bye {
        reason: dux_remote::messages::ByeReason::Graceful,
    };
    if let Ok(bytes) = encode(&bye)
        && guard.send(&bytes).await.is_err()
    {}
    guard.close().await
}
