// HTTP client for the web code editor: read and write a worktree file's working
// copy. Request/response (like `git.ts`) so the editor can await the content,
// show per-file loading/saving state, and surface a real error message.
//
// The server validates every request (session resolution + that the path is a
// real git-changed file inside the worktree, plus an independent path-escape and
// binary/size guard), so the UI never has to. A write triggers an engine
// changed-files recompute that reaches every client over the WebSocket.

export interface WorktreeFile {
  path: string
  // True when the file is binary — `content` is empty and the editor refuses it.
  binary: boolean
  content: string
}

async function postFile<T>(path: string, body: Record<string, unknown>): Promise<T> {
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

async function postFileNoContent(
  path: string,
  body: Record<string, unknown>,
): Promise<void> {
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

export const fileApi = {
  read: (sessionId: string, path: string) =>
    postFile<WorktreeFile>("/api/file/read", { session_id: sessionId, path }),
  write: (sessionId: string, path: string, content: string) =>
    postFileNoContent("/api/file/write", {
      session_id: sessionId,
      path,
      content,
    }),
}
