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
// wrong failure for a tool whose job is to keep a terminal on screen.
//
// It lives in its own module rather than on the store's state so the PTY socket
// can read it without importing the store, which imports the PTY socket.

let validated = false

/// The probe resolved and the run matches. Called from the store.
export function noteServerValidated(): void {
  validated = true
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
