// HTTP client for a session's changed files: a plain same-origin GET, re-issued
// when a `session.changes` event arrives, with a per-session `rev` the store uses
// to drop out-of-order responses.
//
// A non-2xx is thrown as a `ChangesFetchError` carrying the HTTP status.

import type { ChangedFileView } from "./types"

// The changed-files payload, and the one source the store trusts.
export interface SessionChangesResponse {
  rev: number
  staged: ChangedFileView[]
  unstaged: ChangedFileView[]
}

// A failed changed-files fetch; `status` is 0 for a transport failure with no
// response. A 404 means the session is gone and the slice clears; anything else is
// retryable and surfaces a Refresh control.
export class ChangesFetchError extends Error {
  readonly status: number
  // The parsed `Retry-After` (seconds) on a 409 git-lock/rebase response, when
  // present. Advisory — the poller self-heals via events regardless.
  readonly retryAfter: number | null

  constructor(message: string, status: number, retryAfter: number | null = null) {
    super(message)
    this.name = "ChangesFetchError"
    this.status = status
    this.retryAfter = retryAfter
  }
}

function parseRetryAfter(raw: string | null): number | null {
  if (!raw) return null
  const seconds = Number(raw)
  return Number.isFinite(seconds) && seconds >= 0 ? seconds : null
}

export async function fetchChanges(
  sessionId: string,
): Promise<SessionChangesResponse> {
  let resp: Response
  try {
    resp = await fetch(
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/changes`,
      { credentials: "same-origin" },
    )
  } catch {
    // The request never reached the server (offline, DNS, CORS). Status 0 so the
    // caller treats it as retryable, not a 404 "session gone".
    throw new ChangesFetchError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new ChangesFetchError(
      detail || `request failed (${resp.status})`,
      resp.status,
      parseRetryAfter(resp.headers.get("retry-after")),
    )
  }
  return (await resp.json()) as SessionChangesResponse
}
