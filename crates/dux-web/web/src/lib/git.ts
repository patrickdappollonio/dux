// HTTP client for mutating git operations (stage/unstage/discard/commit/push/
// pull/checkout-default). These are request/response — unlike the fire-and-forget
// WebSocket commands — so callers can `await` completion, show a per-action
// loading state, and surface a real error message. Live changed-files updates
// still arrive over the WebSocket once the engine recomputes after the mutation.
//
// The server validates every request (session/project resolution + that a file
// path is a real git-tracked file inside the worktree), so the UI never has to.

async function postGit(path: string, body: Record<string, unknown>): Promise<void> {
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
}

export const git = {
  stage: (sessionId: string, path: string) =>
    postGit("/api/git/stage", { session_id: sessionId, path }),
  unstage: (sessionId: string, path: string) =>
    postGit("/api/git/unstage", { session_id: sessionId, path }),
  // `untracked` is intentionally NOT sent: the server re-derives the
  // delete-vs-restore distinction from live git status (never trusting the
  // client about a destructive outcome).
  discard: (sessionId: string, path: string) =>
    postGit("/api/git/discard", { session_id: sessionId, path }),
  commit: (sessionId: string, message: string) =>
    postGit("/api/git/commit", { session_id: sessionId, message }),
  push: (sessionId: string) => postGit("/api/git/push", { session_id: sessionId }),
  pull: (sessionId: string) => postGit("/api/git/pull", { session_id: sessionId }),
  pullProject: (projectId: string) =>
    postGit("/api/git/pull-project", { project_id: projectId }),
  checkoutDefault: (projectId: string) =>
    postGit("/api/git/checkout-default", { project_id: projectId }),
}
