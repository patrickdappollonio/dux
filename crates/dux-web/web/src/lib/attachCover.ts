// The cover remains until the current attach replay is applied. A visible-time
// replay timeout becomes an explicit reconnect affordance rather than a blank pane.
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
  /// does on a terminal close code); `no-screen` is an OPEN socket that never
  /// sent a screen, and the openness is part of the claim rather than an
  /// implication of it.
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
    //
    // Only the BOX is suppressed there, not the spinner, and that asymmetry is
    // deliberate. The offline overlay is a full-viewport portal at a high
    // z-index: whatever the pane paints is behind it and grayscaled, so a
    // spinner underneath is not a second thing on screen. The box is different
    // because it is an ACTION, and two Reconnect affordances for one outage is a
    // real double-up whether or not both are visible at once.
    //
    // AND ONLY AGAINST A HEALTHY SOCKET, which is what the box's whole wording
    // claims. The clock is reset only by `pty.onOpen`, so its visible time keeps
    // accumulating straight through a drop; without this condition a pty socket
    // that took longer than the wait to come back put an opaque "Still waiting
    // for the terminal's screen" panel over a picture that was perfectly good
    // and a reconnect that was already under way. A socket that is not open has
    // its own honest cue, the reconnecting spinner below.
    if (
      input.socket === "open" &&
      input.waitExpired &&
      !input.offline &&
      !input.replayApplied
    ) {
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
