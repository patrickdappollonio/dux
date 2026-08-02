// Which run of which build of dux the tab is talking to, and whether that is
// still the one it loaded against.
//
// A tab left open across a dux restart keeps running the interface it was
// served, against a server that is not the one that served it. The events socket
// reconnects and the app carries on, rendering whatever the new server sends
// through the old code. So the tab reads `GET /api/v1/build` when it loads,
// remembers the answer, and reads it again while it is on the disconnected
// screen trying to get back (see `store.ts`, `eventsSocket.onOpen`). A changed
// answer means a hard reload, not an in-place reconnect.
//
// TWO fields, not one, and the second is the one doing the work in development:
// `version` is the literal string "development" for every build that is not a
// tagged release, so a rebuild-and-restart never moves it, while `process` is
// minted once per server run and always does. `version` earns its place on a
// release build, where it names what actually changed under the user.
//
// Deliberately narrow. This says which run of which build is answering, and
// nothing else; it is not a schema or data-shape version and must not become
// one. The interface ships inside the server binary, so a shape change cannot
// reach a client without a new build and a restart carrying it, which the run id
// already catches.

export interface ServerIdentity {
  /** The binary's display version ("vX.Y.Z", or "development"). */
  version: string
  /** The server RUN, minted once per process. */
  process: string
}

// Read the server's identity. Answers `null` for anything that is not the
// document: unreachable, non-2xx, or a body missing either field (an older
// server, a proxy error page). Null means UNKNOWN, and `serverChanged` treats
// unknown as "no evidence", never as "changed".
export async function fetchServerIdentity(): Promise<ServerIdentity | null> {
  try {
    const resp = await fetch("/api/v1/build", {
      credentials: "same-origin",
      cache: "no-store",
    })
    if (!resp.ok) return null
    const body: unknown = await resp.json()
    if (typeof body !== "object" || body === null) return null
    const { version, process } = body as Partial<ServerIdentity>
    if (typeof version !== "string" || typeof process !== "string") return null
    return { version, process }
  } catch {
    return null
  }
}

// Whether the server answering now is a different run or a different build from
// the one the tab loaded against.
//
// An unknown side (a probe that failed, or a baseline never learned) is NOT a
// change. Reloading because we could not ask would throw a tab away at exactly
// the moment the network was flaky, which is when its user can least afford it.
export function serverChanged(
  baseline: ServerIdentity | null,
  current: ServerIdentity | null,
): boolean {
  if (baseline === null || current === null) return false
  return (
    baseline.version !== current.version || baseline.process !== current.process
  )
}
