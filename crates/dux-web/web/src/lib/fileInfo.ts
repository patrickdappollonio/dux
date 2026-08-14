// Pure, React-free formatters for the editor's read-only file-info panel, so
// every judgement the panel makes is unit-testable without mounting anything.
// The shapes mirror `dux_core::worktree_file::WorktreeEntryInfo`.

import { fileStatusMeta } from "@/lib/changedFiles"

// What git has to say about the entry. Genuinely different answers, kept apart
// on the wire (see the Rust `GitStatusView`) because collapsing any two of them
// into a null makes the panel lie. `ignored` and `other_repository` exist
// because `git status` lists NOTHING for either, so without them everything
// under `node_modules` and every vendored subrepo read as "Unmodified".
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
  /** The TARGET's mtime and size, present only for a symlink whose target
   *  could be stat'd. The panel never shows these: they exist because the
   *  editor's freshness check reads THROUGH a link and would otherwise be
   *  comparing the target's stamp against the link's. See `stampFromInfo`. */
  target_modified?: string | null
  target_size?: number | null
  git: GitStatusView
}

// One line of the panel's Git row. `status` is the RAW porcelain code when
// there is one, so the shared FileStatusIcon renders it exactly as the changes
// pane and the file tree do.
export interface GitStatusRow {
  label: string
  status?: string
}

const KIB = 1024
const UNITS = ["KiB", "MiB", "GiB", "TiB"] as const

// Sizes under 1 KiB read as a plain byte count (a 12-byte file saying "0.0
// KiB" helps nobody). Above that, a one-decimal binary unit PLUS the exact
// byte count, because both matter: the unit for scale, the exact number for
// anyone diffing or checking a limit.
export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "—"
  if (bytes < KIB) return bytes === 1 ? "1 byte" : `${bytes} bytes`
  let value = bytes / KIB
  let unit: string = UNITS[0]
  for (let i = 1; i < UNITS.length && value >= KIB; i += 1) {
    value /= KIB
    unit = UNITS[i]
  }
  return `${value.toFixed(1)} ${unit} (${bytes.toLocaleString("en-US")} bytes)`
}

// A timestamp the viewer can read, in THEIR timezone (the server's clock is
// not necessarily the reader's). An unparseable value is passed through rather
// than rendered as "Invalid Date": showing what the server actually sent is
// more useful than showing that the browser's Date constructor gave up.
export function formatModified(iso: string | null): string {
  if (iso === null) return "Unknown"
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

// The porcelain code, spelled out. The NOUN is `fileStatusMeta`'s, not a
// second copy of the vocabulary: one marker and one word per status across the
// whole app, including its deliberate refusal to print an unrecognised code
// (it answers "Changed"; the panel used to leak the raw letter as `Status X`).
// All this adds is which SIDE the change is on, which is the thing a one-word
// "Modified" leaves ambiguous and which the panel has room to say.
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
      // A "changed" answer with neither half set cannot come from the server
      // (it would have been reported clean), but a defensive fallback beats
      // rendering an empty row.
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
