import type { FileDiffContents, WorktreeFile } from "./fileApi"

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
  return f ? `${f.status}:${f.additions}:${f.deletions}` : ""
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
export function reloadedInPlace(
  prev: TabBuffer,
  path: string,
  file: WorktreeFile,
  signal: string,
): TabBuffer {
  return {
    ...prev,
    path,
    loadedPath: path,
    loading: false,
    loaded: file.content,
    draft: file.content,
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
export function baselineSavedBuffer(
  prev: TabBuffer,
  body: string,
  stamp: FileStamp,
  signal: string,
): TabBuffer {
  return {
    ...prev,
    loaded: body,
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
