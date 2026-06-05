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
