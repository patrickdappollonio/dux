import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Exercises the editor-tabs store slice end to end: `editorOpenFile` seeding
// and preview-replacing, `openEditor`'s tab-seeding extension, pin/dirty
// mutation, close + neighbor-activation, and session-scoped clearing. The pure
// reducer rules themselves are covered by `editorTabs.test.ts`; this file only
// checks the store's thin wrapping (keys by session id, wires `openEditor`,
// exposes the close-confirm target).

function makeBootstrap(): Bootstrap {
  return {
    available_providers: ["claude", "codex", "opencode"],
    macros: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    always_show_tab_strip: false,
    global_env: {},
    status_clear_seconds: 6,
    agent_tabs_max: 20,
  }
}

function makeSpine(sessionIds: string[] = ["s1"]) {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: sessionIds.map((id) => ({
      id,
      project_id: "p1",
      terminals: [],
      tabs: [{ id }],
    })),
    sidebar: { groups: [] },
  }
}

let spineBody: unknown = makeSpine()

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => makeBootstrap(),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => JSON.stringify(spineBody),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => JSON.stringify({ rev: 1, staged: [], unstaged: [] }),
      headers: { get: () => null },
    } as unknown as Response
  }
  throw new Error(`unexpected fetch: ${u}`)
})

let sockets: FakeWebSocket[] = []

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  binaryType = ""
  readyState = 1
  constructor() {
    sockets.push(this)
  }
  close() {}
  send() {}
}

// Deliver a `sessions.changed` events-socket frame, which makes the store
// re-fetch the current `spineBody` and re-run `applyWorkspace` (mirrors
// storeTabs.test.ts's helper of the same name).
function fireSessionsChanged() {
  sockets.at(-1)?.onmessage?.({
    data: JSON.stringify({ event: "sessions.changed" }),
  })
}

const tick = () => new Promise((r) => setTimeout(r, 0))

beforeEach(() => {
  sockets = []
  spineBody = makeSpine()
  vi.stubGlobal("location", { host: "localhost:0", protocol: "http:" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    go: () => {},
    pushState: () => {},
    replaceState: () => {},
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

describe("editor tabs store slice", () => {
  it("editorOpenFile seeds a preview tab and sets it active", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const tabs = mod.getSnapshot().editorTabs.s1
    expect(tabs.tabs).toHaveLength(1)
    expect(tabs.tabs[0]).toMatchObject({ path: "a.ts", preview: true, dirty: false })
    expect(tabs.activeId).toBe(tabs.tabs[0].id)
  })

  it("editorOpenFile a second path replaces the preview tab (no accumulation)", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const firstId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorOpenFile("s1", "b.ts")
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0].id).toBe(firstId)
    expect(tabs[0].path).toBe("b.ts")
  })

  it("openEditor with initialPath opens the overlay AND seeds a matching tab", async () => {
    const mod = await loadStore()
    mod.openEditor("s1", "a.ts", "diff")
    expect(mod.getSnapshot().editorTarget).toEqual({
      sessionId: "s1",
      initialPath: "a.ts",
      initialMode: "diff",
    })
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toMatchObject({ path: "a.ts", mode: "diff" })
  })

  it("editorOpenFile without an explicit mode preserves an existing tab's mode (tree/search re-click)", async () => {
    const mod = await loadStore()
    mod.openEditor("s1", "a.ts", "diff")
    // A tree/search click on the already-open path carries no mode intent.
    mod.editorOpenFile("s1", "a.ts")
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0].mode).toBe("diff")
  })

  it("an image path never opens in diff mode: openEditor coerces to file", async () => {
    // A changed image clicked in the Changes pane asks for "diff"; there is
    // no text to diff, so the choke point coerces and the overlay shows the
    // picture instead of dead-ending on the binary-diff refusal.
    const mod = await loadStore()
    mod.openEditor("s1", "assets/logo.png", "diff")
    expect(mod.getSnapshot().editorTarget).toEqual({
      sessionId: "s1",
      initialPath: "assets/logo.png",
      initialMode: "file",
    })
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toMatchObject({ path: "assets/logo.png", mode: "file" })
  })

  it("editorOpenFile coerces an explicit diff intent to file for an image path", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "logo.png", { mode: "diff" })
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].mode).toBe("file")
    // And an already-open image tab cannot be retargeted into diff either.
    mod.editorOpenFile("s1", "logo.png", { mode: "diff" })
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].mode).toBe("file")
  })

  it("an svg path still honors diff mode (it is a text tab, not an image tab)", async () => {
    const mod = await loadStore()
    mod.openEditor("s1", "icons/logo.svg", "diff")
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].mode).toBe("diff")
  })

  it("editorOpenFile with an explicit mode retargets an existing tab (changed-files Diff button)", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts", { mode: "file", pin: true })
    mod.editorOpenFile("s1", "a.ts", { mode: "diff" })
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0].mode).toBe("diff")
  })

  it("editorPinTab clears preview", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].preview).toBe(true)
    mod.editorPinTab("s1", id)
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].preview).toBe(false)
  })

  it("editorSetTabDirty flips dirty and shouldConfirmClose-equivalent gating reflects it", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorSetTabDirty("s1", id, true)
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].dirty).toBe(true)
    mod.editorSetTabDirty("s1", id, false)
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].dirty).toBe(false)
  })

  it("editorSetTabDirty with an unchanged value is a no-op and keeps the session's tabs state reference identical (no store-wide re-render)", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    const before = mod.getSnapshot().editorTabs.s1
    // The freshly-opened tab already starts at dirty: false, so setting it to
    // false again must not replace the session's tabs-state object.
    mod.editorSetTabDirty("s1", id, false)
    expect(mod.getSnapshot().editorTabs.s1).toBe(before)
  })

  it("editorCloseTab activates the neighbor and removes the tab", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    mod.editorOpenFile("s1", "b.ts", { pin: true })
    const [a, b] = mod.getSnapshot().editorTabs.s1.tabs
    mod.editorActivateTab("s1", a.id)
    mod.editorCloseTab("s1", a.id)
    const tabs = mod.getSnapshot().editorTabs.s1
    expect(tabs.tabs.map((t) => t.id)).toEqual([b.id])
    expect(tabs.activeId).toBe(b.id)
  })

  it("editorClearSession empties that session's tabs only", async () => {
    spineBody = makeSpine(["s1", "s2"])
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    mod.editorOpenFile("s2", "b.ts")
    mod.editorClearSession("s1")
    expect(mod.getSnapshot().editorTabs.s1).toBeUndefined()
    expect(mod.getSnapshot().editorTabs.s2.tabs).toHaveLength(1)
  })

  it("editorCloseTab on the last tab leaves an empty, overlay-open state", async () => {
    const mod = await loadStore()
    mod.openEditor("s1", "a.ts")
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorCloseTab("s1", id)
    const tabs = mod.getSnapshot().editorTabs.s1
    expect(tabs.tabs).toEqual([])
    expect(tabs.activeId).toBeNull()
    // The overlay itself stays open, only the tab list emptied.
    expect(mod.getSnapshot().editorTarget).not.toBeNull()
  })

  it("a preview-replace via editorOpenFile always lands the reused tab at dirty: false", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorOpenFile("s1", "b.ts")
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toMatchObject({ id, path: "b.ts", dirty: false })
  })

  it("openEditorCloseTab / closeEditorCloseTab drive the close-confirm target", async () => {
    const mod = await loadStore()
    mod.openEditorCloseTab("s1", "t1")
    expect(mod.getSnapshot().editorCloseTabTarget).toEqual({
      sessionId: "s1",
      tabId: "t1",
    })
    mod.closeEditorCloseTab()
    expect(mod.getSnapshot().editorCloseTabTarget).toBeNull()
  })

  // Finding 8: `editorRenameTabPaths`/`editorCloseTabsUnderPath` had no direct
  // store-level coverage; the pure reducers (`renameTabPaths`/
  // `closeTabsUnderPath`) are covered by editorTabs.test.ts, but the store's
  // thin wrapping (keying by session id, and the ref-equal no-op short-
  // circuit `setEditorTabsFor` relies on to skip a store-wide re-render) was
  // untested at this layer.
  it("editorRenameTabPaths retargets the matching tab's path via editorTabsFor snapshot", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "src/a.ts", { pin: true })
    const id = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorRenameTabPaths("s1", "src/a.ts", "src/renamed.ts")
    const tabs = mod.getSnapshot().editorTabs.s1.tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0]).toMatchObject({ id, path: "src/renamed.ts" })
  })

  it("editorRenameTabPaths retargets every tab under a renamed folder", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "src/a.ts", { pin: true })
    mod.editorOpenFile("s1", "src/nested/b.ts", { pin: true })
    mod.editorRenameTabPaths("s1", "src", "lib")
    const paths = mod.getSnapshot().editorTabs.s1.tabs.map((t) => t.path).sort()
    expect(paths).toEqual(["lib/a.ts", "lib/nested/b.ts"])
  })

  it("editorRenameTabPaths is a ref-equal no-op on a session with no matching tab (no store-wide re-render)", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const before = mod.getSnapshot().editorTabs.s1
    mod.editorRenameTabPaths("s1", "unrelated.ts", "still-unrelated.ts")
    expect(mod.getSnapshot().editorTabs.s1).toBe(before)
  })

  it("editorCloseTabsUnderPath closes the tab at an exact deleted file path and reselects", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    mod.editorOpenFile("s1", "b.ts", { pin: true })
    const [, b] = mod.getSnapshot().editorTabs.s1.tabs
    mod.editorCloseTabsUnderPath("s1", "a.ts")
    const tabs = mod.getSnapshot().editorTabs.s1
    expect(tabs.tabs.map((t) => t.id)).toEqual([b.id])
    expect(tabs.activeId).toBe(b.id)
  })

  it("editorCloseTabsUnderPath closes every tab under a deleted folder", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "src/a.ts", { pin: true })
    mod.editorOpenFile("s1", "src/nested/b.ts", { pin: true })
    mod.editorOpenFile("s1", "keep.ts", { pin: true })
    mod.editorCloseTabsUnderPath("s1", "src")
    const paths = mod.getSnapshot().editorTabs.s1.tabs.map((t) => t.path)
    expect(paths).toEqual(["keep.ts"])
  })

  it("editorCloseTabsUnderPath is a ref-equal no-op when nothing matches (no store-wide re-render)", async () => {
    const mod = await loadStore()
    mod.editorOpenFile("s1", "a.ts")
    const before = mod.getSnapshot().editorTabs.s1
    mod.editorCloseTabsUnderPath("s1", "unrelated.ts")
    expect(mod.getSnapshot().editorTabs.s1).toBe(before)
  })

  it("clears a session's editor tabs and closes a targeted editor overlay when the session vanishes from the spine", async () => {
    spineBody = makeSpine(["s1", "s2"])
    const mod = await loadStore()
    mod.openEditor("s1", "a.ts")
    expect(mod.getSnapshot().editorTabs.s1.tabs).toHaveLength(1)

    // Session s1 disappears from a later spine (deleted by this or another client).
    spineBody = makeSpine(["s2"])
    fireSessionsChanged()
    await tick()

    expect(mod.getSnapshot().editorTabs.s1).toBeUndefined()
    expect(mod.getSnapshot().editorTarget).toBeNull()
  })
})

// (f) drafts survive editor close: the store is what prunes the draft cache
// (a tab that no longer exists must take its draft with it, whether or not an
// EditorBody is mounted) and what arms/disarms the beforeunload guard off the
// STORE dirty flags (which outlive the component).
describe("draft cache and unload guard wiring", () => {
  async function loadWithGuardWindow() {
    // The suite-level window stub has only addEventListener, which the guard
    // deliberately refuses (it will not add a handler it cannot remove).
    // These tests need the full pair, captured.
    const added: string[] = []
    const removed: string[] = []
    vi.stubGlobal("window", {
      addEventListener: (type: string) => added.push(type),
      removeEventListener: (type: string) => removed.push(type),
    })
    const mod = await loadStore()
    const drafts = await import("./editorDrafts")
    return { mod, drafts, added, removed }
  }

  function cachedBuffer(path: string) {
    return {
      path,
      loadedPath: path,
      loading: false,
      loaded: "on disk",
      draft: "typed and unsaved",
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

  it("a closed tab takes its cached draft with it", async () => {
    const { mod, drafts } = await loadWithGuardWindow()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    const tabId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    drafts.storeSessionDrafts("s1", new Map([[tabId, cachedBuffer("a.ts")]]))

    mod.editorCloseTab("s1", tabId)
    expect(drafts.loadSessionDrafts("s1").size).toBe(0)
  })

  it("clearing a session drops its whole draft cache entry", async () => {
    const { mod, drafts } = await loadWithGuardWindow()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    const tabId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    drafts.storeSessionDrafts("s1", new Map([[tabId, cachedBuffer("a.ts")]]))

    mod.editorClearSession("s1")
    expect(drafts.loadSessionDrafts("s1").size).toBe(0)
  })

  it("arms the beforeunload guard while any tab is dirty and disarms on discard", async () => {
    const { mod, added, removed } = await loadWithGuardWindow()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    const tabId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    expect(added).not.toContain("beforeunload")

    mod.editorSetTabDirty("s1", tabId, true)
    expect(added).toContain("beforeunload")
    expect(removed).not.toContain("beforeunload")

    // The guard stays armed while the editor is CLOSED with the dirty flag
    // still set: the draft is real and a refresh really would lose it.
    mod.closeEditor()
    expect(removed).not.toContain("beforeunload")

    // The per-tab discard clears the flag with the tab, and the guard drops.
    mod.editorCloseTab("s1", tabId)
    expect(removed).toContain("beforeunload")
  })

  it("disarms when the dirty session vanishes from the spine", async () => {
    const { mod, added, removed } = await loadWithGuardWindow()
    spineBody = makeSpine(["s1", "s2"])
    fireSessionsChanged()
    await tick()
    mod.editorOpenFile("s1", "a.ts", { pin: true })
    const tabId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    mod.editorSetTabDirty("s1", tabId, true)
    expect(added).toContain("beforeunload")

    spineBody = makeSpine(["s2"])
    fireSessionsChanged()
    await tick()
    expect(removed).toContain("beforeunload")
  })
})
