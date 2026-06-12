// Shared, React-free changed-files helpers (so they stay trivially
// unit-testable): git-status interpretation (`fileStatusMeta` → kind + label,
// consumed by FileStatusIcon) and the changed-files search filter
// (`filterChangedFiles`: a case-insensitive substring match on the path; an
// empty or whitespace-only query passes everything through).

import type { ChangedFileView } from "./types"

// A file's git status, interpreted once here (kept React-free so it's trivially
// unit-testable) and shared by the changes pane and the editor's file
// tree/search so the marker reads identically everywhere. `kind` selects the
// icon (see FileStatusIcon); `label` is the human-readable tooltip/aria text.
export type FileStatusKind =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "copied"
  | "conflict"
  | "type-changed"
  | "untracked"
  | "other"

export interface FileStatusMeta {
  kind: FileStatusKind
  label: string
}

export function fileStatusMeta(status: string): FileStatusMeta {
  const code = status.trim().toUpperCase()
  // Untracked covers both the porcelain two-char code "??" and a bare "?".
  if (code === "?" || code === "??") {
    return { kind: "untracked", label: "Untracked" }
  }
  // Everything else keys off the first significant char, so porcelain forms like
  // "MM", "R ", or the conflict code "UU" collapse to the same kind as their
  // leading single-letter code.
  switch (code[0]) {
    case "M":
      return { kind: "modified", label: "Modified" }
    case "A":
      return { kind: "added", label: "Added" }
    case "D":
      return { kind: "deleted", label: "Deleted" }
    case "R":
      return { kind: "renamed", label: "Renamed" }
    case "C":
      return { kind: "copied", label: "Copied" }
    case "U":
      return { kind: "conflict", label: "Conflict" }
    case "T":
      return { kind: "type-changed", label: "Type changed" }
    default:
      // Unknown code — show a neutral label rather than leaking the raw letter.
      return { kind: "other", label: "Changed" }
  }
}

export function filterChangedFiles(
  files: ChangedFileView[],
  query: string,
): ChangedFileView[] {
  const needle = query.trim().toLowerCase()
  if (needle === "") return files
  return files.filter((f) => f.path.toLowerCase().includes(needle))
}

// The changed-files engine state (`watched_worktree`/`changed_files`) is GLOBAL
// and broadcast to every client, but selection is per-client. So a client must
// only trust the broadcast lists when they belong to the session it actually has
// selected; otherwise it would briefly show another tab's session's files. This
// is true exactly when the ViewModel's `watched_session_id` matches the locally
// selected session. Returns false while nothing is selected, or while the server
// hasn't caught up to this client's latest selection (the "loading" window).
export function shouldShowChangedFiles(
  watchedSessionId: string | null,
  selectedSessionId: string | null,
): boolean {
  return selectedSessionId !== null && watchedSessionId === selectedSessionId
}
