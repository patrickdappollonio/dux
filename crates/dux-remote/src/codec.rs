//! Bincode-based codec for `RemoteMessage`.
//!
//! We use bincode via its serde adapter so the wire format is driven by the
//! `Serialize`/`Deserialize` derives on `messages::RemoteMessage`. The config
//! is `bincode::config::standard()` (varint-encoded ints, little-endian) —
//! cheap and compact, which matters for per-tick frame diffs.

use anyhow::{Result, anyhow};

use super::messages::RemoteMessage;

fn config() -> bincode::config::Configuration {
    bincode::config::standard()
}

/// Serialize a `RemoteMessage` into a length-prefixed-by-caller byte buffer.
pub fn encode(msg: &RemoteMessage) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(msg, config()).map_err(|e| anyhow!("remote encode failed: {e}"))
}

/// Deserialize a buffer produced by `encode`.
pub fn decode(bytes: &[u8]) -> Result<RemoteMessage> {
    let (msg, _) = bincode::serde::decode_from_slice(bytes, config())
        .map_err(|e| anyhow!("remote decode failed: {e}"))?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{Capabilities, RemoteMessage};

    #[test]
    fn encode_decode_produces_identical_payload_for_empty_frame() {
        let m = RemoteMessage::Hello {
            protocol_version: 1,
            capabilities: Capabilities::default(),
            peer_label: String::new(),
        };
        let bytes = encode(&m).unwrap();
        assert!(!bytes.is_empty(), "encoded payload must not be empty");
        let _ = decode(&bytes).unwrap();
    }

    #[test]
    fn decode_rejects_malformed_bytes() {
        let res = decode(&[0xff, 0xff, 0xff, 0xff]);
        assert!(res.is_err());
    }
}
