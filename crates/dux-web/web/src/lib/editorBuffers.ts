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
