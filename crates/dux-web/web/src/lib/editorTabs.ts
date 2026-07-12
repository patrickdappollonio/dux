// Pure helpers for the code-editor tab strip, kept out of components so they
// are unit-testable without mounting React or Monaco. Mirrors the `agentTabs.ts`
// idiom: components and the store call ONLY these functions, never reimplement
// the selection/promotion rules inline.
//
// Editor tabs are pure client state (not server-sourced like agent tabs): the
// store keeps one `EditorTabsState` per session, keyed by session id. The heavy
// Monaco buffer (loaded/draft/view-state/diff cache) lives in the `EditorBody`
// component, keyed by tab id — see `EditorOverlay.tsx`.

export type EditorTabMode = "file" | "diff" // reuses the EditorViewMode shape

export interface EditorTab {
  id: string // client uid (injected generator; not a server id)
  path: string // worktree-relative
  mode: EditorTabMode
  preview: boolean // true = italic preview tab (reusable/replaceable)
  dirty: boolean // mirrored up from the buffer, for the strip dot + close gating
}

export interface EditorTabsState {
  tabs: EditorTab[]
  activeId: string | null
}

export function emptyTabsState(): EditorTabsState {
  return { tabs: [], activeId: null }
}

// Open a file, applying the VS Code preview model. Single source of truth for
// every open entry point (tree single-click, tree double-click, search, new
// file, changed-files Edit/Diff):
//  1. If a tab already holds `path`: activate it; if opts.pin, clear its preview.
//  2. Else if a NON-DIRTY preview tab exists: REPLACE it in place (reuse its id,
//     swap path+mode; preview stays true unless opts.pin). Never accumulates.
//  3. Else: append a new tab (preview = !opts.pin), activate it.
// A dirty preview tab is impossible in normal flow (editing pins a tab), but
// rule 2 guards `!preview.dirty` defensively so we never clobber unsaved edits.
export function openFile(
  state: EditorTabsState,
  path: string,
  mode: EditorTabMode,
  opts: { pin?: boolean; newId: () => string },
): EditorTabsState {
  const pin = opts.pin ?? false

  // Rule 1: already open — activate (and optionally pin), never duplicate.
  const existing = state.tabs.find((t) => t.path === path)
  if (existing) {
    const tabs = pin
      ? state.tabs.map((t) =>
          t.id === existing.id ? { ...t, preview: false, mode } : t,
        )
      : state.tabs.map((t) => (t.id === existing.id ? { ...t, mode } : t))
    return { tabs, activeId: existing.id }
  }

  // Rule 2: a non-dirty preview tab exists — replace it in place.
  const previewTab = state.tabs.find((t) => t.preview && !t.dirty)
  if (previewTab) {
    const tabs = state.tabs.map((t) =>
      t.id === previewTab.id
        ? { ...t, path, mode, preview: !pin, dirty: false }
        : t,
    )
    return { tabs, activeId: previewTab.id }
  }

  // Rule 3: append a new tab.
  const id = opts.newId()
  const tab: EditorTab = { id, path, mode, preview: !pin, dirty: false }
  return { tabs: [...state.tabs, tab], activeId: id }
}

// Promote a tab to permanent (double-click on row/pill, OR first edit).
export function pinTab(state: EditorTabsState, id: string): EditorTabsState {
  return {
    ...state,
    tabs: state.tabs.map((t) => (t.id === id ? { ...t, preview: false } : t)),
  }
}

export function setTabDirty(
  state: EditorTabsState,
  id: string,
  dirty: boolean,
): EditorTabsState {
  return {
    ...state,
    tabs: state.tabs.map((t) => (t.id === id ? { ...t, dirty } : t)),
  }
}

export function setTabMode(
  state: EditorTabsState,
  id: string,
  mode: EditorTabMode,
): EditorTabsState {
  return {
    ...state,
    tabs: state.tabs.map((t) => (t.id === id ? { ...t, mode } : t)),
  }
}

export function activateTab(
  state: EditorTabsState,
  id: string,
): EditorTabsState {
  return { ...state, activeId: id }
}

// Close a tab; if it was active, pick the next active via the VS Code rule.
export function closeTab(state: EditorTabsState, id: string): EditorTabsState {
  const wasActive = state.activeId === id
  const nextId = wasActive ? nextActiveId(state.tabs, id, state.activeId) : state.activeId
  const tabs = state.tabs.filter((t) => t.id !== id)
  return { tabs, activeId: tabs.length === 0 ? null : nextId }
}

// VS Code next-active rule: the tab to the RIGHT of the closing tab's index,
// else the tab to the LEFT, else null (no tabs left). `tabs` is the PRE-close
// list. `activeId` is accepted for signature symmetry with the reducer but the
// rule only depends on the closing tab's position within `tabs`.
export function nextActiveId(
  tabs: EditorTab[],
  closingId: string,
  activeId: string | null,
): string | null {
  void activeId
  const idx = tabs.findIndex((t) => t.id === closingId)
  if (idx === -1) return null
  if (idx + 1 < tabs.length) return tabs[idx + 1].id
  if (idx - 1 >= 0) return tabs[idx - 1].id
  return null
}

// Pure dirty-gating check for the close flow: components never re-implement
// this — a vanished tab id is not dirty by definition.
export function shouldConfirmClose(state: EditorTabsState, id: string): boolean {
  return state.tabs.find((t) => t.id === id)?.dirty ?? false
}
