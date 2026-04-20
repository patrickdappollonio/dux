//! Pairing code + HKDF-PIN handshake utilities.
//!
//! The pairing flow is deliberately minimal and transport-independent:
//!
//! 1. Host generates a `PairingSecret` (random 16-byte PIN) and binds an
//!    iroh `Endpoint`. The host's `EndpointAddr` + PIN are encoded together
//!    as a `PairingCode` — one user-copyable string.
//! 2. Client decodes the code, extracts the `EndpointAddr` and PIN, dials
//!    the host, and opens a bidirectional QUIC stream.
//! 3. Client sends an HKDF-derived proof: it hashes the PIN with a nonce
//!    the host supplied, and sends the tag. Host verifies by recomputing.
//!    The PIN itself never crosses the wire.
//! 4. Host invalidates the code once accepted — single-use.
//!
//! iroh's QUIC connection is already authenticated and end-to-end encrypted
//! (it's keyed by the host's iroh identity). This layer exists to prove the
//! client also knows the out-of-band PIN that the host just displayed, which
//! is how we prevent "any iroh peer that learns the endpoint id can connect".
//!
//! The handshake lives in its own module so it can be unit-tested without
//! standing up a real iroh endpoint.

#[cfg(feature = "host")]
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Length in bytes of the secret PIN baked into each pairing code.
///
/// 16 bytes (128 bits) is enough entropy to make online guessing infeasible
/// while keeping the base32-encoded pairing code short enough to copy.
pub const PIN_LEN: usize = 16;

/// Length in bytes of the handshake nonce. The host sends this to the
/// client; the client mixes it into HKDF so a passive observer who learns
/// the tag can't replay it to a different server.
pub const NONCE_LEN: usize = 16;

/// Length in bytes of the HKDF-derived proof tag the client sends.
pub const TAG_LEN: usize = 32;

/// Length in bytes of the session key both sides derive after a successful
/// handshake. Currently unused beyond the handshake — iroh's QUIC provides
/// transport encryption — but it exists so future phases (or a non-iroh
/// transport that exposes a raw byte pipe) can wrap the payload in AEAD.
#[allow(dead_code)]
pub const SESSION_KEY_LEN: usize = 32;

/// HKDF "info" string identifying the proof computation. Stable; do not
/// change without bumping `PROTOCOL_VERSION`.
const INFO_PROOF: &[u8] = b"dux/remote/proof/v1";
/// HKDF "info" string for the session key derivation.
#[allow(dead_code)]
const INFO_SESSION: &[u8] = b"dux/remote/session-key/v1";

/// The secret bundled in every pairing code. Host generates it once per
/// share and keeps it in memory until the code expires or is consumed.
///
/// Host-only: the type uses `std::time::Instant`, which is unavailable on
/// `wasm32-unknown-unknown`. Browser clients never construct a
/// `PairingSecret`; they receive a `PairingCodePayload` containing just the
/// PIN bytes and don't track TTL themselves.
#[cfg(feature = "host")]
#[derive(Clone)]
pub struct PairingSecret {
    pub pin: [u8; PIN_LEN],
    pub created_at: Instant,
    pub ttl: Duration,
}

#[cfg(feature = "host")]
impl PairingSecret {
    /// Generate a fresh secret using the OS RNG.
    pub fn generate(ttl: Duration) -> Self {
        let mut pin = [0u8; PIN_LEN];
        rand::rng().fill_bytes(&mut pin);
        Self {
            pin,
            created_at: Instant::now(),
            ttl,
        }
    }

    /// Has this secret outlived its TTL?
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }

    /// Whole seconds remaining on the TTL (rounded down). Returns 0 if
    /// already expired.
    pub fn seconds_remaining(&self) -> u64 {
        self.ttl.saturating_sub(self.created_at.elapsed()).as_secs()
    }
}

/// Plaintext shape of a pairing code before encoding. Serialized via
/// bincode into bytes then base32-encoded to a user-copyable string.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingCodePayload {
    /// Serialized iroh `EndpointAddr`. We carry bytes rather than the
    /// typed struct so this module stays transport-agnostic.
    pub endpoint_addr: Vec<u8>,
    /// Shared PIN.
    pub pin: [u8; PIN_LEN],
}

/// Encode a pairing payload to a human-friendly copy-paste string.
pub fn encode_pairing_code(payload: &PairingCodePayload) -> Result<String> {
    let raw = bincode::serde::encode_to_vec(payload, bincode::config::standard())
        .map_err(|e| anyhow!("pairing encode failed: {e}"))?;
    Ok(data_encoding::BASE32_NOPAD.encode(&raw))
}

/// Decode a user-entered pairing code back into the raw payload. Trims
/// whitespace and is case-insensitive — users will copy these from modals
/// and chat windows where casing isn't guaranteed.
pub fn decode_pairing_code(s: &str) -> Result<PairingCodePayload> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let raw = data_encoding::BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .context("pairing code is not valid base32")?;
    let (payload, _) = bincode::serde::decode_from_slice(&raw, bincode::config::standard())
        .map_err(|e| anyhow!("pairing code decode failed: {e}"))?;
    Ok(payload)
}

/// Compute the HKDF-SHA256 proof tag: the client sends this to prove it
/// knows the PIN.
///
/// `PRK = HKDF-Extract(salt=nonce, ikm=pin)`
/// `tag = HKDF-Expand(PRK, info="dux/remote/proof/v1", len=32)`
pub fn derive_proof_tag(pin: &[u8; PIN_LEN], nonce: &[u8; NONCE_LEN]) -> [u8; TAG_LEN] {
    let hk = Hkdf::<Sha256>::new(Some(nonce), pin);
    let mut out = [0u8; TAG_LEN];
    hk.expand(INFO_PROOF, &mut out)
        .expect("HKDF expand of 32 bytes cannot fail");
    out
}

/// Derive a session key both sides can use for AEAD if a future transport
/// needs it. iroh's QUIC session makes this optional today, but computing
/// it during the handshake is free and keeps the door open.
#[allow(dead_code)]
pub fn derive_session_key(pin: &[u8; PIN_LEN], nonce: &[u8; NONCE_LEN]) -> [u8; SESSION_KEY_LEN] {
    let hk = Hkdf::<Sha256>::new(Some(nonce), pin);
    let mut out = [0u8; SESSION_KEY_LEN];
    hk.expand(INFO_SESSION, &mut out)
        .expect("HKDF expand of 32 bytes cannot fail");
    out
}

/// Generate a fresh 16-byte nonce using the OS RNG. Host side — the host
/// sends this to the client at handshake start, and the client mixes it
/// into their proof so the exchange can't be replayed against a different
/// session.
pub fn generate_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Constant-time comparison of two byte slices. Required when verifying
/// MAC-like values: a timing-variable compare leaks how many leading bytes
/// matched.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Host-side rate limiter for handshake attempts against a single code.
///
/// Currently **not used** by the active code path: `serve_host_session`
/// consumes `endpoint.accept()` exactly once and drops the endpoint
/// afterwards, which enforces a stricter "one attempt per code" bound
/// than any window-based limiter could. The type is kept for a future
/// iteration where the endpoint stays alive across failed handshakes —
/// at which point wire it into `host_handshake` before accepting the
/// `ClientAuth` proof.
///
/// Host-only (uses `std::time::Instant`).
#[cfg(feature = "host")]
#[allow(dead_code)]
pub struct AttemptLimiter {
    window: Duration,
    max_failures: u32,
    failures: Vec<Instant>,
}

#[cfg(feature = "host")]
#[allow(dead_code)]
impl AttemptLimiter {
    pub fn new(window: Duration, max_failures: u32) -> Self {
        Self {
            window,
            max_failures,
            failures: Vec::new(),
        }
    }

    /// Record a failed attempt and return `true` if the limiter now denies
    /// further attempts.
    pub fn record_failure(&mut self) -> bool {
        let now = Instant::now();
        self.failures
            .retain(|t| now.duration_since(*t) < self.window);
        self.failures.push(now);
        self.failures.len() > self.max_failures as usize
    }

    /// Whether further attempts are currently denied.
    pub fn is_blocked(&mut self) -> bool {
        let now = Instant::now();
        self.failures
            .retain(|t| now.duration_since(*t) < self.window);
        self.failures.len() > self.max_failures as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_roundtrip() {
        let payload = PairingCodePayload {
            endpoint_addr: vec![1, 2, 3, 4, 5],
            pin: [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
                0x0f, 0x10,
            ],
        };
        let encoded = encode_pairing_code(&payload).unwrap();
        assert!(
            encoded.chars().all(|c| c.is_ascii_alphanumeric()),
            "pairing code should be base32 alphanumeric: {encoded}"
        );
        let decoded = decode_pairing_code(&encoded).unwrap();
        assert_eq!(payload, decoded);
    }

    #[test]
    fn pairing_code_decode_is_case_insensitive_and_whitespace_tolerant() {
        let payload = PairingCodePayload {
            endpoint_addr: vec![9, 9, 9],
            pin: [0xab; PIN_LEN],
        };
        let encoded = encode_pairing_code(&payload).unwrap();
        let messy = format!(" {}  {}  ", encoded.to_lowercase(), "");
        let decoded = decode_pairing_code(&messy).unwrap();
        assert_eq!(payload, decoded);
    }

    #[test]
    fn pairing_code_rejects_garbage() {
        let res = decode_pairing_code("not-base32-$$$$");
        assert!(res.is_err());
    }

    #[test]
    fn proof_tag_is_deterministic_for_same_inputs() {
        let pin = [0x11; PIN_LEN];
        let nonce = [0x22; NONCE_LEN];
        assert_eq!(
            derive_proof_tag(&pin, &nonce),
            derive_proof_tag(&pin, &nonce)
        );
    }

    #[test]
    fn proof_tag_changes_with_pin() {
        let nonce = [0x33; NONCE_LEN];
        let a = derive_proof_tag(&[0x01; PIN_LEN], &nonce);
        let b = derive_proof_tag(&[0x02; PIN_LEN], &nonce);
        assert_ne!(a, b);
    }

    #[test]
    fn proof_tag_changes_with_nonce() {
        let pin = [0x44; PIN_LEN];
        let a = derive_proof_tag(&pin, &[0x01; NONCE_LEN]);
        let b = derive_proof_tag(&pin, &[0x02; NONCE_LEN]);
        assert_ne!(a, b);
    }

    #[test]
    fn session_key_differs_from_proof_tag() {
        let pin = [0x55; PIN_LEN];
        let nonce = [0x66; NONCE_LEN];
        // Different `info` strings must yield different outputs.
        assert_ne!(
            &derive_proof_tag(&pin, &nonce)[..],
            &derive_session_key(&pin, &nonce)[..]
        );
    }

    #[test]
    fn ct_eq_is_value_equal_but_length_sensitive() {
        assert!(ct_eq(&[1, 2, 3], &[1, 2, 3]));
        assert!(!ct_eq(&[1, 2, 3], &[1, 2, 4]));
        assert!(!ct_eq(&[1, 2, 3], &[1, 2]));
        assert!(ct_eq(&[], &[]));
    }

    #[cfg(feature = "host")]
    #[test]
    fn secret_expires_past_ttl() {
        let secret = PairingSecret {
            pin: [0; PIN_LEN],
            created_at: Instant::now() - Duration::from_secs(10),
            ttl: Duration::from_secs(5),
        };
        assert!(secret.is_expired());
        assert_eq!(secret.seconds_remaining(), 0);

        let fresh = PairingSecret::generate(Duration::from_secs(120));
        assert!(!fresh.is_expired());
        assert!(fresh.seconds_remaining() > 0);
    }

    #[cfg(feature = "host")]
    #[test]
    fn attempt_limiter_blocks_after_threshold() {
        let mut limiter = AttemptLimiter::new(Duration::from_secs(60), 3);
        assert!(!limiter.is_blocked());
        // Three failures are still within the threshold (threshold is "more
        // than max_failures").
        for _ in 0..3 {
            assert!(!limiter.record_failure());
        }
        // The fourth pushes us over.
        assert!(limiter.record_failure());
        assert!(limiter.is_blocked());
    }

    #[cfg(feature = "host")]
    #[test]
    fn attempt_limiter_forgets_after_window() {
        let mut limiter = AttemptLimiter::new(Duration::from_millis(50), 1);
        limiter.record_failure();
        limiter.record_failure(); // over threshold
        assert!(limiter.is_blocked());
        std::thread::sleep(Duration::from_millis(80));
        assert!(!limiter.is_blocked());
    }
}
