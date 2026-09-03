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

// A group's aggregate recap: how many files, how many lines they add and
// remove between them, and how many of them are binary. Binary files carry no
// line counts (the wire reports zeroes for them), so they contribute nothing to
// the sums and are counted separately instead, which is what lets the header
// say "no lines here, these are binaries" rather than a bare "+0 −0".
export interface ChangedFilesRecap {
  count: number
  additions: number
  deletions: number
  binaryCount: number
}

// The recap describes exactly the rows visible beneath it, so callers pass the
// FILTERED list, never the source one.
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

// A recap's line count, abbreviated so a large sum cannot crowd out the file
// count beside it: under a thousand it is printed as it is, and from a thousand
// up it reads in thousands with one decimal, trimmed when that decimal is zero
// (1000 -> "1k", 1300 -> "1.3k", 12345 -> "12.3k").
//
// The decimal is TRUNCATED rather than rounded, so the figure never claims more
// lines than there are: 1999 reads "1.9k", never "2k". Only LINE counts
// abbreviate: file counts and the binary count stay raw, because a count of
// files is a small number the user is meant to read exactly, and so do the
// per-row +N -N badges, which are data beside a path. There is deliberately no
// "M" step above this: one unit is one thing to learn, and a diff that large is
// past the point where the exact figure matters. The TUI's `format_recap_count`
// answers the same cases identically.
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

// One section's worth of checked paths each. Staged and unstaged are kept apart
// because the two sections carry opposite verbs.
export interface ChangedFileSelection {
  staged: Set<string>
  unstaged: Set<string>
}

// Drop every checked path that is no longer in the section it was checked in. A
// file selected to be staged and then staged is no longer selected to stage, so
// it leaves the set rather than following the file across. Pure and applied at
// render, so keeping the selection honest across a refresh needs no effect.
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
