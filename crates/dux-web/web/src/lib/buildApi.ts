// Which run of which build of dux the tab is talking to, read when the tab loads and
// again while it is trying to reconnect. A changed answer means a hard reload, since
// a tab that survives a restart runs the old interface against a new server.
//
// Two fields because neither alone is enough: `version` is the literal
// "development" for every untagged build, so only `process`, minted once per server
// run, moves after a rebuild and restart.
//
// Deliberately narrow: this identifies the run, never a schema or data shape.

import { apiFetch, rethrowAuthInterruption } from "./apiFetch"

export interface ServerIdentity {
  /** The binary's display version ("vX.Y.Z", or "development"). */
  version: string
  /** The server RUN, minted once per process. */
  process: string
}

// Reads the server's identity, answering `null` for anything that is not the
// document. Null means unknown, which `serverChanged` treats as no evidence.
// Throws the auth interruption when the sign-in gate stopped the read.
export async function fetchServerIdentity(): Promise<ServerIdentity | null> {
  try {
    const resp = await apiFetch("/api/v1/build", {
      credentials: "same-origin",
      cache: "no-store",
    })
    if (!resp.ok) return null
    const body: unknown = await resp.json()
    if (typeof body !== "object" || body === null) return null
    const { version, process } = body as Partial<ServerIdentity>
    if (typeof version !== "string" || typeof process !== "string") return null
    return { version, process }
  } catch (e) {
    // A read the sign-in gate stopped was never answered by anyone, so it is
    // neither "unknown" nor a completed check: the caller must not treat it as
    // having talked to the server.
    rethrowAuthInterruption(e)
    return null
  }
}

// Whether the server answering now is a different run or build from the one the tab
// loaded against. An unknown side is never a change: reloading because dux could not
// ask would throw the tab away exactly when the network is flaky.
export function serverChanged(
  baseline: ServerIdentity | null,
  current: ServerIdentity | null,
): boolean {
  if (baseline === null || current === null) return false
  return (
    baseline.version !== current.version || baseline.process !== current.process
  )
}
