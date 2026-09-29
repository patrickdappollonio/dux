// What a discard did, in words, for the one toast every discard raises.
//
// A destructive act is always confirmed once it lands, and a folded folder
// makes the row count a poor measure of what went: one row can be 28,747 files,
// or a whole repository with its history. The single-row dialog and the bulk
// bar both word their toast here, so the two cannot describe the same act
// differently. The single-row sentences are the engine's own `DiscardFile`
// statuses (dux-core `engine::command`), which reach the terminal UI but not
// the browser's route, built again here with the file count added.

import {
  changedFileCount,
  countWords,
  fileStatusMeta,
  joinWords,
  onlyRepositoriesWords,
} from "@/lib/changedFiles"
import type { LeftOut } from "@/lib/git"
import { chip, prose, type Prose } from "@/lib/prose"
import type { ChangedFileView } from "@/lib/types"

/**
 * What a stage left out, as a sentence, or null when it left nothing out. The
 * terminal UI's folder stage says the same.
 */
export function leftOutNotice(report: LeftOut): string | null {
  const repositories = report.left_out_repositories ?? 0
  const worktrees = report.left_out_worktrees ?? 0
  if (repositories + worktrees === 0) return null
  const parts: string[] = []
  if (repositories > 0) {
    parts.push(countWords(repositories, "nested repository", "nested repositories"))
  }
  if (worktrees > 0) {
    parts.push(
      countWords(worktrees, "worktree of this repository", "worktrees of this repository"),
    )
  }
  return `Staged without the repositories inside: left out ${joinWords(parts)}, which staging would record as links to repositories, not as files.`
}

/** How many files the checked `paths` of one section stand for. */
export function selectedFileCount(
  paths: ReadonlySet<string>,
  files: readonly ChangedFileView[],
  // Rows the act would leave out (see `discardActsOn`, `stageActsOn`) are
  // not counted at all.
  actsOn: (file: ChangedFileView) => boolean = () => true,
): number {
  if (paths.size === 0) return 0
  let count = 0
  let found = 0
  for (const file of files) {
    if (!paths.has(file.path)) continue
    found += 1
    if (actsOn(file)) count += changedFileCount(file)
  }
  // A checked path that already left the list still counts as one, the way
  // the bar counted it before folders existed.
  return count + (paths.size - found)
}

/** The toast for discarding `rows` successfully. */
export function discardOutcome(rows: readonly ChangedFileView[]): Prose {
  if (rows.length === 1) return oneRow(rows[0]!)
  let files = 0
  let repositories = 0
  let folders = 0
  for (const row of rows) {
    if (row.kind === "nested_repository") {
      repositories += 1
      continue
    }
    if (row.kind === "directory") folders += 1
    files += changedFileCount(row)
  }
  const parts: string[] = []
  if (files > 0) parts.push(`Discarded the changes to ${countWords(files, "file", "files")}`)
  if (repositories > 0) {
    const what = countWords(repositories, "repository of its own", "repositories of their own")
    const their = repositories === 1 ? "its" : "their"
    parts.push(
      files > 0
        ? `deleted ${what} with ${their} history`
        : `Deleted ${what} with ${their} history`,
    )
  }
  let sentence = `${parts.join(", and ")}.`
  if (folders > 0) {
    sentence +=
      " Files the repository ignores and repositories of their own inside the folders are kept."
  }
  return prose`${sentence}`
}

function oneRow(row: ChangedFileView): Prose {
  switch (row.kind) {
    case "directory":
      return prose`Deleted the untracked files in ${chip(`${row.path}/`)} (${countWords(
        row.file_count ?? 0,
        "file",
        "files",
      )}); files the repository ignores and repositories of their own inside it are kept.`
    case "nested_repository":
      return prose`Deleted ${chip(`${row.path}/`)}, a repository of its own, with its history.`
    case "linked_worktree":
    case undefined:
      break
  }
  if (fileStatusMeta(row.status).kind === "untracked") {
    return prose`Deleted the untracked file ${chip(row.path)}.`
  }
  return prose`Discarded the unstaged changes to ${chip(row.path)}. Staged changes, if any, are kept.`
}

// Why a discard leaves this row out, or null when it acts on it. The same
// rule as `discardActsOn`, worded for the dialog that lists what it skipped.
export function discardLeftOutReason(file: ChangedFileView): Prose | null {
  if (file.kind === "linked_worktree") {
    return prose`${chip(`${file.path}/`)} is a worktree of this repository, which the worktree manager removes`
  }
  if (file.kind === "directory" && (file.file_count ?? 0) === 0) {
    return prose`${chip(`${file.path}/`)} holds ${onlyRepositoriesWords(file)}, which a delete keeps`
  }
  return null
}

// What a row is, in the words a changed-kind notice uses.
function whatItIs(file: ChangedFileView): string {
  switch (file.kind) {
    case "directory":
      return "an ordinary folder"
    case "nested_repository":
      return "a repository of its own, with a history"
    case "linked_worktree":
      return "a worktree of this repository"
    case undefined:
      return "a file"
  }
}

// The notice for a discard dialog that closed because a row it was opened on
// changed kind underneath it: what was confirmed is no longer what is there,
// so nothing is deleted and the user looks again.
export function changedWhileOpen(before: ChangedFileView, now: ChangedFileView): Prose {
  const name = before.kind || now.kind ? `${before.path}/` : before.path
  return prose`${chip(name)} changed while the dialog was open: it is now ${whatItIs(
    now,
  )}. Nothing was deleted; look at it again before deleting it.`
}

// The first row of `openedOn` whose live row in `live` is now a different
// kind, with that live row; null when none changed.
export function firstChangedKind(
  openedOn: readonly ChangedFileView[],
  live: readonly ChangedFileView[],
): { before: ChangedFileView; now: ChangedFileView } | null {
  if (openedOn.length === 0) return null
  const byPath = new Map(live.map((row) => [row.path, row]))
  for (const before of openedOn) {
    const now = byPath.get(before.path)
    if (now !== undefined && now.kind !== before.kind) return { before, now }
  }
  return null
}
