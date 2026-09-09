// HTTP client for mutating per-session git operations. Request/response rather than
// a fire-and-forget socket command, so a caller can await completion, show a loading
// state and surface a real error; the changed-files update still arrives over the
// socket once the engine recomputes.
//
// The server validates every request, resolving the session and confirming the path
// is a git-tracked file inside the worktree, so the UI never has to. Project-scoped
// operations live in `projectsApi`.

import { getConnectionId } from "./connection"

async function postGit(
  path: string,
  body: Record<string, unknown>,
  opts?: { scopeToConnection?: boolean },
): Promise<void> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
  }
  // The async git operations report progress on the status stream, so the connection
  // id scopes those toasts back to this client. Absent until the `connected` frame.
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
  // A whole checked selection in one request, so one git call and one broadcast. The
  // server names what it could not act on, and the caller raises a single toast.
  stageMany: (sessionId: string, paths: string[]) =>
    postGitJson<BatchResult>(gitUrl(sessionId, "stage-files"), { paths }),
  unstageMany: (sessionId: string, paths: string[]) =>
    postGitJson<BatchResult>(gitUrl(sessionId, "unstage-files"), { paths }),
  // Discard has no batch route: a refusal on one file must not block the rest.
  // Sequential because parallel checkouts contend on index.lock, at the accepted cost
  // of one changed-files refresh and broadcast per file.
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
  // `untracked` is deliberately not sent: the server re-derives delete versus restore
  // from live git status rather than trusting a client about a destructive outcome.
  discard: (sessionId: string, path: string) =>
    postGit(gitUrl(sessionId, "discard"), { path }),
  commit: (sessionId: string, message: string) =>
    postGit(gitUrl(sessionId, "commit"), { message }),
  // Forces a changed-files recompute and mutates nothing, so a change dux did not
  // make through one of its own routes shows up now rather than on the next poll.
  refreshChanges: (sessionId: string) =>
    postGit(gitUrl(sessionId, "refresh-changes"), {}),
  // push/pull are bodiless; the session is in the path.
  push: (sessionId: string) =>
    postGit(gitUrl(sessionId, "push"), {}, { scopeToConnection: true }),
  pull: (sessionId: string) =>
    postGit(gitUrl(sessionId, "pull"), {}, { scopeToConnection: true }),
}
