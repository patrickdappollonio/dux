// The page lifecycle, handled explicitly rather than inferred from visibility. A
// page is not simply visible or hidden: it is also frozen, restored from the
// back/forward cache, or on its way out, and each wants something different from
// a WebSocket.
//
//   `pagehide`  Close both sockets. An open socket disqualifies the page from
//               the bfcache anyway, and it leaves the server a phantom owner
//               holding the pty for a connection nothing has noticed is gone.
//
//   `pageshow`  Reopen, plain, whether or not `persisted` is set. Never a
//               take-over: an automatic reconnect is never a claim.
//
//   `freeze`    Park. The page is about to stop executing, so a timer that
//               survives fires against a discarded document, or hours late.
//
//   `resume`    Reopen, plain. It fires while the page is still hidden, so the
//               reopen is a request rather than an act: a parked socket defers
//               it to the first visible moment (see `resumeNow`), because an
//               attach that lands hidden claims nothing and is never re-asked.
//
// On return, and before input is re-enabled, the server-run identity, ownership,
// the replay epoch and the cover each reconcile; nothing here re-enables typing.

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

/// A socket that participates, satisfied structurally, so nothing in this module
/// needs to know what a WebSocket is.
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
