// HTTP client for the web code editor: read and write a worktree file's working
// copy. Request/response (like `git.ts`) so the editor can await the content,
// show per-file loading and saving state, and surface a real error message.
//
// The server validates every request (root resolution, containment, the
// path-escape, `.git` and symlink guards, the binary and size caps), so the UI
// never has to, and there is no git-tracked gate: any path inside the root is
// editable, ignored or not. A write against an agent root triggers a
// changed-files recompute broadcast to every client; a terminal root has no
// agent and broadcasts nothing.

import { rootApiBase, type EditorRoot } from "@/lib/editorRoot"
import type { DirEntry } from "@/lib/fileTree"
import type { WorktreeEntryInfo } from "@/lib/fileInfo"

export interface WorktreeFile {
  path: string
  // True when the file is binary: `content` is empty and the editor refuses it.
  binary: boolean
  content: string
  /** True when the server opened this file read-only (outside-resolving symlink
   *  or a .git/ path). The editor must not allow saving. */
  read_only?: boolean
  /** The freshness token for these bytes: RFC 3339 mtime plus size, from an
   *  fstat of the descriptor the content was read from. The editor compares it
   *  against `info` to detect a change on disk and echoes it back with a save so
   *  the server can refuse to clobber another writer's edit. Absent from an
   *  older server, in which case the guard does not engage. */
  modified?: string | null
  size?: number | null
}

// What a successful save reports back: the file's stamp after the write, which
// the editor re-baselines on so its own save is not read as somebody else's.
export interface WriteResult {
  modified: string | null
  size: number
}


// The two raw sides of a changed file (HEAD vs working copy) for the Monaco
// diff view. Either side is "" when absent; `binary` means neither is
// renderable text. Mirrors the Rust `DiffContents`.
export interface FileDiffContents {
  path: string
  original: string
  modified: string
  binary: boolean
}

// The head of git's own patch, answered for a version past the size ceiling the
// editor can hold. Mirrors the Rust `DiffHead`.
export interface FileDiffHead {
  text: string
  shown_lines: number
  total_lines: number
  truncated: boolean
  total_is_at_least: boolean
  binary: boolean
}

export interface FileDiffHeadAnswer {
  path: string
  head: FileDiffHead
}

// What the diff endpoint answers with. The two shapes are told apart by which
// key is there, matching the server's untagged encoding.
export type FileDiffAnswer = FileDiffContents | FileDiffHeadAnswer

export function isDiffHeadAnswer(
  answer: FileDiffAnswer,
): answer is FileDiffHeadAnswer {
  return "head" in answer
}

// A failed file request, carrying the HTTP status: the info panel treats a 404
// (the entry is gone) as a reason to dismiss itself and a 400 (the path is
// refused) as a reason to stay put and show why. Still an Error, so a caller
// reading only `.message` is unaffected.
export class FileApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = "FileApiError"
    this.status = status
  }
}

// A save refused (409) because the file moved underneath the buffer. Carries
// the file's current stamp so the editor can offer overwrite or reload without
// another round trip, and `deleted`, which is a different rung: there is nothing
// to reload, only a choice to close or keep. A subclass of `FileApiError` so an
// unhandled conflict degrades to the ordinary error toast, not a silent failure.
export class FileConflictError extends FileApiError {
  readonly modified: string | null
  readonly size: number | null
  readonly deleted: boolean
  constructor(body: {
    modified: string | null
    size: number | null
    deleted: boolean
  }) {
    super(
      409,
      body.deleted
        ? "the file was deleted on disk after you opened it"
        : "the file changed on disk after you opened it",
    )
    this.name = "FileConflictError"
    this.modified = body.modified
    this.size = body.size
    this.deleted = body.deleted
  }
}

async function postFile<T>(path: string, body: Record<string, unknown>): Promise<T> {
  const resp = await fetch(path, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  })
  if (!resp.ok) {
    // The save route's freshness refusal carries a structured body, parsed here
    // so callers route on an error type rather than a status plus their own
    // JSON.parse. An unparseable one falls through to the plain error below.
    if (resp.status === 409) {
      const body = await resp
        .clone()
        .json()
        .catch(() => null)
      if (body !== null && typeof body === "object" && "deleted" in body) {
        const b = body as { modified?: string | null; size?: number | null; deleted?: boolean }
        throw new FileConflictError({
          modified: b.modified ?? null,
          size: b.size ?? null,
          deleted: b.deleted === true,
        })
      }
    }
    const detail = (await resp.text().catch(() => "")).trim()
    throw new FileApiError(
      resp.status,
      detail || `request failed (${resp.status})`,
    )
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
    throw new FileApiError(
      resp.status,
      detail || `request failed (${resp.status})`,
    )
  }
}

// The root's own namespace is the path prefix (see `rootApiBase`), and the
// server refuses each id outside its own namespace.
const fileUrl = (root: EditorRoot, action: string) =>
  `${rootApiBase(root)}/files/${action}`

export const fileApi = {
  // The flat file list backing only the "Search files…" box: a full walk of the
  // worktree (minus .git/objects and .git/logs), capped by the server's
  // `[server] search_index_max_files`, with `truncated` set when the cap was
  // hit. The tree browses lazily via `tree`, and editing is not limited to this
  // set: any path inside the worktree can be read, written or created.
  list: (root: EditorRoot) =>
    postFile<{ files: string[]; truncated?: boolean }>(
      fileUrl(root, "list"),
      {},
    ),
  // One directory's children for the lazy tree. `dir` is worktree-relative and
  // "" is the root; no recursion and no cap, entries pre-sorted dirs-first,
  // case-insensitive.
  tree: (root: EditorRoot, dir: string) =>
    postFile<{ dir: string; entries: DirEntry[] }>(fileUrl(root, "tree"), {
      dir,
    }),
  // The read-only facts behind the "File info…" panel. A missing entry answers
  // 404 (the panel dismisses itself); a refused path answers 400 (it says why).
  info: (root: EditorRoot, path: string) =>
    postFile<WorktreeEntryInfo>(fileUrl(root, "info"), { path }),
  read: (root: EditorRoot, path: string) =>
    postFile<WorktreeFile>(fileUrl(root, "read"), { path }),
  // The GET URL serving a file's raw bytes (the route `markdownAssetUrl` in
  // lib/markdown.ts also hits): a pure builder, no fetch. The server sends
  // Cache-Control: no-cache, so no cache-busting param is needed here.
  rawUrl: (root: EditorRoot, path: string) =>
    `${fileUrl(root, "raw")}?path=${encodeURIComponent(path)}`,
  // The two raw sides (HEAD vs working copy) of a changed file for the Monaco
  // diff view. The server resolves both sides and the binary flag, and answers
  // with the head of git's own patch for a version it will not send whole.
  diff: (root: EditorRoot, path: string) =>
    postFile<FileDiffAnswer>(fileUrl(root, "diff"), { path }),
  // Save a file's working copy. With `expected`, the freshness token the read
  // handed out, a file that moved on disk answers 409 and this rejects with a
  // `FileConflictError` carrying the current stamp; without it the write is
  // unconditional. The resolved stamp is the caller's new baseline.
  write: (
    root: EditorRoot,
    path: string,
    content: string,
    expected?: { modified: string | null; size: number | null },
  ) =>
    postFile<WriteResult>(fileUrl(root, "write"), {
      path,
      content,
      // Both halves or neither: the server treats half a token as no token,
      // and sending one half would only look like a guard.
      ...(expected && expected.modified !== null && expected.size !== null
        ? { expected_modified: expected.modified, expected_size: expected.size }
        : {}),
    }),
  // Open the file in a locally-installed GUI editor (a server-side spawn),
  // resolving with the chosen editor's label. `editor` is the dux-core editor
  // config key; the spawn only helps when the server is the user's own machine,
  // so the UI gates this to local-access URLs.
  openInEditor: (root: EditorRoot, path: string, editor: string) =>
    postFile<{ editor: string }>(fileUrl(root, "open-in-editor"), {
      path,
      editor,
    }).then((r) => r.editor),
  // Create a new empty file. Refused (400) when the entry already exists or the
  // parent directory is missing.
  createFile: (root: EditorRoot, path: string) =>
    postFileNoContent(fileUrl(root, "create-file"), { path }),
  // Create a new directory, creating missing intermediate components.
  createDir: (root: EditorRoot, path: string) =>
    postFileNoContent(fileUrl(root, "create-dir"), { path }),
  // Rename/move a file or directory. Refused (400) if the destination already
  // exists (no overwrite).
  rename: (root: EditorRoot, from: string, to: string) =>
    postFileNoContent(fileUrl(root, "rename"), { from, to }),
  // Permanently delete a file or (recursively) a directory. Named `remove`,
  // not `delete`, to avoid the reserved-word-adjacent name.
  remove: (root: EditorRoot, path: string) =>
    postFileNoContent(fileUrl(root, "delete"), { path }),
}
