// The per-connection id the server assigns on `/ws` connect, delivered as the first frame. It
// lives in its own module so the socket layer and the REST clients, which stamp it as
// `X-Connection-Id`, can reach it without a circular import; scoping an operation to this id
// routes its status back to the client that started it.
//
// Null until the first `connected` frame, and cleared again when the socket drops, so a REST
// action fired during the reconnect window cannot stamp a dead id whose status reaches nobody.
// Callers omit the header while it is null, and the server then broadcasts to every client.
let connectionId: string | null = null

// The last id this tab held, kept across a drop: the tab's next events socket
// names it (`?after=`) so the server hands what the lost connection still
// counted as attached to over to the new one.
let previousConnectionId: string | null = null

// How long a terminal socket waits, at most, for this tab's events id before
// opening without one. An id lets the server count the terminal as part of
// this tab, so the tab is never in its own way when it deletes or stops what
// it shows; a second is far longer than the `connected` frame takes, and short
// enough that a terminal never sits blank on its account.
export const CONNECTION_ID_WAIT_MS = 1000

let pendingTimer: ReturnType<typeof setTimeout> | null = null
const waiters = new Set<() => void>()

export function setConnectionId(id: string | null): void {
  connectionId = id
  if (id !== null) {
    previousConnectionId = id
    settlePending()
  }
}

/// An events socket is opening with no id yet: terminal sockets hold their
/// open until its `connected` frame names one, or until the wait runs out.
export function noteConnectionIdPending(): void {
  if (connectionId !== null || pendingTimer !== null) return
  pendingTimer = setTimeout(settlePending, CONNECTION_ID_WAIT_MS)
}

/// Whether a terminal socket should hold its open for the id right now.
export function awaitingConnectionId(): boolean {
  return pendingTimer !== null
}

/// Subscribe to the wait ending, either way. Returns the unsubscribe.
export function onConnectionIdSettled(wake: () => void): () => void {
  waiters.add(wake)
  return () => {
    waiters.delete(wake)
  }
}

function settlePending(): void {
  if (pendingTimer === null) return
  clearTimeout(pendingTimer)
  pendingTimer = null
  for (const wake of [...waiters]) wake()
}

export function getPreviousConnectionId(): string | null {
  return previousConnectionId
}

export function getConnectionId(): string | null {
  return connectionId
}
