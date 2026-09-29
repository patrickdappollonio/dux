// HTTP client for a session's changed files: a plain same-origin GET, re-issued
// when a `session.changes` event arrives, with a per-session `rev` the store uses
// to drop out-of-order responses.
//
// A non-2xx is thrown as a `ChangesFetchError` carrying the HTTP status.

import { changesRequestTimeoutMs } from "./connectionTiming"
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
  // present. Advisory: the poller self-heals via events regardless.
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

// The caller gave the request up (a reset superseded it). Nothing to show: the
// fetch that replaced it answers for the pane.
export class ChangesFetchAborted extends Error {
  constructor() {
    super("The changed-files request was superseded.")
    this.name = "ChangesFetchAborted"
  }
}

// `signal` lets the caller abandon the request; it rejects with
// `ChangesFetchAborted`. The request's own deadline, `[server]
// changes_request_timeout_seconds` read when the request starts, rejects with a
// `ChangesFetchError` (status 0) that says what happened. A half-open
// connection never settles, and without the deadline the pane would sit loading
// forever.
export async function fetchChanges(
  sessionId: string,
  signal?: AbortSignal,
): Promise<SessionChangesResponse> {
  const controller = new AbortController()
  const deadlineMs = changesRequestTimeoutMs()
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    controller.abort()
  }, deadlineMs)
  const forward = () => controller.abort()
  if (signal?.aborted) controller.abort()
  else signal?.addEventListener("abort", forward)
  // Whatever stopped the request, say which: the caller's abort is silent, the
  // deadline is an error with words for the pane.
  const stopped = (fallback: ChangesFetchError): Error => {
    if (timedOut) {
      return new ChangesFetchError(
        `The server did not send this session's changed files within ${
          deadlineMs / 1000
        } seconds. The connection may have stalled; Refresh to try again.`,
        0,
      )
    }
    if (controller.signal.aborted) return new ChangesFetchAborted()
    return fallback
  }
  try {
    let resp: Response
    try {
      resp = await fetch(
        `/api/v1/sessions/${encodeURIComponent(sessionId)}/changes`,
        { credentials: "same-origin", signal: controller.signal },
      )
    } catch {
      // The request never reached the server (offline, DNS, CORS). Status 0 so
      // the caller treats it as retryable, not a 404 "session gone".
      throw stopped(new ChangesFetchError("Could not reach the server.", 0))
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw stopped(
        new ChangesFetchError(
          detail || `request failed (${resp.status})`,
          resp.status,
          parseRetryAfter(resp.headers.get("retry-after")),
        ),
      )
    }
    try {
      return (await resp.json()) as SessionChangesResponse
    } catch (error) {
      throw stopped(
        new ChangesFetchError(
          error instanceof Error ? error.message : "Could not read the changed files.",
          0,
        ),
      )
    }
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener("abort", forward)
  }
}
