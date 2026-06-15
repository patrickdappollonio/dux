// HTTP client for the web code editor: read and write a worktree file's working
// copy. Request/response (like `git.ts`) so the editor can await the content,
// show per-file loading/saving state, and surface a real error message.
//
// The server validates every request (session resolution + that the path stays
// inside the worktree root — a path-escape/`.git`/symlink guard — plus a
// binary/size guard), so the UI never has to. There is NO git-tracked/changed
// gate: any path inside the worktree is editable, ignored or not. A write
// triggers an engine changed-files recompute that reaches every client over the
// WebSocket.

export interface WorktreeFile {
  path: string
  // True when the file is binary — `content` is empty and the editor refuses it.
  binary: boolean
  content: string
}

// The two raw sides of a changed file (HEAD vs working copy) for the editor's
// Monaco diff view. `original`/`modified` are "" for an added/deleted side;
// `binary` means neither side is renderable text. Mirrors the Rust DiffContents.
export interface FileDiffContents {
  path: string
  original: string
  modified: string
  binary: boolean
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
  // The worktree's browsable files for the editor tree: tracked, untracked, and
  // loose gitignored files (fully-ignored dirs like node_modules are collapsed
  // out server-side). Editing is NOT limited to this set — any path inside the
  // worktree can be read/written/created (the server enforces containment).
  list: (sessionId: string) =>
    postFile<{ files: string[] }>("/api/file/list", {
      session_id: sessionId,
    }).then((r) => r.files),
  read: (sessionId: string, path: string) =>
    postFile<WorktreeFile>("/api/file/read", { session_id: sessionId, path }),
  // The two raw sides (HEAD vs working copy) of a changed file for the Monaco
  // diff view. The server resolves both sides and the binary flag.
  diff: (sessionId: string, path: string) =>
    postFile<FileDiffContents>("/api/file/diff", { session_id: sessionId, path }),
  write: (sessionId: string, path: string, content: string) =>
    postFileNoContent("/api/file/write", {
      session_id: sessionId,
      path,
      content,
    }),
  // Open the file in a locally-installed GUI editor (server-side spawn) and
  // resolve with the chosen editor's label for a toast. `editor` is the dux-core
  // editor config key (e.g. "vscode") the user picked; the server launches that
  // one and errors if it isn't installed. Only useful when the server is the
  // user's own machine — the UI gates this to local-access URLs.
  openInEditor: (sessionId: string, path: string, editor: string) =>
    postFile<{ editor: string }>("/api/file/open-in-editor", {
      session_id: sessionId,
      path,
      editor,
    }).then((r) => r.editor),
}
