// HTTP client for the two first-load screens: dismissing one, and fetching the
// release notes on demand for the app menu's "What's new…" entry.
//
// Both stamp the per-connection id so the server can route each operation's
// status toast back to the initiating client (the `configApi.ts` pattern).
//
// DISMISSAL IS SHARED, not per-browser: the server writes `last_seen_version` to
// SQLite, the same row the TUI reads. Dismissing here settles the screen on both
// surfaces, which is why there is deliberately no localStorage flag anywhere in
// this feature.

import { getConnectionId } from "./connection"
import type { ReleaseNotesView } from "./bootstrapApi"

function headers(): Record<string, string> {
  const h: Record<string, string> = { "content-type": "application/json" }
  const id = getConnectionId()
  if (id) h["x-connection-id"] = id
  return h
}

/** Thrown by `fetchReleaseNotes` so the caller can toast the server's reason.
 * `status` is the HTTP status (0 for a transport failure): 404 means GitHub has
 * no release for this build's tag and retrying cannot help; 502 means it might
 * work later. */
export class ReleaseNotesFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "ReleaseNotesFetchError"
    this.status = status
  }
}

export const firstLoadApi = {
  // Record the running version as seen and drop the pending screen. Called when
  // the user closes an AUTOMATIC first-load screen; an on-demand open (from the
  // app menu) deliberately does NOT call this, because looking something up is
  // not dismissing this launch's screen.
  dismiss: async (): Promise<void> => {
    let resp: Response
    try {
      resp = await fetch("/api/v1/first-load/dismiss", {
        method: "POST",
        credentials: "same-origin",
        headers: headers(),
        body: "{}",
      })
    } catch {
      throw new Error("Could not reach the server.")
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw new Error(detail || `request failed (${resp.status})`)
    }
  },

  // Fetch this build's release notes. May take a moment (the server fetches from
  // GitHub behind a six-hour cache), so callers show a real loading state. Works
  // regardless of the `ui.disable_release_notes` preference: that flag suppresses
  // only the automatic screen.
  fetchReleaseNotes: async (): Promise<ReleaseNotesView> => {
    let resp: Response
    try {
      resp = await fetch("/api/v1/release-notes", {
        credentials: "same-origin",
        headers: headers(),
      })
    } catch {
      throw new ReleaseNotesFetchError("Could not reach the server.", 0)
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw new ReleaseNotesFetchError(
        detail || `request failed (${resp.status})`,
        resp.status,
      )
    }
    return (await resp.json()) as ReleaseNotesView
  },
}
