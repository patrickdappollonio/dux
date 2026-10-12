import type {
  FileDiffContents,
  FileDiffHead,
  SaveVerdict,
  UnwritableReason,
  WorktreeFile,
} from "./fileApi"
import { encodingAfterSave, type SaveEncoding } from "./editorEncoding"

// The server's freshness token for a file's bytes: RFC 3339 mtime plus byte
// size. Both halves travel together and are compared together; see `stampsDiffer`.
export interface FileStamp {
  modified: string | null
  size: number | null
}

// What the last disk check found for a buffer:
//
//   fresh    nothing known to have changed.
//   changed  disk bytes differ and the buffer has unsaved edits, so it cannot
//            reload silently; the banner is up.
//   paused   the same difference on a clean buffer, held back only because a
//            live selection would be collapsed by an in-place reload, so the
//            banner must not claim there are unsaved edits.
//   deleted  the file is gone: nothing to reload, only close or keep the text.
//
// A clean buffer reaches `changed` through `reloadFileInPlace`'s resolve-time
// re-check, when the user typed during the round trip.
export type DiskState = "fresh" | "changed" | "paused" | "deleted"

// The part of the store's changed-files slice the freshness helpers read.
// Structural, so this module stays free of the store, React and Monaco.
export interface ChangesSliceView {
  phase: string
  staged: readonly ChangedRowView[]
  unstaged: readonly ChangedRowView[]
}

export interface ChangedRowView {
  path: string
  status: string
  additions: number
  deletions: number
  renamed_from?: string
  kind?: "directory" | "nested_repository" | "linked_worktree"
  file_count?: number
  nested_repositories?: number
  linked_worktrees?: number
  fingerprint?: string
  repositories_inside?: string[]
}

// One tab's Monaco buffer and diff cache, keyed by tab id. A preview-replace
// reuses a tab id under a new `path`: check `isBufferStale` before any read.
export interface TabBuffer {
  path: string
  // The path whose content is actually held in `loaded`/`draft`, or null
  // while a fetch for `path` is in flight / has never completed.
  loadedPath: string | null
  // Means exactly one thing: a `fileApi.read` for this `path` is in flight, so
  // only `fileLoadSeedBuffer` sets it and it clears when the read settles either
  // way. A diff-only seed claiming it would make `shouldSkipFileLoad` suppress
  // the file read forever; a seed omitting it makes that guard fire a second,
  // redundant read, since `loadedPath` is null both before and during a fetch.
  loading: boolean
  loaded: string
  draft: string
  binary: boolean
  readOnly: boolean
  diff: FileDiffContents | null
  // The head of git's patch, set instead of `diff` when the file is past the
  // size ceiling. Never both: the endpoint answers one shape or the other.
  diffHead: FileDiffHead | null
  diffLoadedPath: string | null
  diffLoadedSignal: string
  fileError: string | null
  diffError: string | null
  // The path a load last settled with an error for, mirroring `loadedPath`. A
  // settled error means "do not auto-retry": `shouldSkipFileLoad` reads it so
  // the load effect cannot refire the read every render; the pane offers Retry.
  errorPath: string | null
  // The changed-files signal (`changeSignalFor`) captured when this buffer's
  // read was issued, not when it resolved: a resolve-time signal can reflect a
  // change the returned bytes do not contain, marking a stale buffer fresh.
  fileLoadedSignal: string
  // The freshness token for `loaded`, sent back with a save so the server can
  // refuse to clobber another writer's edit.
  stamp: FileStamp
  // What the last metadata check found on disk. See `DiskState`.
  diskState: DiskState
  // The disk fact the banner reports (`diskFactKey`) and the one the user has
  // answered with "keep mine". Remembering the fact rather than a boolean is
  // what lets a later, different change raise the banner again; neither is ever
  // used as the save token, since acknowledging a change is not consent to
  // overwrite it and the guard on the wire must stay armed.
  diskFact: string | null
  acknowledgedDisk: string | null
  // The encoding `loaded` was decoded from (a WHATWG name) and whether the file
  // starts with a byte-order mark: together, what an ordinary save writes back.
  encoding: string
  bom: boolean
  // The encoding was the server's last resort for bytes that are not UTF-8,
  // not something the file said; the editor says so above the text.
  encodingFallback: boolean
  // The encoding the person asked for by name (a reopen, or a save that
  // converted the file), sent with every later read of this buffer so a
  // reload in place does not quietly go back to the automatic rules. Null
  // while nobody has asked.
  encodingRequested: string | null
  // Why this text cannot be saved in `encoding` without changing bytes nobody
  // edited; the buffer is read-only while it is set. See `WorktreeFile`.
  unwritable: UnwritableReason | null
  // The file's line endings are mixed or include a lone CR, so a save writes
  // every one as `eol`, the one style the editor holds.
  mixedEol: boolean
  // Null until a read names it (an older server names none).
  eol: "LF" | "CRLF" | null
  // Read in a code page the person chose, yet the bytes now look like UTF-8.
  looksLikeUtf8: boolean
  // Read-only for where the file is (a symlink, a .git path): no save of any
  // kind, converting included. Apart from `unwritable`.
  pathReadOnly: boolean
  // Counts every time `loaded` was replaced from disk (a load, a reload in
  // place, a reopen), so a save that resolves later can tell whether the
  // buffer it answers is still the one it was sent from.
  generation: number
  // The encoding a reopen is reading the file in, by its label, while that
  // read is out; null otherwise. The editor takes no typing meanwhile: the
  // answer replaces the text, and anything typed would go with it.
  reopening: string | null
}

export type BufferEncodingFacts = Pick<
  TabBuffer,
  | "encoding"
  | "bom"
  | "encodingFallback"
  | "encodingRequested"
  | "unwritable"
  | "mixedEol"
  | "eol"
  | "looksLikeUtf8"
  | "pathReadOnly"
>

// The encoding fields of a fresh read. An older server sends none and only ever
// served UTF-8, so absence means plain UTF-8 that writes back unchanged; with
// no split of its read-only either, a read-only file there is the path kind.
export function encodingFromFile(
  file: WorktreeFile,
  requested: string | null,
): BufferEncodingFacts {
  const unwritable = file.unwritable ?? null
  return {
    encoding: file.encoding ?? "UTF-8",
    bom: file.bom ?? false,
    encodingFallback: file.encoding_fallback ?? false,
    encodingRequested: requested,
    unwritable,
    mixedEol: file.mixed_line_endings ?? false,
    eol: file.eol ?? null,
    looksLikeUtf8: file.looks_like_utf8 ?? false,
    pathReadOnly:
      file.path_read_only ?? ((file.read_only ?? false) && unwritable === null),
  }
}

// Every line break in `text` (CRLF, a lone CR, LF) as `eol`.
export function normalizeEol(text: string, eol: "LF" | "CRLF"): string {
  return text.replace(/\r\n|\r|\n/g, eol === "CRLF" ? "\r\n" : "\n")
}

// The text a read puts in the editor: the file's content with every line break
// in the one style the server says the editor holds and a save writes. The
// editor's model keeps one style per file anyway; holding exactly that text in
// the buffer is what lets the mixed-endings notice, the status bar, the dirty
// check and the bytes a save sends all agree.
export function editorTextOf(file: WorktreeFile): string {
  return file.eol ? normalizeEol(file.content, file.eol) : file.content
}

// One counter for every buffer, so a generation names one reading of one file
// for the life of the page. Per-buffer counters all start at 1, and a preview
// tab that swaps files under one tab id would then match a save of the first
// file against the first reading of the second.
let lastGeneration = 0
export function freshGeneration(): number {
  lastGeneration += 1
  return lastGeneration
}

// Whether a save's answer may be applied to `buffer`: only when nothing has
// replaced its text from disk since the save was sent. Otherwise the answer
// describes bytes the buffer no longer holds, and relabelling it (say, as the
// encoding a conversion wrote) would misdescribe a different decoding.
export function saveLandsOn(
  buffer: { generation?: number },
  sentGeneration: number,
): boolean {
  return (buffer.generation ?? 0) === sentGeneration
}

// What a save of this buffer writes, unless the person picked a conversion.
export function bufferSaveEncoding(
  buffer: Pick<TabBuffer, "encoding" | "bom"> | undefined,
): { encoding: string; bom: boolean } {
  return { encoding: buffer?.encoding ?? "UTF-8", bom: buffer?.bom ?? false }
}

// A disk fact flattened to one comparable string, or "deleted" when no file is
// there. Only ever compared for equality, never parsed.
export function diskFactKey(onDisk: FileStamp | null): string {
  if (onDisk === null) return "deleted"
  return `${onDisk.modified ?? ""}|${onDisk.size ?? ""}`
}

// The neutral seed: no content and no file read in flight. Base for the diff
// path, which spreads a fetched diff on top, and for `fileLoadSeedBuffer`.
export function emptyBuffer(path: string): TabBuffer {
  return {
    path,
    loadedPath: null,
    loading: false,
    loaded: "",
    draft: "",
    binary: false,
    readOnly: false,
    diff: null,
    diffHead: null,
    diffLoadedPath: null,
    diffLoadedSignal: "",
    fileError: null,
    diffError: null,
    errorPath: null,
    fileLoadedSignal: "",
    stamp: { modified: null, size: null },
    diskState: "fresh",
    diskFact: null,
    acknowledgedDisk: null,
    encoding: "UTF-8",
    bom: false,
    encodingFallback: false,
    encodingRequested: null,
    unwritable: null,
    mixedEol: false,
    eol: null,
    looksLikeUtf8: false,
    pathReadOnly: false,
    generation: 0,
    reopening: null,
  }
}

// The seed `loadFileBuffer` installs the instant it issues a `fileApi.read`:
// the neutral buffer plus `loading: true`, the one place that flag is set.
export function fileLoadSeedBuffer(path: string): TabBuffer {
  return { ...emptyBuffer(path), loading: true }
}

// Whether a cached buffer belongs to a path the tab no longer shows. `openFile`
// (lib/editorTabs.ts) reuses a preview tab's id while swapping its path, so a
// stale buffer must be treated as unloaded and re-fetched, never rendered.
export function isBufferStale(
  buffer: { path: string } | undefined,
  currentPath: string,
): boolean {
  return buffer === undefined || buffer.path !== currentPath
}

// Whether the file-load effect should skip firing another `fileApi.read` for
// `currentPath`. A path that settled with an error counts as skipped: without
// that, the effect refires the read on every render while the tab stays active,
// and the only way to try again is the error pane's manual Retry.
export function shouldSkipFileLoad(
  buffer:
    | { path: string; loadedPath: string | null; loading: boolean; errorPath: string | null }
    | undefined,
  currentPath: string,
): boolean {
  if (isBufferStale(buffer, currentPath)) return false
  const b = buffer!
  return b.loadedPath === currentPath || b.loading || b.errorPath === currentPath
}

// Drop every entry whose key is no longer a live tab id; the tab-id-keyed
// caches do not shrink on their own when a tab closes. Returns the same map
// instance when nothing needed pruning, so a caller can skip a no-op setState.
export function pruneByIds<V>(
  map: Map<string, V>,
  liveIds: ReadonlySet<string>,
): Map<string, V> {
  let stale = false
  for (const id of map.keys()) {
    if (!liveIds.has(id)) {
      stale = true
      break
    }
  }
  if (!stale) return map
  const next = new Map<string, V>()
  for (const [id, value] of map) {
    if (liveIds.has(id)) next.set(id, value)
  }
  return next
}

// Same idea as `pruneByIds` but for a plain `Set<string>` (EditorBody's
// `previewOpenTabIds`, which has no per-entry value to carry).
export function pruneSetByIds(
  set: Set<string>,
  liveIds: ReadonlySet<string>,
): Set<string> {
  let stale = false
  for (const id of set) {
    if (!liveIds.has(id)) {
      stale = true
      break
    }
  }
  if (!stale) return set
  const next = new Set<string>()
  for (const id of set) {
    if (liveIds.has(id)) next.add(id)
  }
  return next
}

// --- Disk freshness ---------------------------------------------------------
//
// There is no file watcher, so the changed-files broadcast is the event source.
// A moved signal says only that something happened, so it buys a metadata check
// and never a reload on its own, and the two legitimate movers (the user's own
// save, and the slice's refetch churn) must cost the user nothing.

// The per-file change signal: status plus line counts. Best-effort, since an
// edit keeping identical +/- counts does not move it, so it is one trigger for
// a check and never proof that anything changed.
export function changeSignalFor(
  slice: ChangesSliceView | null,
  path: string | null,
): string {
  if (path === null || slice === null) return ""
  const f =
    slice.unstaged.find((x) => x.path === path) ??
    slice.staged.find((x) => x.path === path)
  if (f) return `${f.status}:${f.additions}:${f.deletions}`
  // A file inside a folded folder has no row of its own: the folder answers
  // for it, and its file count is what moves when something inside changes.
  const folder = [...slice.unstaged, ...slice.staged].find(
    (x) => x.kind === "directory" && path.startsWith(`${x.path}/`),
  )
  return folder
    ? `${folder.status}:folder:${folder.file_count ?? 0}:${folder.fingerprint ?? ""}`
    : ""
}

// Whether the open file's change signal has moved since this buffer was read.
// The signal's empty string means "git lists nothing for this path" only on a
// loaded slice; off a loading, errored, idle or foreign one it means "not known
// yet", and reading that as absence fires a check on every changes-pane
// refetch, which for a clean buffer moves text under the user's cursor.
export function fileSignalMoved(
  buffer: TabBuffer | undefined,
  currentPath: string,
  slice: ChangesSliceView | null,
): boolean {
  if (slice === null || slice.phase !== "loaded") return false
  if (isBufferStale(buffer, currentPath)) return false
  const b = buffer!
  if (b.loadedPath !== currentPath) return false
  return changeSignalFor(slice, currentPath) !== b.fileLoadedSignal
}

// Whether two freshness tokens describe different bytes. Both halves count:
// mtime alone aliases two writes inside one coarse clock tick, size alone
// misses length-preserving edits, and an unknown against a known is a
// difference, erring toward a wasted re-read rather than a missed one.
export function stampsDiffer(a: FileStamp, b: FileStamp): boolean {
  return a.modified !== b.modified || a.size !== b.size
}

// The part of the info route's answer the freshness check reads. Structural, so
// this module stays free of the info panel's own types.
export interface EntryStampSource {
  modified: string | null
  size: number | null
  target_modified?: string | null
  target_size?: number | null
}

// Which of the info route's two stamps a buffer's read is comparable with. The
// read follows a symlink while the info route stats the link, so comparing
// across a link finds a difference every time and the file reads as stale
// forever. Presence is the test: only a link whose target stat'd carries the
// target fields, so plain files and dangling links use the entry's own stamp.
export function stampFromInfo(info: EntryStampSource): FileStamp {
  const modified = info.target_modified ?? null
  const size = info.target_size ?? null
  if (modified !== null || size !== null) return { modified, size }
  return { modified: info.modified, size: info.size }
}

// Fold freshly-read disk content into an existing buffer without disturbing
// `loadedPath`: re-seeding through the loading path unmounts `CodeEditor`, and
// @monaco-editor/react disposes the model, taking undo history, scroll and
// cursor with it. The text arrives as one full-range edit, so it is undoable
// and moves the cursor, which is why the caller pauses while a selection lives.
//
// `requested` is the encoding the read asked for by name, which the buffer goes
// on asking for; a reload nobody chose an encoding for keeps the buffer's own.
export function reloadedInPlace(
  prev: TabBuffer,
  path: string,
  file: WorktreeFile,
  signal: string,
  requested: string | null = prev.encodingRequested ?? null,
): TabBuffer {
  return {
    ...prev,
    ...encodingFromFile(file, requested),
    path,
    loadedPath: path,
    loading: false,
    loaded: editorTextOf(file),
    generation: freshGeneration(),
    reopening: null,
    draft: editorTextOf(file),
    binary: file.binary,
    readOnly: file.read_only ?? false,
    fileError: null,
    errorPath: null,
    fileLoadedSignal: signal,
    stamp: { modified: file.modified ?? null, size: file.size ?? null },
    diskState: "fresh",
    // The buffer now IS the disk content, so there is no outstanding fact and
    // nothing left to have acknowledged.
    diskFact: null,
    acknowledgedDisk: null,
    // The cached diff describes the content just replaced, so dropping the path
    // makes the diff effect refetch if the tab is (or becomes) a diff tab.
    diffLoadedPath: null,
  }
}

// Re-baseline a buffer on its own successful save: the save moves the
// changed-files signal exactly like an agent's edit, so adopting the server's
// post-write stamp keeps the next broadcast from checking the editor's own work.
//
// `saved` is the encoding the save wrote; a conversion moves the buffer to it
// (see `encodingAfterSave`).
export function baselineSavedBuffer(
  prev: TabBuffer,
  body: string,
  stamp: FileStamp,
  signal: string,
  saved: SaveEncoding = bufferSaveEncoding(prev),
  verdict?: SaveVerdict,
): TabBuffer {
  // The server's own judgement of what it wrote, when it gave one (every
  // save that states an encoding does); otherwise what the editor sent: one
  // line ending style, nothing new to say about the encoding.
  const unwritable = verdict ? (verdict.unwritable ?? null) : null
  // A text that began with U+FEFF, saved with no mark asked for, wrote exactly
  // the bytes of a marked file, and a read now sets that character aside as the
  // mark. The bytes are what the text implied, so nothing is wrong on disk; the
  // copy and the label move to what a read says, and nothing becomes unsaved:
  // the editor's text without the character is that file exactly. Anything
  // typed since the save stays, and so stays unsaved.
  const markRead = verdict?.leading_mark_read_as_bom === true
  const loaded = markRead && body.startsWith("﻿") ? body.slice(1) : body
  const draft = markRead && prev.draft === body ? loaded : prev.draft
  return {
    ...prev,
    ...encodingAfterSave(
      {
        encoding: prev.encoding ?? "UTF-8",
        bom: prev.bom ?? false,
        encodingFallback: prev.encodingFallback ?? false,
        encodingRequested: prev.encodingRequested ?? null,
      },
      saved,
    ),
    ...(verdict?.bom !== undefined ? { bom: verdict.bom } : {}),
    draft,
    // A conversion is never assumed to have made the file writable: a text
    // that still starts with a second mark is two marks in the new encoding
    // too. Read-only for where the file is never changes.
    mixedEol: verdict?.mixed_line_endings ?? false,
    eol: verdict?.eol ?? prev.eol,
    unwritable,
    looksLikeUtf8: false,
    readOnly: (prev.pathReadOnly ?? false) || unwritable !== null,
    loaded,
    fileLoadedSignal: signal,
    stamp,
    diskState: "fresh",
    diskFact: null,
    acknowledgedDisk: null,
    // The saved content is a new working copy, so any cached diff is stale.
    diffLoadedPath: null,
  }
}

// One pending batch of directories `FileTree` must force-refetch, and the
// nonce it keys its revalidation effect on.
export interface TreeRevalidateBatch {
  dirs: string[]
  nonce: number
}

// Fold a new batch of dirs into the pending revalidation batch, deduping, and
// stamp the latest nonce. Callers must apply it through a functional `setState`:
// React batches two same-tick `revalidateDirs` calls, so a plain assignment
// keeps only the last batch and `FileTree` never re-fetches the earlier dirs.
export function unionRevalidateBatch(
  prev: TreeRevalidateBatch | null,
  dirs: string[],
  nonce: number,
): TreeRevalidateBatch {
  return { dirs: [...new Set([...(prev?.dirs ?? []), ...dirs])], nonce }
}
