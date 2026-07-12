// Pure helper for `EditorBody`'s per-tab Monaco buffer cache (keyed by tab id).
// Kept free of React/Monaco so the load-bearing invariant is unit-testable
// without mounting Monaco (which cannot mount under vitest — see
// monacoSetup.ts).
//
// Why this exists: `EditorBody` keys its buffers by TAB ID, but `openFile`
// rule 2 (lib/editorTabs.ts) reuses a preview tab's id while swapping its
// `path` out from under it (a preview-replace). Without this check, a
// replaced tab would keep rendering the OLD file's buffer at the new path.
// EditorBody stores the path a buffer was fetched FOR alongside its content,
// and must call this before treating any cached buffer as usable — a stale
// buffer is treated as unloaded and re-fetched, never rendered.
export function isBufferStale(
  buffer: { path: string } | undefined,
  currentPath: string,
): boolean {
  return buffer === undefined || buffer.path !== currentPath
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
