// HTTP client for the two first-load screens: dismissing one, and fetching the release notes
// for the app menu's "What's new…" entry. Both stamp the per-connection id so the server routes
// each operation's status back to the initiating client. Dismissal is shared rather than
// per-browser: it writes `last_seen_version`, the same row the TUI reads, which is why this
// feature keeps no localStorage flag.

import { getConnectionId } from "./connection"
import type { ReleaseNotesView } from "./bootstrapApi"

function headers(): Record<string, string> {
  const h: Record<string, string> = { "content-type": "application/json" }
  const id = getConnectionId()
  if (id) h["x-connection-id"] = id
  return h
}

/** Thrown by `fetchReleaseNotes` so the caller can toast the server's reason. `status` is the
 * HTTP status, 0 for a transport failure; a 404 (no release for this tag) cannot be retried. */
export class ReleaseNotesFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "ReleaseNotesFetchError"
    this.status = status
  }
}

export const firstLoadApi = {
  // Record the running version as seen and drop the pending screen. Called only when the user
  // closes an automatic screen; an on-demand open from the app menu deliberately does not.
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

  // Fetch this build's release notes. May take a moment (the server fetches from GitHub behind
  // a cache), so callers show a loading state. `ui.disable_release_notes` suppresses only the
  // automatic screen, never this.
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
