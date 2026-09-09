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
  /// It is what "Attaching…" versus "Reconnecting…" turns on, a fact about the
  /// picture rather than about the socket: nothing has appeared yet, or what
  /// appeared has gone away.
  firstAttach: boolean
}

/// What the pane paints over the terminal.
export type AttachCover =
  /// Nothing: the terminal itself is the whole surface.
  | { kind: "none" }
  /// A spinner with a non-blocking cue.
  | { kind: "spinner"; wording: "starting" | "attaching" | "reconnecting" }
  /// A Reconnect affordance. `lost` is the socket giving up (only on a terminal
  /// close code); `no-screen` is an open socket that never sent a screen, and the
  /// openness is part of the claim rather than an implication of it.
  | { kind: "box"; reason: "lost" | "no-screen" }
  /// The full-pane take-over card: another device drives this pty, or nobody
  /// does and this pane has not claimed it.
  | { kind: "card" }

export function attachCover(input: AttachCoverInputs): AttachCover {
  // A socket that has given up for good outranks everything, the card included:
  // a watcher whose socket died would otherwise see only "Take over" and never
  // learn the connection is gone. While the app-wide overlay is up it owns this
  // signal instead.
  if (input.socket === "failed" && !input.offline) {
    return { kind: "box", reason: "lost" }
  }
  // A watcher's whole pane is the card, before and after the replay lands. It is
  // a statement about control rather than about pixels, so it does not wait for
  // a picture to cover.
  if (!input.isOwner) return { kind: "card" }
  // A socket that is not open has no screen coming either, even when the last
  // open's replay is still on xterm: the picture is frozen, which is the whole
  // point of the reconnect cue. So the cover is up for both reasons, and the wait
  // only becomes a box for the one it can honestly name, a healthy socket that
  // never sent a screen.
  if (!input.replayApplied || input.socket !== "open") {
    // The bounded wait, suppressed while globally offline, where the offline
    // overlay already carries a Retry. Only the box is suppressed there, not the
    // spinner: the overlay is a full-viewport portal, so a spinner behind it is
    // not a second thing on screen, while two Reconnect affordances for one
    // outage is a real double-up.
    //
    // And only against a healthy socket, which is what the box's wording claims.
    // The clock is reset only by `pty.onOpen`, so its visible time keeps
    // accumulating straight through a drop; without this condition a slow
    // reconnect puts an opaque panel over a picture that is perfectly good. A
    // socket that is not open has its own cue, the reconnecting spinner below.
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
