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
