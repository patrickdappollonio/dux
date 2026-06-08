// Pure filter helper for the changed-files search box. Kept free of React so
// it's trivially unit-testable: case-insensitive substring match on the file
// path. An empty (or whitespace-only) query passes everything through, so the
// list is unfiltered until the user actually types.

import type { ChangedFileView } from "./types"

// Map a raw git status code to a short display glyph, shared by the changes pane
// and the editor's file list so they stay consistent.
export function statusGlyph(status: string): string {
  const upper = status.toUpperCase()
  switch (upper) {
    case "M":
      return "M"
    case "A":
      return "A"
    case "D":
      return "D"
    case "?":
    case "??":
      return "?"
    case "R":
      return "R"
    default:
      return status.slice(0, 1).toUpperCase() || "?"
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
