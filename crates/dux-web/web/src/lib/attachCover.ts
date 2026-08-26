// WHAT COVERS THE TERMINAL, decided once, in one pure function.
//
// The bug this replaces was a hole between two conditions rather than a wrong
// condition. The pane cleared its reconnect cue at WebSocket OPEN, but the
// picture only exists once the server's replay frame has been PARSED, and
// between those two moments it drew nothing at all: no card, no spinner, no
// Reconnect box, no compose bar. Measured in the preview container, the blank
// ran from 113ms to the end of a 40 second run whenever the socket opened and
// stayed healthy but the binary replay never landed.
//
// So the cover clears on the APPLIED replay and never on the socket opening,
// and the exhaustiveness test beside this file pins the property that makes the
// blank unreachable: this function never answers `none` while the replay for the
// current attach epoch is unapplied.
//
// A wait with no answer is bounded rather than blank. Past `replay_wait_seconds`
// of VISIBLE time (see `visibleClock.ts` for why visible and not wall) the
// spinner becomes a Reconnect box that says what is missing.
import type { ConnState } from "./types"

/// Everything the decision reads. Each is a fact somebody else owns: the socket
/// state from `ReconnectingSocket`, the applied flag from the attach-replay
/// machine's current epoch, ownership from the ownership machine, `offline` from
/// the events socket, and the expiry from this pane's replay clock.
export type AttachCoverInputs = {
  socket: ConnState
  /// The replay for the CURRENT attach epoch has been written AND parsed. A
  /// superseded epoch's applied signal is not this flag.
  replayApplied: boolean
  /// The pty has produced output at some point (latched off the spine).
  everReady: boolean
  /// The app-wide events socket is down, so `OfflineOverlay` owns the signal.
  offline: boolean
  /// `replay_wait_seconds` of visible time has passed since this open with no
  /// replay. Always false when the wait is configured to zero.
  waitExpired: boolean
  isOwner: boolean
  /// This pane has never had a screen: no replay has been applied on this mount.
  /// It is what "Attaching…" versus "Reconnecting…" turns on, and it is a fact
  /// about the PICTURE rather than about the socket, because that is what the
  /// two words mean to the person reading them: nothing has appeared yet, or
  /// what appeared has gone away.
  firstAttach: boolean
}

/// What the pane paints over the terminal.
export type AttachCover =
  /// Nothing: the terminal itself is the whole surface.
  | { kind: "none" }
  /// A spinner with a non-blocking cue.
  | { kind: "spinner"; wording: "starting" | "attaching" | "reconnecting" }
  /// A Reconnect affordance. `lost` is the socket giving up (which it now only
  /// does on a terminal close code); `no-screen` is a healthy socket that never
  /// sent a screen.
  | { kind: "box"; reason: "lost" | "no-screen" }
  /// The full-pane take-over card: another device drives this pty, or nobody
  /// does and this pane has not claimed it.
  | { kind: "card" }

export function attachCover(input: AttachCoverInputs): AttachCover {
  // A socket that has given up for good outranks everything, the card included:
  // a watcher whose socket died would otherwise see only "Take over" and never
  // learn the connection is gone. While the app-wide overlay is up it owns this
  // signal instead, and the pane must not stack a second answer under it.
  if (input.socket === "failed" && !input.offline) {
    return { kind: "box", reason: "lost" }
  }
  // A watcher's whole pane is the card, before and after the replay lands. It is
  // a statement about control rather than about pixels, so it does not wait for
  // a picture to cover.
  if (!input.isOwner) return { kind: "card" }
  // A socket that is not OPEN has no screen coming either, even when the last
  // open's replay is still on xterm: the picture is frozen, and saying so is the
  // whole point of the reconnect cue. So the cover is up for both reasons, and
  // the wait only ever becomes a box for the one it can honestly name, a healthy
  // socket that never sent a screen.
  if (!input.replayApplied || input.socket !== "open") {
    // The bounded wait. Suppressed while globally offline, where the offline
    // overlay already carries a Retry and a second one underneath it would read
    // as two answers to one question.
    if (input.waitExpired && !input.offline && !input.replayApplied) {
      return { kind: "box", reason: "no-screen" }
    }
    if (input.firstAttach) {
      // A first attach that has never seen output is a launch, and saying so is
      // more useful than saying the socket is busy.
      return { kind: "spinner", wording: input.everReady ? "attaching" : "starting" }
    }
    return { kind: "spinner", wording: "reconnecting" }
  }
  // The screen is on, but this pty has never printed anything: the provider is
  // still starting up. The oldest cover in the pane, and unrelated to attaching.
  if (!input.everReady) return { kind: "spinner", wording: "starting" }
  return { kind: "none" }
}
