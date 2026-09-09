// React-free changed-files helpers: git-status interpretation and the changed-files
// search filter, a case-insensitive substring match an empty query passes through.

import type { ChangedFileView } from "./types"

// A file's git status interpreted once, shared by the changes pane and the editor's file
// tree so the marker reads identically. `kind` selects the icon, `label` is the aria text.
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
  // Everything else keys off the first significant char, so "MM", "R " and "UU"
  // collapse to the kind of their leading letter.
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

// A group's aggregate recap. Binary files carry no line counts on the wire, so they
// contribute nothing to the sums and are counted separately instead.
export interface ChangedFilesRecap {
  count: number
  additions: number
  deletions: number
  binaryCount: number
}

// The recap describes exactly the rows visible beneath it, so callers pass the
// filtered list, never the source one.
export function summarizeChangedFiles(
  files: ChangedFileView[],
): ChangedFilesRecap {
  const recap: ChangedFilesRecap = {
    count: files.length,
    additions: 0,
    deletions: 0,
    binaryCount: 0,
  }
  for (const file of files) {
    if (file.binary) {
      recap.binaryCount += 1
      continue
    }
    recap.additions += file.additions
    recap.deletions += file.deletions
  }
  return recap
}

// A recap's line count, abbreviated from a thousand up in thousands with one decimal,
// trimmed when that decimal is zero (1300 -> "1.3k"). The decimal is truncated, never
// rounded, so the figure never claims more lines than there are, and there is no "M"
// step above it. Only line counts abbreviate: file and binary counts and the per-row
// badges stay raw. The TUI's `format_recap_count` answers the same cases identically.
export function formatRecapCount(n: number): string {
  if (n < 1000) return String(n)
  const thousands = Math.floor(n / 1000)
  const tenths = Math.floor((n % 1000) / 100)
  return tenths === 0 ? `${thousands}k` : `${thousands}.${tenths}k`
}

// Two recaps added together, for the header's whole-pane figure over both
// groups' visible rows.
export function mergeChangedFilesRecaps(
  a: ChangedFilesRecap,
  b: ChangedFilesRecap,
): ChangedFilesRecap {
  return {
    count: a.count + b.count,
    additions: a.additions + b.additions,
    deletions: a.deletions + b.deletions,
    binaryCount: a.binaryCount + b.binaryCount,
  }
}

// The changed-files broadcast is global but selection is per client, so a client trusts
// the lists only while `watched_session_id` matches its own selection; otherwise it would
// briefly show another tab's files. False while nothing is selected, or while the server
// has not caught up to this client's latest selection.
export function shouldShowChangedFiles(
  watchedSessionId: string | null,
  selectedSessionId: string | null,
): boolean {
  return selectedSessionId !== null && watchedSessionId === selectedSessionId
}

// One section's worth of checked paths each. Staged and unstaged are kept apart
// because the two sections carry opposite verbs.
export interface ChangedFileSelection {
  staged: Set<string>
  unstaged: Set<string>
}

// Drop every checked path that is no longer in the section it was checked in: a file
// checked to be staged and then staged leaves the set rather than following the file across.
export function reconcileSelection(
  prev: ChangedFileSelection,
  slice: { staged: ChangedFileView[]; unstaged: ChangedFileView[] },
): ChangedFileSelection {
  const survivors = (checked: Set<string>, files: ChangedFileView[]) => {
    const live = new Set(files.map((f) => f.path))
    return new Set([...checked].filter((path) => live.has(path)))
  }
  return {
    staged: survivors(prev.staged, slice.staged),
    unstaged: survivors(prev.unstaged, slice.unstaged),
  }
}
