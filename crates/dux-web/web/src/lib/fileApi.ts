// HTTP client for the web code editor: read and write a worktree file's working
// copy. Request/response (like `git.ts`) so the editor can await the content,
// show per-file loading/saving state, and surface a real error message.
//
// The server validates every request (root resolution + that the path stays
// inside that root, a path-escape/`.git`/symlink guard, plus a binary/size
// guard), so the UI never has to. There is NO git-tracked/changed gate: any
// path inside the root is editable, ignored or not. A write against an AGENT
// root triggers an engine changed-files recompute that reaches every client
// over the WebSocket; a terminal root has no agent, so it broadcasts nothing.

import { rootApiBase, type EditorRoot } from "@/lib/editorRoot"
import type { DirEntry } from "@/lib/fileTree"
import type { WorktreeEntryInfo } from "@/lib/fileInfo"

export interface WorktreeFile {
  path: string
  // True when the file is binary — `content` is empty and the editor refuses it.
  binary: boolean
  content: string
  /** True when the server opened this file read-only (outside-resolving symlink
   *  or a .git/ path). The editor must not allow saving. */
  read_only?: boolean
  /** The freshness token for these bytes: the mtime (RFC 3339, from the
   *  server's one shared formatter) and the size, taken from an fstat of the
   *  descriptor the content was read from. The editor keeps it on the buffer,
   *  compares it against `info` to detect a change on disk, and echoes it back
   *  with a save so the server can refuse to clobber somebody else's edit.
   *  Absent from an older server, in which case the guard simply does not
   *  engage. */
  modified?: string | null
  size?: number | null
}

// What a successful save reports back: the file's stamp AFTER the write. The
// editor re-baselines on it, so its own save is never mistaken for an edit by
// something else when the changed-files broadcast lands a moment later.
export interface WriteResult {
  modified: string | null
  size: number
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

// A failed file request, carrying the HTTP status. The status is load-bearing
// for exactly one caller so far: the info panel treats a 404 ("the entry is
// gone") as a reason to dismiss itself and a 400 ("that path is refused") as a
// reason to stay put and show why. Every other caller can keep reading
// `.message` as before, because this still IS an Error.
export class FileApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = "FileApiError"
    this.status = status
  }
}

// A save refused (409) because the file moved underneath the buffer. Carries
// the file's CURRENT stamp so the editor can offer overwrite/reload without
// another round trip, and `deleted` because "gone" is a different rung from
// "changed": there is nothing to reload, only a choice to close or keep.
//
// Declared as a subclass of `FileApiError` so a caller that only reads
// `.message` (or `.status`) still works: an unhandled conflict degrades to the
// ordinary error toast rather than to a silent failure.
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
    // A 409 is the save route's freshness refusal and carries a structured
    // body. It is parsed HERE, in the one place that knows the wire shape, so
    // callers route on an error type instead of on a status code plus a
    // hand-rolled JSON.parse. Anything unparseable falls through to the plain
    // error below, because a conflict the client cannot read is still an error
    // the user must see.
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

// The root's own namespace is the path prefix; see `rootApiBase`. An agent
// root serves from `/api/v1/sessions/<id>` and a terminal root from its
// terminal address, and the server refuses each id outside its own namespace.
const fileUrl = (root: EditorRoot, action: string) =>
  `${rootApiBase(root)}/files/${action}`

export const fileApi = {
  // The flat file list backing ONLY the editor's "Search files…" box: a full
  // filesystem walk of the worktree (minus .git/objects and .git/logs), capped
  // by the server's `[server] search_index_max_files` (`truncated` set when the
  // cap was hit). The TREE does not use this — it browses lazily via `tree`.
  // Editing is NOT limited to this set — any path inside the worktree can be
  // read/written/created (the server enforces containment).
  list: (root: EditorRoot) =>
    postFile<{ files: string[]; truncated?: boolean }>(
      fileUrl(root, "list"),
      {},
    ),
  // One directory's children for the lazy tree. `dir` is worktree-relative; ""
  // lists the worktree root. The server lists exactly this directory (no
  // recursion, no cap). Entries are pre-sorted dirs-first, case-insensitive.
  tree: (root: EditorRoot, dir: string) =>
    postFile<{ dir: string; entries: DirEntry[] }>(fileUrl(root, "tree"), {
      dir,
    }),
  // The read-only facts behind the editor's "File info…" panel: kind, size,
  // modified time, permissions, and what git says about this one path. A
  // missing entry answers 404 (the panel dismisses itself); a refused path
  // answers 400 (the panel says why).
  info: (root: EditorRoot, path: string) =>
    postFile<WorktreeEntryInfo>(fileUrl(root, "info"), { path }),
  read: (root: EditorRoot, path: string) =>
    postFile<WorktreeFile>(fileUrl(root, "read"), { path }),
  // The GET URL that serves a file's raw bytes (same route the markdown
  // preview's asset proxy hits, see `markdownAssetUrl` in lib/markdown.ts): a
  // pure builder, no fetch; the image preview pane hands it straight to an
  // <img src>. The server re-validates worktree containment and caps the
  // response; it already sends Cache-Control: no-cache, so no cache-busting
  // param is needed here.
  rawUrl: (root: EditorRoot, path: string) =>
    `${fileUrl(root, "raw")}?path=${encodeURIComponent(path)}`,
  // The two raw sides (HEAD vs working copy) of a changed file for the Monaco
  // diff view. The server resolves both sides and the binary flag.
  diff: (root: EditorRoot, path: string) =>
    postFile<FileDiffContents>(fileUrl(root, "diff"), { path }),
  // Save a file's working copy, optionally guarded by the freshness token the
  // read handed out. With `expected`, a file that moved on disk since it was
  // read answers 409 and this rejects with a `FileConflictError` carrying the
  // current stamp; without it the write is unconditional, which is what every
  // other writer (and any older page) does. The resolved value is the file's
  // new stamp, which the caller must adopt as its baseline.
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
  // Open the file in a locally-installed GUI editor (server-side spawn) and
  // resolve with the chosen editor's label for a toast. `editor` is the dux-core
  // editor config key (e.g. "vscode") the user picked; the server launches that
  // one and errors if it isn't installed. Only useful when the server is the
  // user's own machine — the UI gates this to local-access URLs.
  openInEditor: (root: EditorRoot, path: string, editor: string) =>
    postFile<{ editor: string }>(fileUrl(root, "open-in-editor"), {
      path,
      editor,
    }).then((r) => r.editor),
  // Create a new empty file. Refused (400) if the entry already exists or the
  // parent directory is missing, matching write's create semantics minus the
  // implicit overwrite.
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
