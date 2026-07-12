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

import type { DirEntry } from "@/lib/fileTree"

export interface WorktreeFile {
  path: string
  // True when the file is binary — `content` is empty and the editor refuses it.
  binary: boolean
  content: string
  /** True when the server opened this file read-only (outside-resolving symlink
   *  or a .git/ path). The editor must not allow saving. */
  read_only?: boolean
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

// The session id is the `:id` path segment (encoded) — no longer a body field.
const fileUrl = (sessionId: string, action: string) =>
  `/api/v1/sessions/${encodeURIComponent(sessionId)}/files/${action}`

export const fileApi = {
  // The flat file list backing ONLY the editor's "Search files…" box: a full
  // filesystem walk of the worktree (minus .git/objects and .git/logs), capped
  // by the server's `[server] search_index_max_files` (`truncated` set when the
  // cap was hit). The TREE does not use this — it browses lazily via `tree`.
  // Editing is NOT limited to this set — any path inside the worktree can be
  // read/written/created (the server enforces containment).
  list: (sessionId: string) =>
    postFile<{ files: string[]; truncated?: boolean }>(
      fileUrl(sessionId, "list"),
      {},
    ),
  // One directory's children for the lazy tree. `dir` is worktree-relative; ""
  // lists the worktree root. The server lists exactly this directory (no
  // recursion, no cap). Entries are pre-sorted dirs-first, case-insensitive.
  tree: (sessionId: string, dir: string) =>
    postFile<{ dir: string; entries: DirEntry[] }>(fileUrl(sessionId, "tree"), {
      dir,
    }),
  read: (sessionId: string, path: string) =>
    postFile<WorktreeFile>(fileUrl(sessionId, "read"), { path }),
  // The two raw sides (HEAD vs working copy) of a changed file for the Monaco
  // diff view. The server resolves both sides and the binary flag.
  diff: (sessionId: string, path: string) =>
    postFile<FileDiffContents>(fileUrl(sessionId, "diff"), { path }),
  write: (sessionId: string, path: string, content: string) =>
    postFileNoContent(fileUrl(sessionId, "write"), { path, content }),
  // Open the file in a locally-installed GUI editor (server-side spawn) and
  // resolve with the chosen editor's label for a toast. `editor` is the dux-core
  // editor config key (e.g. "vscode") the user picked; the server launches that
  // one and errors if it isn't installed. Only useful when the server is the
  // user's own machine — the UI gates this to local-access URLs.
  openInEditor: (sessionId: string, path: string, editor: string) =>
    postFile<{ editor: string }>(fileUrl(sessionId, "open-in-editor"), {
      path,
      editor,
    }).then((r) => r.editor),
  // Create a new empty file. Refused (400) if the entry already exists or the
  // parent directory is missing, matching write's create semantics minus the
  // implicit overwrite.
  createFile: (sessionId: string, path: string) =>
    postFileNoContent(fileUrl(sessionId, "create-file"), { path }),
  // Create a new directory, creating missing intermediate components.
  createDir: (sessionId: string, path: string) =>
    postFileNoContent(fileUrl(sessionId, "create-dir"), { path }),
  // Rename/move a file or directory. Refused (400) if the destination already
  // exists (no overwrite).
  rename: (sessionId: string, from: string, to: string) =>
    postFileNoContent(fileUrl(sessionId, "rename"), { from, to }),
  // Permanently delete a file or (recursively) a directory. Named `remove`,
  // not `delete`, to avoid the reserved-word-adjacent name.
  remove: (sessionId: string, path: string) =>
    postFileNoContent(fileUrl(sessionId, "delete"), { path }),
}
