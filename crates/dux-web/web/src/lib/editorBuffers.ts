import type { FileDiffContents } from "./fileApi"

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
