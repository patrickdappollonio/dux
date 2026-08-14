import type { FileDiffContents, WorktreeFile } from "./fileApi"

// What the server said about the file when its bytes were handed over: the
// mtime (RFC 3339, from the shared Rust formatter) and the byte size. The two
// travel together and are compared together; see `stampsDiffer`.
export interface FileStamp {
  modified: string | null
  size: number | null
}

// What has happened to the file on disk since this buffer was loaded, as far as
// the editor has been able to establish. Four states, exhaustively:
//
//   fresh    nothing known to have changed (the ordinary case).
//   changed  a metadata check found different bytes on disk AND the buffer has
//            unsaved edits, so it cannot silently reload. The banner is up.
//   paused   the same difference on a CLEAN buffer, held back only because a
//            selection is live in the editor and an in-place reload would
//            collapse it. Nothing is at risk here, so the banner must not say
//            there are unsaved edits: it is a "your reload is waiting for you"
//            notice, and the offer is the reload the editor would have done
//            unasked.
//   deleted  the file is gone. A different rung with a different offer: there
//            is nothing to reload, only a choice to close or keep the text.
//
// A CLEAN buffer reaches `changed` only through the resolve-time re-check in
// `EditorBody.reloadFileInPlace`: it was clean when the reload was decided and
// the user typed during the round trip, so the arriving bytes may no longer be
// applied silently.
export type DiskState = "fresh" | "changed" | "paused" | "deleted"

// The part of the store's changed-files slice the freshness helpers read. A
// structural subset (not an import of `ChangesSlice`) so this module stays free
// of the store, the way the rest of it is free of React and Monaco.
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

// One tab's Monaco buffer + diff cache, keyed by TAB ID in `EditorBody`'s
// `buffers` map. `path` is the path this entry was fetched FOR: every read
// must check it against the tab's CURRENT path via `isBufferStale` before
// trusting `loaded`/`draft`/diff fields, because `openFile` rule 2 (preview-
// replace) reuses a tab's id while swapping its path out from under it. A
// stale entry is treated as unloaded and re-fetched, never rendered.
export interface TabBuffer {
  path: string
  // The path whose content is actually held in `loaded`/`draft`, or null
  // while a fetch for `path` is in flight / has never completed.
  loadedPath: string | null
  // Means exactly one thing: a `fileApi.read` for this buffer's `path` is in
  // flight. ONLY `loadFileBuffer` issues that read, so ONLY a file-load seed
  // (`fileLoadSeedBuffer`) starts `loading: true`; it clears the moment the
  // read settles (success OR error). The neutral `emptyBuffer` seed is
  // `loading: false` -- a buffer the diff path seeds to hold a just-fetched
  // diff (a tab opened straight into diff mode) has no file read in flight, so
  // it must NOT report `loading`, or `shouldSkipFileLoad` reads that phantom
  // flag and never fires the file read when the user switches to code mode,
  // leaving the pane spinning forever.
  //
  // The flag also dedups within the file path itself: `loadFileBuffer` seeds a
  // buffer synchronously (so a stale buffer never renders even for one frame)
  // before its async read resolves, and that synchronous `setState` flips
  // `loadedPath` (undefined -> null), re-triggering the load effect on the next
  // render. Without `loading`, the effect's "already loaded?" check
  // (`loadedPath === path`) sees `null` both before AND during the in-flight
  // fetch and can't tell them apart, firing a second redundant `fileApi.read`.
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
  // The path a load last settled with an ERROR for, or null. Mirrors
  // `loadedPath` (which records a successful load's path) so the load effect
  // can tell "never fetched" apart from "fetched and failed" via
  // `shouldSkipFileLoad`: without this, a failed read left `loadedPath: null`
  // and `loading: false` forever, so the effect refired `fileApi.read` on
  // every render for as long as the tab stayed active (a delete/rename race,
  // or a plain 404, would retry-loop). A settled error is "don't
  // auto-retry"; the error pane offers a manual Retry action instead.
  errorPath: string | null
  // The changed-files signal (`changeSignalFor`) as of the moment the read for
  // this buffer was ISSUED. The mirror of `diffLoadedSignal`, with one
  // difference that matters: it is captured per request for the request's own
  // path, not read off a ref when the response resolves. Resolve-time stamping
  // can record a signal that already reflects a change the returned bytes do
  // NOT contain, which would mark a stale buffer fresh. Capturing early can
  // only err the other way, into a wasted re-check.
  fileLoadedSignal: string
  // The freshness token for `loaded`: what the file looked like when these
  // bytes were read (or when this buffer's own save landed). Sent back with a
  // save so the server can refuse to clobber somebody else's edit.
  stamp: FileStamp
  // What the last metadata check found on disk. See `DiskState`.
  diskState: DiskState
  // The disk fact the banner is currently reporting (`diskFactKey`), and the
  // one the user has already answered with "keep mine".
  //
  // Dismissal has to be remembered or it means nothing: every window focus
  // runs another check, which would find the same difference and put the same
  // banner back. Remembering the FACT rather than a boolean is what keeps a
  // SECOND, later change raising the banner again. Neither field is ever used
  // as the save token: acknowledging a change is not consenting to overwrite
  // it, and the guard on the wire must stay armed.
  diskFact: string | null
  acknowledgedDisk: string | null
}

// A disk fact, flattened to one comparable string: the stamp the check found,
// or the fact that there is no file there at all. Only ever compared for
// equality, never parsed.
export function diskFactKey(onDisk: FileStamp | null): string {
  if (onDisk === null) return "deleted"
  return `${onDisk.modified ?? ""}|${onDisk.size ?? ""}`
}

// The neutral seed: a buffer that holds no content and has NO file read in
// flight (`loading: false`). Used as the base for the diff path (which spreads
// a fetched diff on top) and as the spread base of `fileLoadSeedBuffer`.
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
// the neutral buffer plus `loading: true`, the one place that flag is set. See
// the `loading` field doc for why only the file path may claim it.
export function fileLoadSeedBuffer(path: string): TabBuffer {
  return { ...emptyBuffer(path), loading: true }
}

// Pure helper for `EditorBody`'s per-tab Monaco buffer cache (keyed by tab id).
// Kept free of React/Monaco so the load-bearing invariant is unit-testable
// without mounting Monaco (which cannot mount under vitest, see
// monacoSetup.ts).
//
// Why this exists: `EditorBody` keys its buffers by TAB ID, but `openFile`
// rule 2 (lib/editorTabs.ts) reuses a preview tab's id while swapping its
// `path` out from under it (a preview-replace). Without this check, a
// replaced tab would keep rendering the OLD file's buffer at the new path.
// EditorBody stores the path a buffer was fetched FOR alongside its content,
// and must call this before treating any cached buffer as usable: a stale
// buffer is treated as unloaded and re-fetched, never rendered.
export function isBufferStale(
  buffer: { path: string } | undefined,
  currentPath: string,
): boolean {
  return buffer === undefined || buffer.path !== currentPath
}

// Whether `EditorBody`'s file-load effect should skip firing another
// `fileApi.read` for `currentPath`. Without the `errorPath` check, a failed
// read left `loadedPath: null` and `loading: false`: the effect's "already
// loaded?" guard saw neither "loaded" nor "loading" on every render while the
// tab stayed active, so it fired a fresh read on every render forever
// (reachable via a delete/rename race, or a plain 404). `errorPath` records
// which path a load last settled with an error FOR, the same way `loadedPath`
// records which path last settled successfully; a settled error is treated
// as "don't auto-retry" and surfaces via the existing error pane instead,
// with a manual Retry action as the only way to try again for that path.
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

// Drop every entry whose key is no longer a live tab id. `EditorBody` keys
// several per-tab caches by tab id (the `buffers` Map, the file/diff
// request-token maps, the markdown-preview-open Set) and none of them shrink
// on their own when a tab closes. The Monaco model disposal effect already
// diffs the open PATH set correctly, but these tab-id-keyed structures were
// never wired to the same `[tabs]` effect. Returns the SAME map instance when
// nothing needed pruning, so a caller can skip `setState` on a no-op tick the
// same way the store's `setEditorTabsFor` does for the editor-tabs slice.
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
// dux has no file watcher, deliberately. What it does have is the changed-files
// broadcast the git poller already produces, and that is the event source these
// helpers hang off: when the open file's row in that slice moves, SOMETHING
// happened to the file. What exactly, the slice cannot say, so a moved signal
// buys a metadata check and never a reload on its own. Two entirely legitimate
// movers exist that must not cost the user anything: their own save, and the
// slice's lifecycle churn (a refetch passing through `loading`).

// The per-file change signal: status plus line counts, the same expression the
// diff view's staleness has always used. Best-effort by nature (an edit that
// keeps identical +/- counts does not move it), which is why it is only ever
// one of several triggers for a check, never the proof of anything.
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
//
// The subtlety, and the reason this is a function rather than an inline `!==`:
// the signal's empty string is ambiguous. It means "git lists nothing for this
// path", which is a real fact only once the slice has actually LOADED and
// belongs to this session. Read off a loading, errored, idle or foreign slice
// it means "we do not know yet", and treating that as absence fires a check on
// every changes-pane refetch (and, for a clean buffer, a reload). The diff
// view's equivalent could afford to be sloppy about this because all it did was
// light a button; this one can move text under the user's cursor.
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

// Whether two freshness tokens describe different bytes.
//
// Both halves are compared. An mtime alone aliases two writes that land inside
// one coarse clock tick, which is the racing-writer case worth catching; a size
// alone misses every length-preserving edit. An unknown mtime on one side and a
// known one on the other is a difference, not a match: the safe direction is a
// wasted re-read, never a missed one.
export function stampsDiffer(a: FileStamp, b: FileStamp): boolean {
  return a.modified !== b.modified || a.size !== b.size
}

// Fold freshly-read disk content into an EXISTING buffer without disturbing
// `loadedPath`.
//
// That is the whole point, and it is load-bearing rather than tidy: the pane
// renders `CodeEditor` only while the buffer reports loaded content for the
// tab's path, so re-seeding through the loading path would unmount it, and
// @monaco-editor/react DISPOSES the model on unmount. Undo history, scroll
// position and cursor all live in that model. Keeping `loadedPath` keeps the
// component mounted, so the new text arrives as a `value` prop change, which
// the wrapper applies to the retained model.
//
// Two consequences of that route are accepted and stated rather than hidden:
// the push lands as one full-range edit, so it is UNDOABLE (a ctrl-z after an
// auto-reload steps back to the previous content, not into the agent's), and it
// moves the cursor. For the same reason the caller does not apply it at all
// while a selection is active: it raises the `paused` banner instead and lets
// the user ask for the reload when they are ready.
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
    // The cached diff describes the content that was just replaced, so it is
    // no longer an answer about this file; dropping the path makes the diff
    // effect refetch if the tab is (or becomes) a diff tab.
    diffLoadedPath: null,
  }
}

// Re-baseline a buffer on its own successful save.
//
// The user's save moves the changed-files signal exactly like an agent's edit
// would, so without adopting the server's post-write stamp here, the very next
// broadcast would send the editor checking its own work, and (worse, before the
// check existed) reloading over it.
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

// Fold a new batch of dirs into whatever revalidation batch is already
// pending, deduping, and stamp the latest nonce. `EditorBody.revalidateDirs`
// must call this via a FUNCTIONAL `setState` update (not a plain assignment):
// a plain `setTreeRevalidate({ dirs, nonce })` silently drops a same-tick
// prior batch when two mutations (e.g. a rename touching both its source and
// destination parent dirs, or a rapid create followed by a rename) each call
// `revalidateDirs` before React flushes a render in between. React batches
// the two `setState` calls, so only the LAST plain assignment survives and the
// first batch's dirs are lost, meaning `FileTree` never re-fetches them. A
// functional updater's callback runs in call order even within one batch, so
// unioning here (rather than overwriting) preserves every pending dir.
export function unionRevalidateBatch(
  prev: TreeRevalidateBatch | null,
  dirs: string[],
  nonce: number,
): TreeRevalidateBatch {
  return { dirs: [...new Set([...(prev?.dirs ?? []), ...dirs])], nonce }
}
