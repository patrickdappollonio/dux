// HTTP client for a session's changed files. Unlike `fileApi.ts` (POST), this is
// a plain GET (the read-only `git.ts` pattern, with `credentials: "same-origin"`)
// so it composes with HTTP caching and reads as a resource fetch. The matching
// `session.changes` event over `/ws/events` tells the client WHEN to re-GET; the
// per-session `rev` lets the store drop out-of-order responses.
//
// The server is authoritative: it resolves the session -> worktree and computes
// the lists. A non-2xx is thrown as a `ChangesFetchError` carrying the HTTP
// status so the caller can branch (404 -> clear, anything else -> retryable).

import type { ChangedFileView } from "./types"

// The dedicated changed-files payload. Distinct from the legacy `ChangedFiles`
// ViewModel shape (which carries `watched_session_id`, the global field being
// retired, and lacks `rev`): this is the single source the store now trusts.
export interface SessionChangesResponse {
  rev: number
  staged: ChangedFileView[]
  unstaged: ChangedFileView[]
}

// A failed changed-files fetch. `status` is the HTTP status (0 for a network/
// transport failure with no response) so the store can special-case 404
// (session gone -> clear the slice) versus a retryable error (409 git lock,
// 5xx, network) that surfaces a Refresh affordance.
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
