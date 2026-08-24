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
  const headers: Record<string, string> = {
    "content-type": "application/json",
  }
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

// A batch route's answer: what it acted on, and what had already left the
// section it validates against.
export interface BatchResult {
  done: string[]
  refused: string[]
}

async function postGitJson<T>(
  path: string,
  body: Record<string, unknown>,
): Promise<T> {
  const resp = await fetch(path, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  })
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new Error(detail || `request failed (${resp.status})`)
  }
  return (await resp.json()) as T
}

// The session id is the `:id` path segment (encoded), never a body field.
const gitUrl = (sessionId: string, action: string) =>
  `/api/v1/sessions/${encodeURIComponent(sessionId)}/git/${action}`

export const git = {
  stage: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "stage"), { path }),
  unstage: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "unstage"), { path }),
  // A whole checked selection in one request: one git call, one changed-files
  // refresh, one broadcast. The server partitions the batch and names what it
  // could not act on, so the caller raises a single toast for the outcome.
  stageMany: (sessionId: string, paths: string[]) =>
    postGitJson<BatchResult>(gitUrl(sessionId, "stage-files"), { paths }),
  unstageMany: (sessionId: string, paths: string[]) =>
    postGitJson<BatchResult>(gitUrl(sessionId, "unstage-files"), { paths }),
  // Discard has no batch route: each file is independent, and a refusal on one
  // ("unstage it first") must not block the rest. Sequential because parallel
  // checkouts contend on index.lock. The per-file outcomes come back to the
  // caller, which raises one toast for the lot.
  discardMany: async (
    sessionId: string,
    paths: string[],
  ): Promise<{ done: string[]; failed: { path: string; message: string }[] }> => {
    const done: string[] = []
    const failed: { path: string; message: string }[] = []
    for (const path of paths) {
      try {
        await postGit(gitUrl(sessionId, "discard"), { path })
        done.push(path)
      } catch (err) {
        failed.push({
          path,
          message: err instanceof Error ? err.message : "discard failed",
        })
      }
    }
    return { done, failed }
  },
  // `untracked` is intentionally NOT sent: the server re-derives the
  // delete-vs-restore distinction from live git status (never trusting the
  // client about a destructive outcome).
  discard: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "discard"), { path }),
  commit: (sessionId: string, message: string) =>
    postGit(gitUrl(sessionId, "commit"), { message }),
  // Force a changed-files recompute. Mutates nothing: it makes the server do
  // the same refresh every mutating route above does, and the file-drop upload
  // does for a file landing in the worktree, so a change dux did not make
  // through one of its own routes (a file the user changed from a terminal, say)
  // shows up now instead of on the next poll. Bodiless; the session is in the
  // path.
  refreshChanges: (sessionId: string) =>
    postGit(gitUrl(sessionId, "refresh-changes"), {}),
  // push/pull are bodiless; the session is in the path.
  push: (sessionId: string) =>
    postGit(gitUrl(sessionId, "push"), {}, { scopeToConnection: true }),
  pull: (sessionId: string) =>
    postGit(gitUrl(sessionId, "pull"), {}, { scopeToConnection: true }),
}
