// The editor's per-root draft cache, plus the beforeunload guard covering the
// one loss the cache cannot: the page itself going away. A module Map rather
// than the store, whose unselective external store would re-render every
// consumer on each keystroke, and rather than the deliberately pure
// `editorBuffers.ts`.
//
// Entries are pruned to the live tab set whenever the store's editor-tabs slice
// changes (`setEditorTabsFor`), and a root's entry dies with its target
// (`editorClearRoot`). Drafts live in page memory only.

import type { TabBuffer } from "./editorBuffers"

const cache = new Map<string, Map<string, TabBuffer>>()

// A copy of the root's cached buffers, safe to hand to React state. A buffer
// cached mid-fetch (`loading: true`) is dropped, because its resolver died with
// the component that started it and `shouldSkipFileLoad` would then skip the
// re-read and park the tab on a spinner.
//
// Everything else comes back untouched, including the disk-freshness fields
// (`diskState`, `diskFact`, `acknowledgedDisk`): those are facts about the file
// rather than the component, so a live difference and a "keep mine" answer both
// survive a close and reopen. None of them is a save token, so a restored one
// cannot authorize an overwrite.
export function loadRootDrafts(rootKey: string): Map<string, TabBuffer> {
  const entry = cache.get(rootKey)
  const restored = new Map<string, TabBuffer>()
  if (!entry) return restored
  for (const [tabId, buffer] of entry) {
    if (!buffer.loading) restored.set(tabId, buffer)
  }
  return restored
}

// Snapshot the root's buffers into the cache. Copied rather than referenced, so
// the cache is immune to later state mutations.
export function storeRootDrafts(
  rootKey: string,
  buffers: ReadonlyMap<string, TabBuffer>,
): void {
  cache.set(rootKey, new Map(buffers))
}

// Drop every cached buffer whose tab no longer exists. Wired into the store's
// `setEditorTabsFor`, so a tab closed anywhere takes its draft with it, whether
// or not an `EditorBody` is mounted at the time.
export function pruneRootDrafts(
  rootKey: string,
  liveTabIds: ReadonlySet<string>,
): void {
  const entry = cache.get(rootKey)
  if (!entry) return
  for (const tabId of [...entry.keys()]) {
    if (!liveTabIds.has(tabId)) entry.delete(tabId)
  }
  if (entry.size === 0) cache.delete(rootKey)
}

// Drop a root's whole entry: the path taken when the agent or terminal it was
// rooted at leaves the spine, cleared exactly where `editorTabs` is.
export function clearRootDrafts(rootKey: string): void {
  cache.delete(rootKey)
}

// The guard is armed while any editor tab of any session is dirty in the store,
// which outlives `EditorBody`: a cached draft is real and a refresh would lose
// it, so closing the editor must not disarm the prompt. The one unload that
// must not prompt is the silent server-restart reload, which disarms first.

let armedHandler: ((event: BeforeUnloadEvent) => void) | null = null

function beforeUnloadHandler(event: BeforeUnloadEvent): void {
  // Both channels on purpose: preventDefault is the standard, returnValue the
  // legacy one some browsers still require for the leave prompt to show.
  event.preventDefault()
  event.returnValue = ""
}

// Bring the guard in line with the dirty predicate. Idempotent, so the store
// can call it on every editor-tabs write. A window missing either half of the
// listener API gets no handler: one that could be added but never removed would
// prompt forever.
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

// The restart-reload escape hatch, called by `reloadPage()` immediately before
// reloading: that reload is silent by tenet and must win over the guard, at the
// cost of the drafts.
export function disarmBeforeUnloadGuard(): void {
  syncBeforeUnloadGuard(false)
}
