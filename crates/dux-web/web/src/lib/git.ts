// HTTP client for mutating per-session git operations (stage/unstage/discard/
// commit/push/pull). These are request/response — unlike the fire-and-forget
// WebSocket commands — so callers can `await` completion, show a per-action
// loading state, and surface a real error message. Live changed-files updates
// still arrive over the WebSocket once the engine recomputes after the mutation.
//
// Project-scoped git operations (pull-project, checkout-default) moved to the
// REST `projectsApi` (`/api/v1/projects/{id}/pull` and `/checkout-default`).
//
// The server validates every request (session/project resolution + that a file
// path is a real git-tracked file inside the worktree), so the UI never has to.

import { getConnectionId } from "./connection"

async function postGit(
  path: string,
  body: Record<string, unknown>,
  opts?: { scopeToConnection?: boolean },
): Promise<void> {
  const headers: Record<string, string> = { "content-type": "application/json" }
  // The async git operations (push/pull/checkout) report progress on the status
  // stream; stamp this connection's id so the server can scope those toasts back
  // to this client. Omitted until the `connected` frame has set the id.
  if (opts?.scopeToConnection) {
    const id = getConnectionId()
    if (id) headers["x-connection-id"] = id
  }
  const resp = await fetch(path, {
    method: "POST",
    credentials: "same-origin",
    headers,
    body: JSON.stringify(body),
  })
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new Error(detail || `request failed (${resp.status})`)
  }
}

// The session id is the `:id` path segment (encoded) — no longer a body field.
const gitUrl = (sessionId: string, action: string) =>
  `/api/v1/sessions/${encodeURIComponent(sessionId)}/git/${action}`

export const git = {
  stage: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "stage"), { path }),
  unstage: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "unstage"), { path }),
  // `untracked` is intentionally NOT sent: the server re-derives the
  // delete-vs-restore distinction from live git status (never trusting the
  // client about a destructive outcome).
  discard: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "discard"), { path }),
  commit: (sessionId: string, message: string) =>
    postGit(gitUrl(sessionId, "commit"), { message }),
  // push/pull are bodiless; the session is in the path.
  push: (sessionId: string) =>
    postGit(gitUrl(sessionId, "push"), {}, { scopeToConnection: true }),
  pull: (sessionId: string) =>
    postGit(gitUrl(sessionId, "pull"), {}, { scopeToConnection: true }),
}
