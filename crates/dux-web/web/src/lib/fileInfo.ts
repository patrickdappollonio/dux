// Pure, React-free formatters for the editor's read-only file-info panel. The
// shapes mirror `dux_core::worktree_file::WorktreeEntryInfo`.

import { fileStatusMeta } from "@/lib/changedFiles"

// What git has to say about the entry, kept apart on the wire (see the Rust
// `GitStatusView`) because collapsing any two answers into a null makes the
// panel lie: `git status` lists nothing for an ignored path or a nested
// repository, which would otherwise read as "Unmodified".
export type GitStatusView =
  | { state: "not_a_repository" }
  | { state: "other_repository" }
  | { state: "not_applicable" }
  | { state: "ignored" }
  | { state: "clean" }
  | { state: "changed"; staged: string | null; unstaged: string | null }

export type EntryKind = "file" | "dir" | "symlink" | "other"

export interface WorktreeEntryInfo {
  path: string
  kind: EntryKind
  /** null for a directory: a directory's on-disk entry size is not a fact
   *  anybody wants. */
  size: number | null
  /** RFC 3339, UTC, or null when the filesystem reports no mtime. */
  modified: string | null
  /** Octal permission bits without a leading zero, e.g. "644". */
  mode: string
  /** The same bits as `ls -l` prints them, e.g. "rw-r--r--". */
  permissions: string
  /** A symlink's target as stored on disk (not resolved). */
  symlink_target: string | null
  /** The target's mtime and size, present only for a symlink whose target could
   *  be stat'd. The panel never shows these: the freshness check reads through a
   *  link and would otherwise compare the target's stamp against the link's.
   *  See `stampFromInfo`. */
  target_modified?: string | null
  target_size?: number | null
  git: GitStatusView
}

// One line of the panel's Git row. `status` is the raw porcelain code when there
// is one, so the shared `FileStatusIcon` renders it as every other surface does.
export interface GitStatusRow {
  label: string
  status?: string
}

const KIB = 1024
const UNITS = ["KiB", "MiB", "GiB", "TiB"] as const

// Sizes under 1 KiB read as a plain byte count; above that, a one-decimal binary
// unit for scale plus the exact byte count for anyone checking a limit.
export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "-"
  if (bytes < KIB) return bytes === 1 ? "1 byte" : `${bytes} bytes`
  let value = bytes / KIB
  let unit: string = UNITS[0]
  for (let i = 1; i < UNITS.length && value >= KIB; i += 1) {
    value /= KIB
    unit = UNITS[i]
  }
  return `${value.toFixed(1)} ${unit} (${bytes.toLocaleString("en-US")} bytes)`
}

// A timestamp in the viewer's own timezone, which the server's need not be. An
// unparseable value is passed through rather than shown as "Invalid Date": what
// the server sent is more useful than the Date constructor giving up.
export function formatModified(iso: string | null): string {
  if (iso === null) return "Unknown"
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

// The porcelain code, spelled out. The noun comes from `fileStatusMeta` so the
// app keeps one word per status; all this adds is which side the change is on.
function codeLabel(code: string, staged: boolean): string {
  const meta = fileStatusMeta(code)
  // Untracked and conflicted have no staged/unstaged half to name: the file is
  // in neither state, it is in that one.
  if (meta.kind === "untracked" || meta.kind === "conflict") return meta.label
  return staged ? `${meta.label}, staged` : `${meta.label}, not staged`
}

export function gitStatusRows(git: GitStatusView): GitStatusRow[] {
  switch (git.state) {
    case "not_a_repository":
      return [{ label: "Not a git repository" }]
    case "other_repository":
      return [
        { label: "In a different git repository (a nested repo or submodule)" },
      ]
    case "not_applicable":
      return [{ label: "Not tracked: git tracks files, not folders" }]
    case "ignored":
      return [{ label: "Ignored by git" }]
    case "clean":
      return [{ label: "Unmodified" }]
    case "changed": {
      const rows: GitStatusRow[] = []
      if (git.staged !== null) {
        rows.push({ label: codeLabel(git.staged, true), status: git.staged })
      }
      if (git.unstaged !== null) {
        rows.push({ label: codeLabel(git.unstaged, false), status: git.unstaged })
      }
      // A "changed" answer with neither half set cannot come from the server,
      // but a fallback beats rendering an empty row.
      return rows.length > 0 ? rows : [{ label: "Unmodified" }]
    }
  }
}

// How the entry's kind reads in the panel.
export function entryKindLabel(kind: EntryKind): string {
  switch (kind) {
    case "file":
      return "File"
    case "dir":
      return "Folder"
    case "symlink":
      return "Symbolic link"
    case "other":
      return "Special file"
  }
}
