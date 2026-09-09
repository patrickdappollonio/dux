// Reporting a file drop onto the editor's file tree. Pure: no React, no
// network, no DOM.
//
// Separate from `fileDrop.ts`'s `dropToastFor`, whose vocabulary is delivery
// into a PTY: a tree drop saves a file where the user pointed and pastes
// nothing, so those distinctions could only be stubbed with a lie. Shared
// instead: `dropRefusalReason`, `endSentence` and the cap on how many files get
// named. The first applicable rung wins, so a bad outcome is never reported as
// a good one.

import {
  MAX_NAMED_FILES,
  dragCarriesFiles,
  dropRefusalReason,
  endSentence,
} from "./fileDrop"
import type { DropToast } from "./fileDrop"
import { FileDropApiError } from "./fileDropApi"

/// What became of one file dropped on the tree. Two endings, not three: there
/// is nothing to deliver, so "saved but not delivered" cannot happen.
export type EditorDropOutcome =
  /// Saved. `savedName` differs from `requestedName` when the name was taken
  /// and the server suffixed it rather than overwriting.
  | { kind: "saved"; requestedName: string; savedName: string }
  /// Never saved. The reason is the server's own words wherever it had any.
  | { kind: "refused"; requestedName: string; reason: string }

/// How the destination folder is named in a sentence. The root travels as the
/// empty string, which is correct on the wire and unreadable in prose.
export function editorDropDirLabel(dir: string): string {
  return dir === "" ? "the worktree root" : dir
}

/// `name (reason)` for up to [`MAX_NAMED_FILES`] files, then a count: a drop of
/// many files failing the same way produces a toast nobody reads.
function reasonList(
  refused: { requestedName: string; reason: string }[],
): string {
  const named = refused
    .slice(0, MAX_NAMED_FILES)
    .map((r) => `${r.requestedName} (${r.reason})`)
    .join(", ")
  const rest = refused.length - MAX_NAMED_FILES
  return rest > 0 ? `${named} and ${rest} more` : named
}

/// What every renamed file is now called, appended to any rung that saved one.
/// The server suffixes a colliding name instead of overwriting, so without this
/// the user goes looking for a file that is not on disk under the name they
/// dropped.
function renameNote(saved: { requestedName: string; savedName: string }[]) {
  const renamed = saved.filter((s) => s.requestedName !== s.savedName)
  if (renamed.length === 0) return ""
  const pairs = renamed
    .slice(0, MAX_NAMED_FILES)
    .map((r) => `${r.requestedName} was saved as ${r.savedName}`)
    .join(", ")
  const rest = renamed.length - MAX_NAMED_FILES
  const tail = rest > 0 ? ` and ${rest} more` : ""
  return ` ${pairs}${tail}, so nothing was overwritten.`
}

/// The one toast for a whole tree drop, chosen from the per-file outcomes. The
/// folder belongs to the drop rather than to a file: every file goes to the one
/// directory the user dropped on. Every rung is `sticky: false` deliberately,
/// since nothing reported here is the only copy of anything (the bar
/// `NotifyOptions.sticky` sets): a saved file is in the folder the user pointed
/// at and the tree refreshes to show it, a refused one never left their disk.
export function editorDropToast(
  outcomes: EditorDropOutcome[],
  dir: string,
): DropToast {
  const saved = outcomes.filter((o) => o.kind === "saved")
  const refused = outcomes.filter((o) => o.kind === "refused")
  const where = editorDropDirLabel(dir)

  // 1. Nothing landed.
  if (saved.length === 0) {
    if (refused.length === 1) {
      return {
        tone: "error",
        sticky: false,
        message: endSentence(
          `Could not save ${refused[0].requestedName}: ${refused[0].reason}`,
        ),
      }
    }
    return {
      tone: "error",
      sticky: false,
      message: endSentence(
        `Could not save any of the ${refused.length} dropped files. ${reasonList(refused)}`,
      ),
    }
  }

  // 2. Something landed and something did not. A warning, never a success:
  // the count is the only honest headline.
  if (refused.length > 0) {
    return {
      tone: "warning",
      sticky: false,
      message:
        `Saved ${saved.length} of ${outcomes.length} files to ${where}. ` +
        endSentence(`Refused: ${reasonList(refused)}`) +
        renameNote(saved),
    }
  }

  // 3. Everything landed.
  if (saved.length === 1) {
    const one = saved[0]
    return {
      tone: "success",
      sticky: false,
      message:
        one.requestedName === one.savedName
          ? `Saved ${one.savedName} to ${where}.`
          : `Saved ${one.requestedName} to ${where} as ${one.savedName}, so nothing was overwritten.`,
    }
  }
  return {
    tone: "success",
    sticky: false,
    message: `Saved ${saved.length} files to ${where}.` + renameNote(saved),
  }
}

/// What a browser handed over when the user let go, sorted into files and
/// folders. `dataTransfer.files` is not a list of files: a dropped folder is
/// browser-dependent, either riding in `files` as an entry whose read fails or
/// not arriving at all, so this is written to be correct for either shape
/// rather than to a measurement (a real folder drop cannot be synthesised).
export interface DroppedItems {
  /// The things that really are files.
  files: File[]
  /// Names of the entries the browser reported as directories.
  folders: string[]
}

/// The subset of `DataTransferItem` this needs, so the sorting is testable
/// without a `DataTransfer` (jsdom builds none).
export interface DroppedItemLike {
  kind: string
  webkitGetAsEntry?: () => { isDirectory: boolean; name: string } | null
}

/// Sort what the browser delivered into files and folders. `webkitGetAsEntry`
/// is the only thing that tells them apart, and anything it does not call a
/// directory stays a file, so a browser without the entry API still takes
/// legitimate files. Folders are removed from `files` by name, not by index:
/// the two lists only line up when every item is a file.
export function classifyDroppedItems(
  files: readonly File[],
  items: readonly DroppedItemLike[] | undefined,
): DroppedItems {
  const folders: string[] = []
  for (const item of items ?? []) {
    if (item.kind !== "file" || item.webkitGetAsEntry === undefined) continue
    let entry: { isDirectory: boolean; name: string } | null
    try {
      entry = item.webkitGetAsEntry()
    } catch {
      // An item that refuses to describe itself is left to the file list.
      continue
    }
    if (entry !== null && entry.isDirectory) folders.push(entry.name)
  }
  const folderNames = new Set(folders)
  return { files: files.filter((f) => !folderNames.has(f.name)), folders }
}

/// The little of a drag event this module reads, so the guard below is
/// testable without a DOM. A React `DragEvent` satisfies it structurally.
export interface DragEventLike {
  dataTransfer: { types: readonly string[]; dropEffect?: string } | null
  preventDefault: () => void
}

/// Swallow a file drop that missed every real drop target inside the editor.
/// The browser's default action for a dropped file is to navigate to it, which
/// throws the tab away and takes every unsaved in-memory buffer with it. This
/// only ever swallows: it never uploads and never reports, the real targets are
/// the tree's own rows, which `stopPropagation` before this ancestor handler is
/// reached, and the cursor says `none` over the dead zone. Returns whether it
/// acted.
export function swallowMissedFileDrop(e: DragEventLike): boolean {
  if (!dragCarriesFiles(e.dataTransfer?.types)) return false
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = "none"
  return true
}

/// Everything [`performTreeDrop`] needs from the outside, injected so the
/// composition is testable without a server, a store or a rendered tree.
export interface TreeDropDeps {
  /// Save one file into `dir`, answering with the name it actually got.
  upload: (file: File, dir: string) => Promise<{ saved_name: string }>
  /// Force the tree to re-read these directories past its lazy cache.
  revalidateDirs: (dirs: string[]) => void
  /// Re-index the worktree for the "Search files…" box.
  refreshSearchIndex: () => Promise<void>
  reportBusy: (message: string) => void
  reportFinal: (toast: DropToast) => void
}

/// Save every dropped file into `dir`, then refresh what the new files changed.
///
/// A folder becomes a named refusal in the same outcome list, so a mixed drop
/// is still one toast, and a drop carrying nothing identifiable gets its own
/// message rather than silence. Uploads are sequential: the route holds one
/// concurrency permit per in-flight upload and refuses once the wait expires,
/// and drop order is the order the toast reads. A refusal is per file and never
/// abandons the rest. The tree's cached listing of `dir` and the flat search
/// index are refreshed only when something was saved.
export async function performTreeDrop(
  dir: string,
  dropped: DroppedItems,
  deps: TreeDropDeps,
): Promise<void> {
  const { files, folders } = dropped
  const where = editorDropDirLabel(dir)

  // Nothing identifiable arrived (see `DroppedItems`), the case where silence
  // is worst: the user let go and the interface carried on regardless.
  if (files.length === 0 && folders.length === 0) {
    deps.reportFinal({
      tone: "error",
      // Not sticky: nothing was taken and nothing saved, so nothing to recover.
      sticky: false,
      message:
        "Nothing came through in that drop. If you dropped a folder, drop the " +
        "files inside it instead.",
    })
    return
  }

  // A folder is refused by name before any upload starts and joins the same
  // outcome list, so one drop still produces one toast. There is no recursive
  // walk: inventing one would copy a whole tree from a one-thing gesture.
  const outcomes: EditorDropOutcome[] = folders.map((name) => ({
    kind: "refused" as const,
    requestedName: name,
    reason: "dux cannot take a folder, drop its files",
  }))

  // Per file, not once per drop: it is the progress report for a slow or
  // many-file drop, and it keeps the spinner alive, since `notifyBusy` arms a
  // leak guard that only a later touch of the same id rearms.
  const total = files.length
  for (const [i, file] of files.entries()) {
    deps.reportBusy(
      total === 1
        ? `Saving ${file.name} to ${where}…`
        : `Saving ${file.name} to ${where} (${i + 1} of ${total})…`,
    )
    try {
      const saved = await deps.upload(file, dir)
      outcomes.push({
        kind: "saved",
        requestedName: file.name,
        savedName: saved.saved_name,
      })
    } catch (e) {
      // Anything that is not a `FileDropApiError` must still become a reported
      // outcome: an uncaught rejection is a drop that looks like it did nothing.
      outcomes.push({
        kind: "refused",
        requestedName: file.name,
        reason:
          e instanceof FileDropApiError
            ? dropRefusalReason(e.status, e.message)
            : e instanceof Error
              ? e.message
              : "the upload failed",
      })
    }
  }

  const anySaved = outcomes.some((o) => o.kind === "saved")
  if (anySaved) deps.revalidateDirs([dir])
  deps.reportFinal(editorDropToast(outcomes, dir))
  if (anySaved) await deps.refreshSearchIndex()
}
