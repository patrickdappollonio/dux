// The per-connection id the server assigns on `/ws` connect, delivered as the
// first frame (`{ type: "connected", id }`). It lives in its own module so both
// the socket layer (`store.ts`, which records it) and the REST client (`git.ts`,
// which stamps it on the push/pull/checkout POSTs as `X-Connection-Id`) can
// reach it without a circular import. Scoping those operations to this id lets
// the server route their status toasts back to the client that initiated them.
//
// Null until the first `connected` frame arrives (overwritten on each reconnect,
// which re-issues one). Callers omit the header while it is null. The store also
// clears it back to null when the socket drops (`socket.onConn` closed/failed) so a
// REST action fired during the reconnect window does not stamp a now-dead id whose
// status would be routed to nobody — a null id falls back to scope `All` (broadcast
// to every client), the safe default for this single-tenant tool.
let connectionId: string | null = null

export function setConnectionId(id: string | null): void {
  connectionId = id
}

export function getConnectionId(): string | null {
  return connectionId
}
