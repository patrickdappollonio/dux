// What a discard did, in words, for the one toast every discard raises.
//
// A destructive act is always confirmed once it lands, and a folded folder
// makes the row count a poor measure of what went: one row can be 28,747 files,
// or a whole repository with its history. The single-row dialog and the bulk
// bar both word their toast here, so the two cannot describe the same act
// differently. The single-row sentences are the engine's own `DiscardFile`
// statuses (dux-core `engine::command`), which reach the terminal UI but not
// the browser's route, built again here with the file count added.

import { changedFileCount, countWords, fileStatusMeta } from "@/lib/changedFiles"
import { chip, prose, type Prose } from "@/lib/prose"
import type { ChangedFileView } from "@/lib/types"

/** How many files the checked `paths` of one section stand for. */
export function selectedFileCount(
  paths: ReadonlySet<string>,
  files: readonly ChangedFileView[],
): number {
  if (paths.size === 0) return 0
  let count = 0
  let found = 0
  for (const file of files) {
    if (!paths.has(file.path)) continue
    count += changedFileCount(file)
    found += 1
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
