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

// What a stage left out of the index on purpose: the repositories inside a
// staged folder, which staging would otherwise record as links, not files.
export interface LeftOut {
  left_out_repositories?: number
  left_out_worktrees?: number
}

// What the user confirmed a discarded row was: `kind` is "file" or a folder
// row's kind, and a "directory" also carries `files`, how many files its
// dialog said would go, which the server refuses to exceed.
export interface DiscardConfirmation {
  kind: string
  files?: number
}

// A batch route's answer: what it acted on, and what it did not. A refused
// path with an entry in `reasons` was refused for a reason of its own (a
// folder holding only repositories, a worktree of this repository), in the
// server's words; any other had already left the section it validates against.
export interface BatchResult extends LeftOut {
  done: string[]
  refused: string[]
  reasons?: Record<string, string>
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
    postGitJson<LeftOut>(gitUrl(sessionId, "stage"), { path }),
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
  //
  // `kinds` names, for each row, what the user confirmed it was (see
  // `discard`); a row with no entry is sent as a file.
  discardMany: async (
    sessionId: string,
    paths: string[],
    confirmations: Readonly<Record<string, DiscardConfirmation>>,
  ): Promise<{
    done: string[]
    // How many files each done path actually took, by the server's count.
    deleted: Record<string, number>
    failed: { path: string; message: string }[]
  }> => {
    const done: string[] = []
    const deleted: Record<string, number> = {}
    const failed: { path: string; message: string }[] = []
    for (const path of paths) {
      try {
        const answer = await postGitJson<{ files_deleted?: number }>(
          gitUrl(sessionId, "discard"),
          { path, ...(confirmations[path] ?? { kind: "file" }) },
        )
        done.push(path)
        if (typeof answer.files_deleted === "number") deleted[path] = answer.files_deleted
      } catch (err) {
        failed.push({
          path,
          message: err instanceof Error ? err.message : "discard failed",
        })
      }
    }
    return { done, deleted, failed }
  },
  // `untracked` is deliberately not sent: the server re-derives delete versus restore
  // from live git status rather than trusting a client about a destructive outcome.
  // `confirmation` is what the user confirmed the row was (see
  // `DiscardConfirmation`). The server refuses when the path is no longer that
  // or a folder now holds more files, deletes a repository of its own only when
  // told it is one, and answers how many files actually went.
  discard: async (
    sessionId: string,
    path: string,
    confirmation: DiscardConfirmation,
  ): Promise<number | undefined> => {
    const answer = await postGitJson<{ files_deleted?: number }>(
      gitUrl(sessionId, "discard"),
      { path, ...confirmation },
    )
    return answer.files_deleted
  },
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
