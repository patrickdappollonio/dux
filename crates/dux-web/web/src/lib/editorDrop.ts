// Reporting a file drop onto the EDITOR'S FILE TREE. Pure: no React, no
// network, no DOM, so every sentence below is unit-testable without mounting
// anything.
//
// # Why this is not `dropToastFor`
//
// dux has two drop INTENTS. A drop on an agent or terminal pane means "look at
// this for me": the file is saved and its PATH IS PASTED into the PTY, and
// `fileDrop.ts`'s `dropToastFor` exists to describe exactly that, which is why
// its whole vocabulary is about delivery (`sent`, `saved-not-sent`, "and pasted
// the path"). A drop on the editor's tree means "add this file to my project":
// it saves a file where the user pointed and pastes nothing, anywhere. There is
// no delivery to report, so every distinction that type carries would have to
// be stubbed out with a lie. The two share what genuinely is shared instead:
// the refusal wording (`dropRefusalReason`), the sentence terminator
// (`endSentence`), and the cap on how many files get named.
//
// The rungs mirror `dropToastFor`'s for the same reason it has them: a bad
// outcome must never be reported as a good one, so the FIRST applicable rung
// wins.

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

/// How the destination folder is named in a sentence.
///
/// The root travels as the empty string, which is correct on the wire and
/// unreadable in prose, so it gets a name.
export function editorDropDirLabel(dir: string): string {
  return dir === "" ? "the worktree root" : dir
}

/// `name (reason)` for up to [`MAX_NAMED_FILES`] files, then a count.
///
/// Capped for the same reason the pane drop caps: a drop of thirty files that
/// all failed the same way produces a toast nobody reads, and the count is the
/// part that matters once the list is long.
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
///
/// This is not decoration. The server suffixes a colliding name instead of
/// overwriting, so a file the user dropped as `notes.md` may be on disk as
/// something else; without this they would go looking for a file that is not
/// there and would not know why.
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

/// The ONE toast for a whole tree drop, chosen from the per-file outcomes.
///
/// The folder is a property of the drop rather than of a file here, unlike the
/// pane drop: every file of one tree drop goes to the one directory the user
/// dropped on, and that directory cannot move underneath them the way a
/// shell's `cd` can.
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
        message: endSentence(
          `Could not save ${refused[0].requestedName}: ${refused[0].reason}`,
        ),
      }
    }
    return {
      tone: "error",
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
      message:
        one.requestedName === one.savedName
          ? `Saved ${one.savedName} to ${where}.`
          : `Saved ${one.requestedName} to ${where} as ${one.savedName}, so nothing was overwritten.`,
    }
  }
  return {
    tone: "success",
    message: `Saved ${saved.length} files to ${where}.` + renameNote(saved),
  }
}

/// What a browser handed over when the user let go, sorted into the three
/// things a drop can actually contain.
///
/// It exists because `dataTransfer.files` is not a list of files. A FOLDER
/// dropped on a file tree is an entirely natural gesture, and what arrives for
/// one is browser-dependent and unpleasant: in the shape this was written
/// against it rides in `files` as an entry whose read fails, so uploading it
/// produces a transport-shaped failure that blames the network for something
/// the user did on purpose. In the other shape nothing arrives at all, and a
/// drop that reports nothing is worse still.
///
/// Both shapes are the reviewer's, INFERRED and not measured here: dropping a
/// real folder cannot be synthesised from a test or a headless driver (the
/// entries come from the OS drag source), so this is written to be correct for
/// either shape rather than to a measurement. What IS measured is the sorting
/// itself, which is pure and tested below.
export interface DroppedItems {
  /// The things that really are files.
  files: File[]
  /// Names of the entries the browser reported as DIRECTORIES.
  folders: string[]
}

/// The subset of `DataTransferItem` this needs, so the sorting is testable
/// without a `DataTransfer` (jsdom builds none).
export interface DroppedItemLike {
  kind: string
  webkitGetAsEntry?: () => { isDirectory: boolean; name: string } | null
}

/// Sort what the browser delivered into files and folders.
///
/// `webkitGetAsEntry` is the only thing that can tell them apart: a directory
/// entry answers `isDirectory`. Anything the items list does not identify as a
/// directory stays a file, so a browser with no entry API degrades to today's
/// behaviour rather than refusing legitimate files.
///
/// Folders are removed from `files` BY NAME, rather than by index, because the
/// two lists only line up when every item is a file: a drag carrying a text
/// item alongside the files shifts `items` and not `files`.
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

/// Swallow a file drop that MISSED every real drop target inside the editor.
///
/// The browser's default action for a dropped file is to NAVIGATE to it, so a
/// drag aimed at the file tree that lands a few pixels off, on Monaco, on the
/// tab strip, or on the panel chrome, throws the whole tab away and takes every
/// unsaved in-memory buffer with it. (`lib/editorDrafts.ts` puts a
/// `beforeunload` prompt in the way, so the work is not lost silently, but a
/// prompt is not a feature.) A drop target is exactly one `preventDefault` away
/// from being safe, and the editor now INVITES this drag, so the whole surface
/// gets a floor under it.
///
/// It only ever swallows: it never uploads and never reports. The real targets
/// are the tree's own rows, which `stopPropagation` before this ancestor
/// handler is reached, so this runs for the misses and nothing else. The cursor
/// says `none` while over the dead zone, which is the honest cue: this area
/// takes nothing.
///
/// Returns whether it acted, for the tests.
export function swallowMissedFileDrop(e: DragEventLike): boolean {
  if (!dragCarriesFiles(e.dataTransfer?.types)) return false
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = "none"
  return true
}

/// Everything [`performTreeDrop`] needs from the outside, injected so the
/// composition is testable without a server, a store or a rendered tree. Same
/// shape as `moveEntry.ts`'s `MoveEntryDeps`, and for the same reason.
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
/// What arrived is a [`DroppedItems`], not a file list, because a drop can also
/// carry FOLDERS or nothing identifiable at all. A folder becomes a named
/// refusal in the same outcome list as everything else, so a mixed drop is
/// still one toast; a drop carrying neither ends immediately with its own
/// message rather than in silence.
///
/// The uploads are SEQUENTIAL, deliberately. The route holds one concurrency
/// permit per in-flight upload and refuses once the wait expires, so firing a
/// twenty-file drop in parallel would make the drop refuse itself. Sequential
/// also keeps the outcomes in the order the user dropped them, which is the
/// order the toast reads them out in.
///
/// A refusal does NOT abandon the rest: refusals here are per-file (an unusable
/// name, a symlink sitting on the name) and stopping would lose files that were
/// perfectly fine.
///
/// The refresh mirrors the move path exactly, because the same two things went
/// stale: the tree's cached listing of the affected directory, and the flat
/// search index. It is skipped entirely when nothing was saved, since there is
/// then nothing to re-read and a refresh would imply otherwise.
export async function performTreeDrop(
  dir: string,
  dropped: DroppedItems,
  deps: TreeDropDeps,
): Promise<void> {
  const { files, folders } = dropped
  const where = editorDropDirLabel(dir)

  // Nothing identifiable arrived. That is a real shape (see `DroppedItems`),
  // and it is the one where saying nothing is worst: the user let go of
  // something and the interface carried on as if they had not.
  if (files.length === 0 && folders.length === 0) {
    deps.reportFinal({
      tone: "error",
      message:
        "Nothing came through in that drop. If you dropped a folder, drop the " +
        "files inside it instead.",
    })
    return
  }

  // A folder is refused BY NAME, before any upload starts, and it joins the
  // same outcome list as everything else so one drop still produces one toast.
  // dux takes files: there is no recursive walk here, and inventing one would
  // silently copy a whole tree into the user's project from a gesture that
  // looks like dropping one thing.
  const outcomes: EditorDropOutcome[] = folders.map((name) => ({
    kind: "refused" as const,
    requestedName: name,
    reason: "dux cannot take a folder, drop its files",
  }))

  // Per FILE, not once per drop, and both halves of that matter. It is the
  // honest progress report for a slow or twenty-file drop, and it is also what
  // keeps the spinner alive: `showBusyToast` arms a leak guard that only a
  // later touch of the same id disarms and rearms, so a single busy call at the
  // start had its spinner silently retired mid-flight on any drop that outlived
  // the guard, leaving nothing on screen until the final toast. The pane drop
  // counts through its files for the same two reasons.
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
      // A `FileDropApiError` carries the server's own words and its status,
      // which is what `dropRefusalReason` turns into advice. Anything else is
      // a bug or an aborted request, and must still become a reported outcome:
      // a rejection nobody catches is a drop that looks like it did nothing.
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
