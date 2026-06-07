// Pure filter helper for the changed-files search box. Kept free of React so
// it's trivially unit-testable: case-insensitive substring match on the file
// path. An empty (or whitespace-only) query passes everything through, so the
// list is unfiltered until the user actually types.

import type { ChangedFileView } from "./types"

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
