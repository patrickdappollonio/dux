// Pure helpers for the code-editor tab strip. Components and the store call
// only these functions and never reimplement the selection or promotion rules,
// the same idiom `agentTabs.ts` follows.
//
// Editor tabs are client state, not server-sourced: the store keeps one
// `EditorTabsState` per session, while the heavy Monaco buffer lives in the
// `EditorBody` component, keyed by tab id.

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

// Open a file, applying the VS Code preview model. The one entry point for every
// open gesture:
//  1. A tab already holding `path` is activated, pinned when opts.pin, and
//     retargeted only when opts.mode states an explicit intent, so a tree
//     re-click never flips an open diff tab back to file view.
//  2. Else a non-dirty preview tab is replaced in place, reusing its id, so
//     preview tabs never accumulate.
//  3. Else a new tab is appended and activated.
// Rule 2 guards on `!preview.dirty` so unsaved edits are never clobbered, even
// though editing pins a tab.
export function openFile(
  state: EditorTabsState,
  path: string,
  opts: { mode?: EditorTabMode; pin?: boolean; newId: () => string },
): EditorTabsState {
  const pin = opts.pin ?? false
  const explicitMode = opts.mode

  // Rule 1: already open, activate (and optionally pin), never duplicate.
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

  // Rule 2: a non-dirty preview tab exists, replace it in place.
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

// Returns the same `state` reference when the flag is unchanged or the id
// matches no tab. The store skips `setState` on a same-reference result, and
// the overlay calls this on every keystroke, so without the short-circuit each
// keystroke would fan a store-wide re-render out.
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

// VS Code next-active rule: the tab right of the closing tab's index, else the
// one left of it, else null. `tabs` is the pre-close list; `activeId` is taken
// for signature symmetry with the reducer and unused.
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
// this: a vanished tab id is not dirty by definition.
export function shouldConfirmClose(state: EditorTabsState, id: string): boolean {
  return state.tabs.find((t) => t.id === id)?.dirty ?? false
}

// Whether a first edit should promote its tab from preview to permanent, so an
// in-progress edit is never discarded by a later preview-replace. True only when
// the edit turns a still-preview tab dirty.
export function shouldPromoteOnEdit(
  tab: EditorTab | undefined,
  newDirty: boolean,
): boolean {
  return tab !== undefined && tab.preview && newDirty
}

// True when `tabPath` is exactly `base`, or a descendant of it (`base/...`).
// Shared by the file-management reducers below: a file-scoped op (base ===
// the file's own path) matches by equality; a folder-scoped op (base === the
// folder's path) matches every tab nested under it.
function underPath(tabPath: string, base: string): boolean {
  return tabPath === base || tabPath.startsWith(`${base}/`)
}

// Rename retarget: rewrite the path of the tab at `from`, or of every tab under
// `from/`, replacing the prefix with `to`. Returns the same state reference when
// nothing matched, matching `setTabDirty`'s contract.
//
// Any other tab already sitting at a destination path is closed first, through
// `closeTab`'s next-active rule, or retargeting would leave two tabs on one
// path and break the uniqueness the Monaco model disposal depends on.
//
// Retargeting a clean tab discards its Monaco undo history and view state,
// since the model is keyed by the path's URI. A dirty tab is never renamed: the
// caller gates the Rename dialog on `hasDirtyUnderPath`.
export function renameTabPaths(
  state: EditorTabsState,
  from: string,
  to: string,
): EditorTabsState {
  const matchingFrom = state.tabs.filter((t) => underPath(t.path, from))
  if (matchingFrom.length === 0) return state

  const fromIds = new Set(matchingFrom.map((t) => t.id))
  const newPathFor = (tabPath: string) =>
    tabPath === from ? to : to + tabPath.slice(from.length)
  const newPaths = new Set(matchingFrom.map((t) => newPathFor(t.path)))

  const collidingIds = state.tabs
    .filter((t) => !fromIds.has(t.id) && newPaths.has(t.path))
    .map((t) => t.id)

  let working = state
  for (const id of collidingIds) {
    working = closeTab(working, id)
  }

  return {
    ...working,
    tabs: working.tabs.map((t) =>
      fromIds.has(t.id) ? { ...t, path: newPathFor(t.path) } : t,
    ),
  }
}

// Delete: close the tab at `path` or every tab under `path/`, one at a time
// through `closeTab`, so a cascade reselects as a user closing them one by one
// would. Returns the same reference when nothing matched.
export function closeTabsUnderPath(
  state: EditorTabsState,
  path: string,
): EditorTabsState {
  const matching = state.tabs.filter((t) => underPath(t.path, path))
  if (matching.length === 0) return state
  let working = state
  for (const tab of matching) {
    working = closeTab(working, tab.id)
  }
  return working
}

// True when `path` (a file) or any tab under `path/` (a folder) is DIRTY.
// Gates the Rename dialog so an unsaved buffer is never silently reloaded away
// by the post-rename staleness refetch.
export function hasDirtyUnderPath(
  state: EditorTabsState,
  path: string,
): boolean {
  return state.tabs.some((t) => underPath(t.path, path) && t.dirty)
}

// True when any tab of any session carries a dirty flag: the beforeunload-guard
// predicate. The store flags outlive the editor body, so the guard stays honest
// while the editor is closed with a dirty draft cached (see `editorDrafts.ts`).
export function hasAnyDirtyTab(
  states: Record<string, EditorTabsState>,
): boolean {
  return Object.values(states).some((s) => s.tabs.some((t) => t.dirty))
}

export interface SaveResolution {
  tone: "success" | "warning"
  message: string
}

// What to toast once a save's write resolves. `tabStillOpen` must be read from
// the live tabs list at resolve time, not when `save()` was called: a delete
// confirmed mid-write already reached the server, so the file really was
// recreated on disk and "Saved" would misreport it.
export function saveResolutionOutcome(
  path: string,
  tabStillOpen: boolean,
): SaveResolution {
  if (!tabStillOpen) {
    return {
      tone: "warning",
      message: `${path} was deleted while this save was in flight. The edit was still written back to disk at that path.`,
    }
  }
  return { tone: "success", message: `Saved ${path}` }
}
