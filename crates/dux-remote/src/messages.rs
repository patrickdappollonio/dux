//! Wire-level message types exchanged between host and client.
//!
//! All types in this file are `Serialize + Deserialize` via serde and flow
//! through `bincode` on the wire. Keep them small and stable — any change
//! that alters layout must bump `PROTOCOL_VERSION` in `super`.
//!
//! None of the wire types reach outside this crate for crossterm — browser
//! clients compile without crossterm and construct [`WireKeyEvent`] values
//! directly from DOM keyboard events.

use serde::{Deserialize, Serialize};

/// A crossterm-free wire representation of a single key event.
///
/// The host translates to/from `crossterm::event::KeyEvent` at the edge of
/// the remote-share subsystem (see `dux-remote/src/key_translate.rs` behind
/// `feature = "host"`). Browser clients build these directly from DOM
/// `KeyboardEvent`s and never see a crossterm type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireKeyEvent {
    pub code: WireKeyCode,
    /// Bitmask — `SHIFT=0x01, CONTROL=0x02, ALT=0x04, SUPER=0x08, HYPER=0x10, META=0x20`.
    /// Matches crossterm's `KeyModifiers::bits()` layout so round-tripping is lossless.
    pub modifiers: u8,
    pub kind: WireKeyKind,
}

/// Wire-safe key-code enum. Uses serde's default externally tagged
/// representation so bincode can round-trip it: unit variants encode as
/// strings (`"Backspace"`), data variants as single-key objects
/// (`{"Char":"a"}`, `{"F":9}`). The browser dispatches on either shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireKeyCode {
    Char(char),
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    KeypadBegin,
    F(u8),
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireKeyKind {
    Press,
    Repeat,
    Release,
}

/// Which side is currently driving input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Leader {
    /// The host holds the lead; any keystrokes the client sends are dropped.
    Host,
    /// The client holds the lead; the host is view-only.
    Client,
}

/// Color values carried on the wire. Mirrors the variants `ratatui::Color`
/// supports that we actually render, expressed as a shape that serde can
/// roundtrip without relying on ratatui's (non-serde) types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

/// A single styled cell, addressed by absolute row/column.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireCell {
    pub row: u16,
    pub col: u16,
    pub symbol: String,
    pub fg: WireColor,
    pub bg: WireColor,
    /// Packed `ratatui::Modifier` bitmask.
    pub modifier: u16,
}

/// Capabilities advertised in `Hello`. A forward-compatible extension point.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Host can accept remote input at all (respects `[remote].allow_remote_input`).
    pub accepts_input: bool,
    /// Host will emit PTY-snapshot streams in addition to chrome diffs.
    pub streams_pty: bool,
}

/// Reason a peer is dropping the connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ByeReason {
    Graceful,
    ProtocolMismatch { expected: u16, got: u16 },
    AuthFailed,
    InputNotAllowed,
    InternalError(String),
}

/// Every message exchanged over the wire. Bidirectional; not every variant
/// is sent by both peers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteMessage {
    /// First message each peer sends. Carries protocol version + capabilities.
    Hello {
        protocol_version: u16,
        capabilities: Capabilities,
        peer_label: String,
    },

    /// Full keyframe — sent on connect/reconnect or when the client asks for
    /// resync. Contains every cell on the viewport.
    FullFrame {
        seq: u64,
        cols: u16,
        rows: u16,
        cells: Vec<WireCell>,
    },

    /// Incremental frame update from the `TeeBackend` cell diff.
    FrameDiff { seq: u64, cells: Vec<WireCell> },

    /// PTY grid diff for the currently focused pane, streamed in parallel with
    /// chrome `FrameDiff` so the remote sees smooth terminal output in
    /// interactive mode.
    PtySnapshotDiff {
        seq: u64,
        pane_id: String,
        cols: u16,
        rows: u16,
        cells: Vec<WireCell>,
    },

    /// Client → host: raw byte stream. Reserved for future use (e.g.
    /// paste events that don't map cleanly to a single `KeyEvent`).
    /// Host drops these when it is the leader or when
    /// `allow_remote_input = false`.
    Input { bytes: Vec<u8> },

    /// Client → host: a single structured key press. Funnelled through
    /// the host's normal input dispatch (`handle_key`). Preferred over
    /// `Input { bytes }` for regular typing because key code / modifier
    /// semantics survive unchanged.
    ///
    /// The wire type is [`WireKeyEvent`], not `crossterm::event::KeyEvent`,
    /// so browser clients (which cannot depend on crossterm) can construct
    /// key events directly. The host translates on ingress.
    InputKey { key: WireKeyEvent },

    /// Host → both: the leader just changed. Both sides update their UI so
    /// the user knows whose keystrokes will land.
    LeaderChange { leader: Leader },

    /// Client → host: please let me drive. Host answers with
    /// `LeaderResponse`.
    LeaderRequest,

    /// Host → client: response to `LeaderRequest`.
    LeaderResponse { granted: bool },

    /// Host → client: viewport dimensions changed. Client letterboxes or
    /// scales to fit; it never resizes the host.
    Resize { cols: u16, rows: u16 },

    /// Keepalive / RTT probe.
    Ping { nonce: u64 },
    /// Keepalive reply.
    Pong { nonce: u64 },

    /// Connection is closing.
    Bye { reason: ByeReason },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, encode};

    fn roundtrip(msg: RemoteMessage) -> RemoteMessage {
        let bytes = encode(&msg).expect("encode");
        decode(&bytes).expect("decode")
    }

    #[test]
    fn hello_roundtrip() {
        let m = RemoteMessage::Hello {
            protocol_version: 1,
            capabilities: Capabilities {
                accepts_input: true,
                streams_pty: true,
            },
            peer_label: "laptop-kitchen".to_string(),
        };
        match roundtrip(m) {
            RemoteMessage::Hello {
                protocol_version,
                capabilities,
                peer_label,
            } => {
                assert_eq!(protocol_version, 1);
                assert!(capabilities.accepts_input);
                assert!(capabilities.streams_pty);
                assert_eq!(peer_label, "laptop-kitchen");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn frame_diff_roundtrip_preserves_cells() {
        let cells = vec![
            WireCell {
                row: 0,
                col: 0,
                symbol: "h".into(),
                fg: WireColor::White,
                bg: WireColor::Reset,
                modifier: 0,
            },
            WireCell {
                row: 0,
                col: 1,
                symbol: "i".into(),
                fg: WireColor::Rgb(255, 128, 0),
                bg: WireColor::Indexed(42),
                modifier: 0b1,
            },
        ];
        let m = RemoteMessage::FrameDiff {
            seq: 7,
            cells: cells.clone(),
        };
        match roundtrip(m) {
            RemoteMessage::FrameDiff {
                seq,
                cells: decoded,
            } => {
                assert_eq!(seq, 7);
                assert_eq!(decoded.len(), 2);
                assert_eq!(decoded[0].symbol, "h");
                assert_eq!(decoded[1].fg, WireColor::Rgb(255, 128, 0));
                assert_eq!(decoded[1].bg, WireColor::Indexed(42));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn input_roundtrip() {
        let m = RemoteMessage::Input {
            bytes: b"hello\x1b[A".to_vec(),
        };
        match roundtrip(m) {
            RemoteMessage::Input { bytes } => assert_eq!(bytes, b"hello\x1b[A"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn input_key_roundtrip_preserves_char_and_modifiers() {
        let key = WireKeyEvent {
            code: WireKeyCode::Char('c'),
            modifiers: 0x02, // CONTROL
            kind: WireKeyKind::Press,
        };
        let m = RemoteMessage::InputKey { key: key.clone() };
        match roundtrip(m) {
            RemoteMessage::InputKey { key: decoded } => assert_eq!(decoded, key),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn input_key_roundtrip_function_key() {
        let key = WireKeyEvent {
            code: WireKeyCode::F(9),
            modifiers: 0,
            kind: WireKeyKind::Press,
        };
        let m = RemoteMessage::InputKey { key: key.clone() };
        match roundtrip(m) {
            RemoteMessage::InputKey { key: decoded } => assert_eq!(decoded, key),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn leader_variants_roundtrip() {
        for leader in [Leader::Host, Leader::Client] {
            let m = RemoteMessage::LeaderChange { leader };
            match roundtrip(m) {
                RemoteMessage::LeaderChange { leader: decoded } => assert_eq!(decoded, leader),
                other => panic!("unexpected variant: {other:?}"),
            }
        }
    }

    #[test]
    fn bye_reason_roundtrip() {
        let m = RemoteMessage::Bye {
            reason: ByeReason::ProtocolMismatch {
                expected: 1,
                got: 2,
            },
        };
        match roundtrip(m) {
            RemoteMessage::Bye { reason } => match reason {
                ByeReason::ProtocolMismatch { expected, got } => {
                    assert_eq!(expected, 1);
                    assert_eq!(got, 2);
                }
                other => panic!("unexpected reason: {other:?}"),
            },
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
