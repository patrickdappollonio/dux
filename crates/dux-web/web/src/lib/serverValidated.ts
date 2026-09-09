// Is this still the server this tab loaded against, as far as has actually been
// checked? The events socket's `onOpen` fires the run-identity probe as an async
// fetch, so `conn === "open"` is true for a whole round trip before the check
// answers, and a PTY socket that attaches in that window launches a provider
// against a server that may have restarted.
//
// So the gate is set only once the probe has resolved and the run has not moved.
// There is deliberately no "invalid" value: a moved run hard reloads the page.
//
// An unknown answer (a failed probe, or a server too old to have a build
// endpoint) counts as validated, matching `serverChanged`, which reads unknown
// as no evidence rather than as a change; holding every terminal shut while one
// endpoint is unreachable is the wrong failure. It is not latched: the store
// re-asks after an unknown answer.
//
// It is its own module so the PTY socket can read it without importing the
// store, which imports the PTY socket.

let validated = false

// Sockets holding a retry behind this gate. Opening the gate must wake them, or
// a retry timer sits out whatever gap it had grown to first. Registration is per
// socket and retired on close, so the set is bounded by the live PTY sockets.
const waiters = new Set<() => void>()

/// Subscribe to the gate opening. Returns the unsubscribe. Called by
/// `PtySocket`, which is the only thing the gate holds.
export function onServerValidated(wake: () => void): () => void {
  waiters.add(wake)
  return () => {
    waiters.delete(wake)
  }
}

/// The probe resolved and the run matches. Called from the store on the boot
/// baseline read as well as on every later events open that passes the check:
/// boot's read is by construction a round trip to the server this tab loaded
/// from, and the first events open skips the probe, so without it the gate would
/// stay shut for the life of that connection.
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
