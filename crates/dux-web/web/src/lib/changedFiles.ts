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
      // Unknown code: show a neutral label rather than leaking the raw letter.
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
// contribute nothing to the sums and are counted separately instead, and so do the
// files the repository excludes from diffs, which are counted apart from the
// binaries because they are text git merely refuses to diff.
export interface ChangedFilesRecap {
  count: number
  additions: number
  deletions: number
  binaryCount: number
  diffExcludedCount: number
}

// How many files one row stands for: a folded folder counts the files inside
// it, so a total never reads one for a folder of thirty thousand. A nested
// repository is one entry, since nothing looks inside it.
export function changedFileCount(file: ChangedFileView): number {
  return file.kind === "directory" ? (file.file_count ?? 0) : 1
}

// A folded folder's count in words ("28,747 files", "nested repository"), or
// null for an ordinary file row. The TUI's `folder_count_label` says the same.
export function folderCountLabel(file: ChangedFileView): string | null {
  if (file.kind === "nested_repository") return "nested repository"
  if (file.kind === "linked_worktree") return "worktree of this repository"
  if (file.kind !== "directory") return null
  const files = countWords(file.file_count ?? 0, "file", "files")
  const nested = file.nested_repositories ?? 0
  if (nested === 0) return files
  const repositories = countWords(nested, "nested repository", "nested repositories")
  return (file.file_count ?? 0) === 0 ? repositories : `${files} and ${repositories}`
}

// A count with its noun, grouped by thousands the way the TUI writes it.
export function countWords(count: number, one: string, many: string): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`
}

// Whether discarding this row would delete anything. A folder holding only
// repositories of their own has nothing a delete takes (they are kept), and a
// worktree of this same repository belongs to the worktree manager; the server
// refuses both, so neither is offered. The TUI refuses the same two.
export function discardActsOn(file: ChangedFileView): boolean {
  if (file.kind === "linked_worktree") return false
  if (file.kind === "directory") return (file.file_count ?? 0) > 0
  return true
}

// Whether staging this row does what staging says. A worktree of this same
// repository would be recorded as a link to it, which the server refuses, so
// it is not offered and a bulk stage leaves it out.
export function stageActsOn(file: ChangedFileView): boolean {
  return file.kind !== "linked_worktree"
}

// Why a discard leaves this row out, or null when it acts on it. The same
// rule as `discardActsOn`, worded for the dialog that lists what it skipped.
export function discardLeftOutReason(file: ChangedFileView): string | null {
  if (file.kind === "linked_worktree") {
    return `${file.path}/ is a worktree of this repository, which the worktree manager removes`
  }
  if (file.kind === "directory" && (file.file_count ?? 0) === 0) {
    return `${file.path}/ holds only repositories of their own, which a delete keeps`
  }
  return null
}

// The recap describes exactly the rows visible beneath it, so callers pass the
// filtered list, never the source one.
export function summarizeChangedFiles(
  files: ChangedFileView[],
): ChangedFilesRecap {
  const recap: ChangedFilesRecap = {
    count: files.reduce((sum, file) => sum + changedFileCount(file), 0),
    additions: 0,
    deletions: 0,
    binaryCount: 0,
    diffExcludedCount: 0,
  }
  for (const file of files) {
    if (file.binary) {
      recap.binaryCount += 1
      continue
    }
    if (file.diff_excluded) {
      recap.diffExcludedCount += 1
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
    diffExcludedCount: a.diffExcludedCount + b.diffExcludedCount,
  }
}

// Every field of the wire type, typed against its keys: a field added to
// `ChangedFileView` stops this compiling until it is listed, and every listed
// field is compared. All of them are primitives, so `===` is the comparison.
export const CHANGED_FILE_FIELDS = {
  status: true,
  path: true,
  additions: true,
  deletions: true,
  binary: true,
  diff_excluded: true,
  renamed_from: true,
  kind: true,
  file_count: true,
  nested_repositories: true,
  fingerprint: true,
} as const satisfies Record<keyof ChangedFileView, true>

const FIELD_KEYS = Object.keys(CHANGED_FILE_FIELDS) as (keyof ChangedFileView)[]

function sameChangedFile(a: ChangedFileView, b: ChangedFileView): boolean {
  return FIELD_KEYS.every((key) => a[key] === b[key])
}

// A freshly fetched list with every file that did not change swapped back for
// the object already on screen, and the previous array itself when nothing
// changed at all. Everything downstream memoizes by reference, so a refetch
// that moved one file re-renders one row, and one that moved nothing re-renders
// none.
export function reuseUnchangedFiles(
  previous: ChangedFileView[],
  next: ChangedFileView[],
): ChangedFileView[] {
  if (previous === next) return previous
  if (
    previous.length === next.length &&
    next.every((file, index) => sameChangedFile(previous[index]!, file))
  ) {
    return previous
  }
  if (previous.length === 0) return next
  const byPath = new Map(previous.map((file) => [file.path, file]))
  return next.map((file) => {
    const old = byPath.get(file.path)
    return old && sameChangedFile(old, file) ? old : file
  })
}

// One section's worth of checked paths each. Staged and unstaged are kept apart
// because the two sections carry opposite verbs.
export interface ChangedFileSelection {
  staged: Set<string>
  unstaged: Set<string>
}

// Drop every checked path that is no longer in the section it was checked in: a file
// checked to be staged and then staged leaves the set rather than following the file across.
//
// A section whose checked paths all survive is returned as the SAME set, and the
// whole selection as the same object when both do, so a caller can tell "nothing
// changed" by reference. An empty section costs nothing, which is the common case
// for a pane listing tens of thousands of files and checking none of them.
export function reconcileSelection(
  prev: ChangedFileSelection,
  slice: { staged: ChangedFileView[]; unstaged: ChangedFileView[] },
): ChangedFileSelection {
  const survivors = (checked: Set<string>, files: ChangedFileView[]) => {
    if (checked.size === 0) return checked
    const live = new Set(files.map((f) => f.path))
    for (const path of checked) {
      if (!live.has(path)) {
        return new Set([...checked].filter((kept) => live.has(kept)))
      }
    }
    return checked
  }
  const staged = survivors(prev.staged, slice.staged)
  const unstaged = survivors(prev.unstaged, slice.unstaged)
  if (staged === prev.staged && unstaged === prev.unstaged) return prev
  return { staged, unstaged }
}
