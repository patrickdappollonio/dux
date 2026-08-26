// IS THIS STILL THE SERVER THIS TAB LOADED AGAINST, as far as we have actually
// checked?
//
// The events socket's `onOpen` fires the run-identity probe as
// `void reloadIfServerChanged()`: an async fetch that resolves LATER. So
// `conn === "open"` is true for a whole network round trip BEFORE the check has
// answered, and a PTY socket allowed to retry in that window attaches to a
// server that may have restarted. Attaching to an agent's pty LAUNCHES its
// provider, so that window is exactly the one the check exists to close, and
// gating on `conn` leaves it wide open.
//
// Hence a distinct signal, set only when the probe has RESOLVED and the run has
// not moved. A moved run hard reloads the page, so there is deliberately no
// "invalid" value to publish: the page goes away instead.
//
// An UNKNOWN answer (the probe failed, or the server is too old to have a build
// endpoint) counts as validated, matching `serverChanged`, which treats unknown
// as no evidence rather than as a change. The alternative would hold every
// terminal shut for as long as one endpoint stayed unreachable, which is the
// wrong failure for a tool whose job is to keep a terminal on screen. It is not
// LATCHED, though: the store re-asks after an unknown answer, so a tab does not
// spend the rest of its life running old code against a run it never confirmed.
//
// It lives in its own module rather than on the store's state so the PTY socket
// can read it without importing the store, which imports the PTY socket.

let validated = false

// Sockets holding a retry behind this gate. A gate that opens has to WAKE them:
// the retry timer would otherwise sit out whatever gap it had grown to before
// noticing, which on a phone that just came back is exactly the wait the gate
// was never meant to add. Registration is per socket and retired on close, so
// the set is bounded by the number of live PTY sockets.
const waiters = new Set<() => void>()

/// Subscribe to the gate opening. Returns the unsubscribe. Called by
/// `PtySocket`, which is the only thing the gate holds.
export function onServerValidated(wake: () => void): () => void {
  waiters.add(wake)
  return () => {
    waiters.delete(wake)
  }
}

/// The probe resolved and the run matches. Called from the store, both from the
/// boot baseline read and from every later events open that passes the check.
///
/// BOOT COUNTS, and that is the whole reason this is called from two places. The
/// baseline read boot performs is by construction a round trip to the very server
/// this tab loaded from, so its completion is proof the run has not moved. Before
/// it was, the FIRST events open took the skip-the-duplicate-load branch, the
/// probe never ran, and the gate stayed shut for the life of that connection: a
/// PTY socket that dropped on a freshly loaded page re-armed its retry timer
/// forever and never attempted an attach.
export function noteServerValidated(): void {
  const opened = !validated
  validated = true
  if (!opened) return
  // Snapshot, so a waiter that unsubscribes while being woken cannot perturb
  // the live iteration.
  for (const wake of [...waiters]) wake()
}

/// The events socket dropped, so the next open owes a fresh check: whatever was
/// confirmed was confirmed about a connection that is gone. Called from the
/// store on every events-socket close.
export function clearServerValidated(): void {
  validated = false
}

/// Whether a PTY socket may attach right now.
export function serverValidated(): boolean {
  return validated
}
