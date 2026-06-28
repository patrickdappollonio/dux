// HTTP client for the workspace "spine": the projects, sessions, and core-computed
// sidebar grouping. Like `bootstrapApi.ts` and `changesApi.ts`, this is a plain
// GET (the read-only `git.ts` pattern, with `credentials: "same-origin"`) so it
// composes with HTTP caching and reads as a resource fetch. The matching
// `projects.changed` / `sessions.changed` events over `/ws/events` tell the
// client WHEN to re-GET.
//
// These three fields used to ride the broadcast `ViewModel`; they are volatile
// workspace state but are now read on demand over REST rather than re-broadcast
// to every client on every change. The server is authoritative: it projects the
// live projects/sessions plus the core sidebar model into this single document.
// A non-2xx is thrown as a `SpineFetchError` carrying the HTTP status so the
// caller can branch.

import type { ProjectView, SessionView, SidebarModel } from "./types"

// The spine document. Field names/types mirror the server's JSON and the values
// the legacy ViewModel carried, so consumers move over without a shape change.
export interface Spine {
  /** Every known project, in display order. */
  projects: ProjectView[]
  /** Every agent session, in display order. */
  sessions: SessionView[]
  /** Core-computed sidebar grouping (projects + sessions, orphans surfaced) so
   * both surfaces render an identical tree without re-deriving grouping. */
  sidebar: SidebarModel
}

// A failed spine fetch. `status` is the HTTP status (0 for a network/transport
// failure with no response). The boot path swallows this and keeps the
// last-known spine (null on first boot); a later `projects.changed` /
// `sessions.changed` event or a reconnect retries.
export class SpineFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "SpineFetchError"
    this.status = status
  }
}

export async function fetchSpine(): Promise<Spine> {
  let resp: Response
  try {
    resp = await fetch("/api/v1/spine", { credentials: "same-origin" })
  } catch {
    // The request never reached the server (offline, DNS, CORS).
    throw new SpineFetchError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new SpineFetchError(
      detail || `request failed (${resp.status})`,
      resp.status,
    )
  }
  return (await resp.json()) as Spine
}
