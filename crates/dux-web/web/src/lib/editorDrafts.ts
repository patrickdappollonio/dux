// The editor's per-session draft cache, plus the beforeunload guard that
// covers the one loss the cache cannot: the page itself going away.
//
// Why a NEW module with mutable state, rather than either of the obvious
// homes. Not the store: file contents in `useDux()`'s unselective external
// store would fan a re-render out to every consumer on each keystroke, which
// is the exact design `EditorBody` keeps its buffers in component state to
// avoid (see lib/editorTabs.ts header comment). Not `editorBuffers.ts`: that
// module is deliberately PURE (stateless helpers with their own unit tests),
// and module-level mutable state would change its character and leak state
// across its tests. So the cache lives here: a plain module Map that
// `EditorBody` seeds its buffer state from on mount and writes back on every
// change, which is what lets an unsaved draft survive the editor being closed
// and reopened.
//
// Lifecycle: entries are pruned to the live tab set whenever the store's
// editor-tabs slice changes (`setEditorTabsFor`), so a closed tab's draft
// (including a confirmed per-tab discard) leaves the cache with the tab, and
// a whole session's entry dies with the session (`editorClearSession`).
// Drafts live in page memory only: a reload or a dux restart loses them,
// which is exactly what the beforeunload guard below exists to warn about.

import type { TabBuffer } from "./editorBuffers"

const cache = new Map<string, Map<string, TabBuffer>>()

// A COPY of the session's cached buffers, safe to hand to React state. A
// buffer cached mid-fetch (`loading: true`) is dropped: its resolver died
// with the component that started it, and restoring it would make
// `shouldSkipFileLoad` skip the re-read and park the tab on a spinner
// forever. The remount re-fetches that path from scratch instead.
//
// Everything else comes back untouched, INCLUDING the disk-freshness fields
// (`diskState`, `diskFact`, `acknowledgedDisk`), and that is a decision rather
// than an oversight. Those fields are facts about the file, not about the
// component: if the file on disk had moved away from this buffer before the
// editor was closed, it has still moved away when it reopens, and dropping the
// banner would hide a live difference at exactly the moment the user comes
// back to look at the text. The same goes for a dismissal: "keep mine" was an
// answer about a specific change, and re-asking it because a panel was closed
// and reopened would make the button mean nothing. Neither field is ever a
// save token, so a restored one cannot authorize an overwrite, and the mount
// trigger in `EditorBody` re-checks every restored buffer, so a stale banner
// is retired by the next check rather than living forever.
export function loadSessionDrafts(sessionId: string): Map<string, TabBuffer> {
  const entry = cache.get(sessionId)
  const restored = new Map<string, TabBuffer>()
  if (!entry) return restored
  for (const [tabId, buffer] of entry) {
    if (!buffer.loading) restored.set(tabId, buffer)
  }
  return restored
}

// Snapshot the session's buffers into the cache. Called by `EditorBody` on
// every buffer change; copying is cheap (the map is small and the buffers are
// immutable values) and keeps the cache immune to later state mutations.
export function storeSessionDrafts(
  sessionId: string,
  buffers: ReadonlyMap<string, TabBuffer>,
): void {
  cache.set(sessionId, new Map(buffers))
}

// Drop every cached buffer whose tab no longer exists. Wired into the store's
// `setEditorTabsFor`, so a tab closed ANYWHERE (per-tab discard confirmed, a
// deleted file closing its tabs, a rename collision) takes its draft with it,
// whether or not an `EditorBody` is mounted at the time.
export function pruneSessionDrafts(
  sessionId: string,
  liveTabIds: ReadonlySet<string>,
): void {
  const entry = cache.get(sessionId)
  if (!entry) return
  for (const tabId of [...entry.keys()]) {
    if (!liveTabIds.has(tabId)) entry.delete(tabId)
  }
  if (entry.size === 0) cache.delete(sessionId)
}

// Drop a session's whole entry: the session-delete path, cleared exactly
// where `editorTabs` is (`editorClearSession`).
export function clearSessionDrafts(sessionId: string): void {
  cache.delete(sessionId)
}

// --- The beforeunload guard ------------------------------------------------
//
// Closing the EDITOR is non-destructive now (the cache above), so the only
// real losses left are the page-level ones: a hard refresh, closing the
// browser tab, closing the window. The guard is armed while any editor tab of
// any session is dirty IN THE STORE (the store flag outlives `EditorBody`, so
// the guard deliberately stays armed while the editor is closed with a dirty
// draft cached: that draft is real, a refresh really would lose it, and the
// way to stop the prompt is to deal with the draft, not to close the editor).
// The one page unload that must NOT prompt is the server-restart reload,
// which is silent by tenet: `reloadPage()` disarms the guard first.

let armedHandler: ((event: BeforeUnloadEvent) => void) | null = null

function beforeUnloadHandler(event: BeforeUnloadEvent): void {
  // Both channels on purpose: preventDefault is the standard, returnValue the
  // legacy one some browsers still require for the leave prompt to show.
  event.preventDefault()
  event.returnValue = ""
}

// Bring the guard in line with the dirty predicate. Idempotent, so the store
// can call it on every editor-tabs write. A window missing EITHER half of the
// listener API gets no handler at all: a handler that could be added but
// never removed would prompt forever.
export function syncBeforeUnloadGuard(anyDirty: boolean): void {
  if (
    typeof window === "undefined" ||
    typeof window.addEventListener !== "function" ||
    typeof window.removeEventListener !== "function"
  ) {
    return
  }
  if (anyDirty && armedHandler === null) {
    armedHandler = beforeUnloadHandler
    window.addEventListener("beforeunload", armedHandler)
  } else if (!anyDirty && armedHandler !== null) {
    window.removeEventListener("beforeunload", armedHandler)
    armedHandler = null
  }
}

// The restart-reload escape hatch: `reloadPage()` calls this immediately
// before `window.location.reload()`, because that reload is silent by tenet
// (no prompt, no toast, no banner) and must win over the guard. The drafts
// are lost in that case, and the plan says so plainly.
export function disarmBeforeUnloadGuard(): void {
  syncBeforeUnloadGuard(false)
}
