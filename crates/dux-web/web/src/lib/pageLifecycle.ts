// THE PAGE LIFECYCLE, HANDLED EXPLICITLY, never inferred from visibility.
//
// A page is not simply visible or hidden. It is also frozen, restored from the
// back/forward cache, or on its way out, and each of those wants something
// different from a WebSocket. Guessing at them from `visibilitychange` gets two
// of them wrong:
//
//   `pagehide`  CLOSE both sockets, cleanly. A page held in the bfcache with an
//               open socket is EVICTED from the cache anyway (an open WebSocket
//               is a documented disqualifier in every engine that implements
//               it), so keeping it buys nothing, and it costs the server a
//               phantom owner: the pty stays claimed by a connection whose next
//               send is the first thing to discover it is gone.
//
//   `pageshow`  REOPEN, plain, whether or not `persisted` is set. A restored page
//               has the sockets it closed on the way out, which is none, and a
//               plain navigation back also lands here. Never a take-over: an
//               automatic reconnect is never a claim.
//
//   `freeze`    PARK (Chromium). The page is about to stop executing; a timer
//               that survives it fires against a document that has been
//               discarded, or hours late.
//
//   `resume`    REOPEN, plain (Chromium's other half of `freeze`). It fires
//               while the page is still HIDDEN, which is why the reopen is a
//               request rather than an act: a socket that parks defers it to
//               the first visible moment (see `resumeNow`), because an attach
//               that lands hidden claims nothing and is never re-asked.
//
// The mapping is a pure function so it can be read and tested as a table, and
// the wiring below is the only place that touches the events.
//
// ON RETURN, and BEFORE input is re-enabled, four things reconcile: the
// server-run identity (the events socket's `onOpen` probe, which is also the PTY
// retry gate, see `serverValidated.ts`), ownership (the pty handshake's owner
// snapshot), the replay epoch (`attachReplay.ts`), and the cover
// (`attachCover.ts`). Nothing here re-enables typing on its own; each of those
// four is what the pane waits on.

/// What one lifecycle event asks of a socket.
export type LifecycleAction = "close" | "reopen" | "park" | "ignore"

/// The whole table. `persisted` is accepted and deliberately ignored for
/// `pageshow`: a bfcache restore and an ordinary back navigation both arrive
/// here with nothing open, and both want the same plain reopen.
export function lifecycleAction(event: string): LifecycleAction {
  switch (event) {
    case "pagehide":
      return "close"
    case "pageshow":
    case "resume":
      return "reopen"
    case "freeze":
      return "park"
    default:
      return "ignore"
  }
}

/// The events this module listens to, in one list so the wiring and the table
/// cannot drift.
export const LIFECYCLE_EVENTS = ["pagehide", "pageshow", "freeze", "resume"] as const

/// A socket that participates. Both `ReconnectingSocket` subclasses satisfy it
/// structurally; it is spelled out here so nothing in this module needs to know
/// what a WebSocket is.
export type LifecycleParticipant = {
  close: () => void
  resumeNow: () => void
  park: () => void
}

const participants = new Set<LifecycleParticipant>()
let attached = false

/// Apply one event to one participant. Exported for the table test, which would
/// otherwise have to assert against a `switch` it cannot see.
export function applyLifecycle(
  participant: LifecycleParticipant,
  event: string,
): LifecycleAction {
  const action = lifecycleAction(event)
  switch (action) {
    case "close":
      participant.close()
      break
    case "reopen":
      participant.resumeNow()
      break
    case "park":
      participant.park()
      break
    case "ignore":
      break
  }
  return action
}

function onLifecycleEvent(ev: Event): void {
  for (const participant of [...participants]) {
    applyLifecycle(participant, ev.type)
  }
}

/// Enrol a socket. Returns the unregister, which every caller with a lifetime
/// shorter than the page's (every PTY socket) must call.
export function registerPageLifecycle(
  participant: LifecycleParticipant,
): () => void {
  participants.add(participant)
  // Guarded on the METHOD, not the global: off-browser there is no window, and
  // some test harnesses stub a partial one.
  if (
    !attached &&
    typeof window !== "undefined" &&
    typeof window.addEventListener === "function"
  ) {
    attached = true
    for (const event of LIFECYCLE_EVENTS) {
      window.addEventListener(event, onLifecycleEvent)
    }
  }
  return () => {
    participants.delete(participant)
  }
}
