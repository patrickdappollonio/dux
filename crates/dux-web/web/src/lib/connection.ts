// The per-connection id the server assigns on `/ws` connect, delivered as the first frame. It
// lives in its own module so the socket layer and the REST clients, which stamp it as
// `X-Connection-Id`, can reach it without a circular import; scoping an operation to this id
// routes its status back to the client that started it.
//
// Null until the first `connected` frame, and cleared again when the socket drops, so a REST
// action fired during the reconnect window cannot stamp a dead id whose status reaches nobody.
// Callers omit the header while it is null, and the server then broadcasts to every client.
let connectionId: string | null = null

export function setConnectionId(id: string | null): void {
  connectionId = id
}

export function getConnectionId(): string | null {
  return connectionId
}
