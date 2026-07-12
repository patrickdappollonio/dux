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
//  1. If a tab already holds `path`: activate it; if opts.pin, clear its preview;
//     if opts.mode is given (an EXPLICIT mode intent, e.g. the changed-files
//     Edit/Diff buttons), retarget the existing tab's mode. A plain activation
//     (tree/search click, opts.mode omitted) PRESERVES whatever mode the tab
//     was already showing: a tree re-click must never silently flip an open
//     diff tab back to file view.
//  2. Else if a NON-DIRTY preview tab exists: REPLACE it in place (reuse its id,
//     swap path+mode; preview stays true unless opts.pin). Never accumulates.
//     The new tab's mode is opts.mode, defaulting to "file".
//  3. Else: append a new tab (preview = !opts.pin, mode = opts.mode ?? "file"),
//     activate it.
// A dirty preview tab is impossible in normal flow (editing pins a tab), but
// rule 2 guards `!preview.dirty` defensively so we never clobber unsaved edits.
export function openFile(
  state: EditorTabsState,
  path: string,
  opts: { mode?: EditorTabMode; pin?: boolean; newId: () => string },
): EditorTabsState {
  const pin = opts.pin ?? false
  const explicitMode = opts.mode

  // Rule 1: already open — activate (and optionally pin), never duplicate.
  // Mode is retargeted only when the caller expressed an explicit intent.
  const existing = state.tabs.find((t) => t.path === path)
  if (existing) {
    const tabs = state.tabs.map((t) => {
      if (t.id !== existing.id) return t
      return {
        ...t,
        mode: explicitMode ?? t.mode,
        preview: pin ? false : t.preview,
      }
    })
    return { tabs, activeId: existing.id }
  }

  const newTabMode = explicitMode ?? "file"

  // Rule 2: a non-dirty preview tab exists — replace it in place.
  const previewTab = state.tabs.find((t) => t.preview && !t.dirty)
  if (previewTab) {
    const tabs = state.tabs.map((t) =>
      t.id === previewTab.id
        ? { ...t, path, mode: newTabMode, preview: !pin, dirty: false }
        : t,
    )
    return { tabs, activeId: previewTab.id }
  }

  // Rule 3: append a new tab.
  const id = opts.newId()
  const tab: EditorTab = { id, path, mode: newTabMode, preview: !pin, dirty: false }
  return { tabs: [...state.tabs, tab], activeId: id }
}

// Promote a tab to permanent (double-click on row/pill, OR first edit).
export function pinTab(state: EditorTabsState, id: string): EditorTabsState {
  return {
    ...state,
    tabs: state.tabs.map((t) => (t.id === id ? { ...t, preview: false } : t)),
  }
}

// Returns the SAME `state` reference when the target tab's dirty flag is
// already `dirty` (including when `id` doesn't match any tab). This matters
// because the store wrapper skips `setState` on a same-reference result (see
// `store.ts` `editorSetTabDirty`), and the overlay currently calls this on
// every keystroke. Without the identity short-circuit, that call would fan
// out a store-wide re-render (useSyncExternalStore has no per-field
// selectors) on every keystroke rather than only on an actual dirty flip.
export function setTabDirty(
  state: EditorTabsState,
  id: string,
  dirty: boolean,
): EditorTabsState {
  const target = state.tabs.find((t) => t.id === id)
  if (!target || target.dirty === dirty) return state
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

// Whether a first edit should promote its tab from preview to permanent, so
// an in-progress edit is never silently discarded by a later preview-replace.
// True only when the edit turns a still-preview tab dirty; false for an
// already-permanent tab (nothing to promote) or an edit that doesn't actually
// change the dirty flag (e.g. undoing back to the saved content, or a
// duplicate call for a tab that's already dirty).
export function shouldPromoteOnEdit(
  tab: EditorTab | undefined,
  newDirty: boolean,
): boolean {
  return tab !== undefined && tab.preview && newDirty
}

// The overlay-close "Discard unsaved changes?" confirmation copy, singular vs.
// plural across however many tabs are dirty. Single source of truth so the
// dialog body and any future caller never drift on the exact wording.
export function dirtyCloseMessage(dirtyCount: number): string {
  return dirtyCount === 1
    ? "You have unsaved changes in 1 tab. They will be lost."
    : `You have unsaved changes in ${dirtyCount} tabs. They will be lost.`
}
