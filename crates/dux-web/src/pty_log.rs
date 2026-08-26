//! The log lines a PTY socket writes, as pure formatters.
//!
//! ## Why these live in a module of their own
//!
//! A PTY socket's interesting moments are ownership handovers, refusals,
//! releases and the two sends a stranded client never receives. Every one of
//! them is invisible from the outside: the symptom a user reports is "my
//! terminal is blank" or "my keystrokes do nothing", and the server-side fact
//! that explains it (whose claim won, which send timed out) was previously
//! either unlogged or logged at debug, which nobody has on.
//!
//! Building each line in a pure function keeps its exact text under test, which
//! matters more than it sounds: a log line is a support tool, and one that omits
//! the connection id or the pty it is talking about costs a round trip with the
//! person reporting the bug. There is no tracing and no subscriber here; the
//! project logs through [`dux_core::logger`], whose entry points take `&str`, so
//! these functions return `String` and the call site logs them.

/// Why a resize frame was refused. A small enum rather than a free-text reason
/// so a third refusal cannot be added without the formatter being taught about
/// it: the match below is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeRefusal {
    /// Somebody else owns the pty and the frame did not ask to take over.
    /// Ordinary, expected traffic: a watcher's window really does change size.
    NonOwnerPlainResize,
    /// The frame DID ask to take over, but named an expected predecessor that
    /// no longer owns the pty (an unowned pty included). This is the delayed
    /// ghost-succession frame the compare-and-swap exists to refuse.
    ExpectedOwnerMismatch,
}

/// Which send failed or ran past its deadline, for the reap line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedSend {
    /// The periodic WebSocket Ping that reaps a dead or half-open peer.
    LivenessPing,
    /// The `connected` handshake. A client that never receives it never learns
    /// its own connection id or who owns the pty.
    ConnectedHandshake,
    /// The scrollback replay. A client that never receives it sits in front of
    /// a permanently blank terminal.
    ScrollbackReplay,
}

impl FailedSend {
    fn label(self) -> &'static str {
        match self {
            FailedSend::LivenessPing => "liveness ping",
            FailedSend::ConnectedHandshake => "connected handshake",
            FailedSend::ScrollbackReplay => "scrollback replay",
        }
    }
}

/// How a device is named in a log line when it presented no `User-Agent`.
fn device_label(device: Option<&str>) -> &str {
    device.unwrap_or("no device label")
}

/// A pty changed hands: `conn_id` now owns input and sizing.
pub fn describe_claim_granted(
    pty_id: &str,
    conn_id: u64,
    device: Option<&str>,
    takeover: bool,
) -> String {
    let how = if takeover {
        "an explicit take-over"
    } else {
        "a plain claim of a pty nobody was driving"
    };
    format!(
        "PTY {pty_id}: input and sizing ownership granted to connection {conn_id} \
         ({device}) by {how}",
        device = device_label(device)
    )
}

/// A resize frame was refused, naming who does own the pty and why the frame
/// lost.
pub fn describe_claim_refused(
    pty_id: &str,
    conn_id: u64,
    current_owner: Option<u64>,
    reason: ResizeRefusal,
) -> String {
    let owner = match current_owner {
        Some(id) => format!("connection {id} currently owns it"),
        None => "nobody currently owns it".to_string(),
    };
    let why = match reason {
        ResizeRefusal::NonOwnerPlainResize => {
            "the frame did not ask to take over, and attaching never steals"
        }
        ResizeRefusal::ExpectedOwnerMismatch => {
            "the take-over named a predecessor that no longer owns the pty, so it \
             was superseded before it arrived"
        }
    };
    format!(
        "PTY {pty_id}: resize from connection {conn_id} refused whole, nothing applied \
         and nothing broadcast; {owner} and {why}"
    )
}

/// A socket ended and gave the pty back.
pub fn describe_ownership_released(pty_id: &str, conn_id: u64, epoch: u64) -> String {
    format!(
        "PTY {pty_id}: connection {conn_id} released input and sizing ownership at \
         epoch {epoch} as its socket closed"
    )
}

/// A connection is being torn down because a send failed or ran past its
/// deadline.
pub fn describe_connection_reaped(conn_id: u64, failed: FailedSend) -> String {
    format!(
        "PTY connection {conn_id} reaped: its {failed} send failed or ran past the \
         send deadline, so the socket is torn down and its pty ownership released",
        failed = failed.label()
    )
}

/// The scrollback replay reached the client (debug: one line per socket open).
pub fn describe_replay_sent(pty_id: &str, generation: u64, bytes: usize) -> String {
    format!("PTY {pty_id}: replay generation {generation} sent, {bytes} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_granted_claim_names_the_pty_the_connection_the_device_and_how_it_won() {
        assert_eq!(
            describe_claim_granted("s1", 7, Some("Chrome UA"), true),
            "PTY s1: input and sizing ownership granted to connection 7 (Chrome UA) \
             by an explicit take-over"
        );
        assert_eq!(
            describe_claim_granted("term-2", 0, None, false),
            "PTY term-2: input and sizing ownership granted to connection 0 \
             (no device label) by a plain claim of a pty nobody was driving"
        );
    }

    #[test]
    fn a_refused_resize_names_the_current_owner_and_the_reason() {
        assert_eq!(
            describe_claim_refused("s1", 9, Some(4), ResizeRefusal::NonOwnerPlainResize),
            "PTY s1: resize from connection 9 refused whole, nothing applied and \
             nothing broadcast; connection 4 currently owns it and the frame did not \
             ask to take over, and attaching never steals"
        );
        assert_eq!(
            describe_claim_refused("s1", 9, Some(4), ResizeRefusal::ExpectedOwnerMismatch),
            "PTY s1: resize from connection 9 refused whole, nothing applied and \
             nothing broadcast; connection 4 currently owns it and the take-over named \
             a predecessor that no longer owns the pty, so it was superseded before it \
             arrived"
        );
        assert_eq!(
            describe_claim_refused("s1", 9, None, ResizeRefusal::ExpectedOwnerMismatch),
            "PTY s1: resize from connection 9 refused whole, nothing applied and \
             nothing broadcast; nobody currently owns it and the take-over named a \
             predecessor that no longer owns the pty, so it was superseded before it \
             arrived"
        );
    }

    #[test]
    fn a_release_names_the_pty_the_connection_and_the_epoch() {
        assert_eq!(
            describe_ownership_released("term-1", 3, 12),
            "PTY term-1: connection 3 released input and sizing ownership at epoch 12 \
             as its socket closed"
        );
    }

    #[test]
    fn a_reap_names_the_connection_and_which_send_failed() {
        assert_eq!(
            describe_connection_reaped(5, FailedSend::LivenessPing),
            "PTY connection 5 reaped: its liveness ping send failed or ran past the \
             send deadline, so the socket is torn down and its pty ownership released"
        );
        assert_eq!(
            describe_connection_reaped(5, FailedSend::ConnectedHandshake),
            "PTY connection 5 reaped: its connected handshake send failed or ran past \
             the send deadline, so the socket is torn down and its pty ownership released"
        );
        assert_eq!(
            describe_connection_reaped(5, FailedSend::ScrollbackReplay),
            "PTY connection 5 reaped: its scrollback replay send failed or ran past the \
             send deadline, so the socket is torn down and its pty ownership released"
        );
    }

    #[test]
    fn a_replay_line_names_the_generation_and_the_byte_count() {
        assert_eq!(
            describe_replay_sent("s1", 4, 2048),
            "PTY s1: replay generation 4 sent, 2048 bytes"
        );
    }
}
