import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./workspaceApi"

// Theater mode in the store: the live flag, the address modifier, the
// per-pane memory, and the ownership-loss reset. The harness is the deep-link
// one (a fake history whose writers mirror the URL back into a fake location),
// plus a REAL Map-backed localStorage, because the memory is half the feature.

function makeSpine(
  sessions: {
    id: string
    project_id: string
    terminals?: string[]
    tabs?: string[]
  }[],
  projects: { id: string; terminals?: string[] }[] = [],
): Spine {
  return {
    projects: projects.map((p) => ({ id: p.id, name: p.id })) as unknown as Spine["projects"],
    sessions: sessions.map((s) => ({
      id: s.id,
      workspace: {
        kind: "managed",
        project_id: s.project_id,
        branch_name: "",
        initial_branch: "",
        branch_provenance: "created",
        source_branch: "",
        worktree_path: "",
      },
      tabs: [{ id: s.id }, ...(s.tabs ?? []).map((id) => ({ id }))],
    })) as unknown as Spine["sessions"],
    terminals: [
      ...sessions.flatMap((s) =>
        (s.terminals ?? []).map((id) => ({
          id,
          owner: { kind: "session", session_id: s.id },
        })),
      ),
      ...projects.flatMap((p) =>
        (p.terminals ?? []).map((id) => ({
          id,
          owner: { kind: "project", project_id: p.id },
        })),
      ),
    ] as unknown as Spine["terminals"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])
let replaceStateMock: ReturnType<typeof vi.fn>
let pushStateMock: ReturnType<typeof vi.fn>
let store: Map<string, string>
let loc: {
  protocol: string
  host: string
  hash: string
  pathname: string
  search: string
}

// Set to hold the first spine read open forever, so a test can look at the
// state the very first render sees rather than the one the workspace produced.
let holdSpine = false

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/workspace")) {
    if (holdSpine) await new Promise(() => {})
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    ok: true,
    status: 200,
    json: async () => ({}),
    text: async () => "{}",
    headers: { get: () => null },
  } as unknown as Response
})

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 1
  close() {}
  send() {}
}

beforeEach(() => {
  holdSpine = false
  spineBody = makeSpine([])
  store = new Map<string, string>()
  replaceStateMock = vi.fn((_s: unknown, _t: string, url: string) => {
    loc.hash = url.startsWith("#") ? url : ""
  })
  pushStateMock = vi.fn((_s: unknown, _t: string, url: string) => {
    loc.hash = url.startsWith("#") ? url : ""
  })
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    pushState: pushStateMock,
    replaceState: replaceStateMock,
    state: null,
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore(
  hash: string,
  sessions: { id: string; project_id: string; terminals?: string[]; tabs?: string[] }[],
  projects: { id: string; terminals?: string[] }[] = [],
) {
  loc = { protocol: "http:", host: "localhost:0", hash, pathname: "/", search: "" }
  vi.stubGlobal("location", loc)
  spineBody = makeSpine(sessions, projects)
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Import the store on a boot hash WITHOUT waiting for a spine: what the first
// render sees is exactly the question the boot-layout tests ask.
async function loadUnresolved(hash: string) {
  loc = { protocol: "http:", host: "localhost:0", hash, pathname: "/", search: "" }
  vi.stubGlobal("location", loc)
  return await import("./store")
}

describe("entering and leaving theater", () => {
  it("starts off, and entering pushes the modified address", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    expect(mod.getSnapshot().theater).toBe(false)
    pushStateMock.mockClear()

    mod.enterTheater()
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1?view=theater")
    // A push, so Back is the way out of a mode the user chose to enter.
    expect(pushStateMock).toHaveBeenCalledTimes(1)
  })

  it("leaving pushes too, so Back is never a dead press", async () => {
    // The house precedent is `closeEditor`: closing a position is a move
    // between two real places, and a replace here left the entry the enter
    // pushed on the stack with nothing to go back to. The accepted cost is
    // that Back after enter-then-exit re-enters theater.
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()
    pushStateMock.mockClear()
    replaceStateMock.mockClear()

    mod.exitTheater()
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1")
    expect(pushStateMock).toHaveBeenCalledTimes(1)
    expect(replaceStateMock).not.toHaveBeenCalled()
  })

  it("toggles both ways from the one action the buttons call", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.toggleTheater()
    expect(mod.getSnapshot().theater).toBe(true)
    mod.toggleTheater()
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("refuses to enter with nothing focused, since there is no pane to fill", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.enterTheater()
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("")
  })

  it("switching tab from the pill carries the mode to a tab that never had it", async () => {
    // The real journey: theater is entered on ONE tab, and the pill's mini
    // strip switches to a sibling that has never been in theater. Reading the
    // destination's memory would drop the mode mid-gesture, so the switch says
    // the mode explicitly and the destination remembers it from then on.
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    pushStateMock.mockClear()
    replaceStateMock.mockClear()

    mod.selectTab("s1", "t2", { theater: true })
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1/tab/t2?view=theater")
    expect(store.get("dux:theater:agent:t2")).toBe("on")
    // A move within a screen, so it replaces like every other tab switch.
    expect(pushStateMock).not.toHaveBeenCalled()
    expect(replaceStateMock).toHaveBeenCalled()
  })

  it("a plain tab switch after leaving theater never resurrects it", async () => {
    // Leaving through the chrome forgets the pane, so an ordinary switch back
    // (no override, because there is no pill on screen to make one) lands on
    // the full layout.
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.selectTab("s1", "t2", { theater: true })
    mod.exitTheater()
    mod.selectSession("s1")
    mod.exitTheater()

    mod.selectTab("s1", "t2")
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1/tab/t2")
    mod.selectSession("s1")
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("puts a terminal pane in theater the same way", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", terminals: ["tm1"] },
    ])
    mod.selectTerminal("tm1", { kind: "session", sessionId: "s1" })
    mod.enterTheater()
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1/terminal/tm1?view=theater")
  })
})

describe("the per-pane memory", () => {
  it("restores the mode when the pane is selected again", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()

    mod.selectTab("s1", "t2")
    expect(mod.getSnapshot().theater).toBe(false)

    mod.selectTab("s1", "s1")
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1?view=theater")
  })

  it("is per tab, so one tab's mode never follows its sibling", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.selectTab("s1", "t2")
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1/tab/t2")
  })

  it("forgets the mode once the user leaves it", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.exitTheater()
    mod.selectTab("s1", "t2")
    mod.selectTab("s1", "s1")
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("writes nothing but its own key", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()
    expect([...store.keys()]).toContain("dux:theater:agent:s1")
  })
})

describe("theater in the address", () => {
  it("opens a shared link in theater", async () => {
    const mod = await loadStore("#/agent/s1?view=theater", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().theater).toBe(true)
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    expect(loc.hash).toBe("#/agent/s1?view=theater")
  })

  it("opens an extra tab's shared theater link", async () => {
    const mod = await loadStore("#/agent/s1/tab/t2?view=theater", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1/tab/t2?view=theater")
  })

  it("boots with the mode already on, before any workspace has landed", async () => {
    // The pane measures itself the instant it mounts. When the flag only
    // arrived with the first spine, the chrome was painted at boot and then
    // collapsed as an ANIMATED transition underneath a pane that was mounting
    // into it: the pane fitted at a geometry it was only passing through, the
    // server's replay was parsed at that grid, and the settled fit re-gridded
    // on top of it, dropping and reordering lines. The address already says
    // where the page is going, so the first layout says it too.
    holdSpine = true
    const mod = await loadUnresolved("#/agent/s1?view=theater")
    expect(mod.getSnapshot().spine).toBeNull()
    expect(mod.getSnapshot().theater).toBe(true)
  })

  it("boots with the mode off on an address that does not carry it", async () => {
    holdSpine = true
    const mod = await loadUnresolved("#/agent/s1")
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("never boots into theater on an address that names no pane", async () => {
    // Home has nothing to fill the screen, and `withTheaterHash` refuses to
    // write the modifier there, so a bare one is a hand-made address that must
    // not blank the chrome while the route resolves to home anyway.
    holdSpine = true
    const mod = await loadUnresolved("#?view=theater")
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("still tells the truth about a missing agent", async () => {
    const mod = await loadStore("#/agent/missing?view=theater", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "missing",
    })
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("round-trips through the route grammar", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    const route = {
      target: { kind: "agent" as const, sessionId: "s1", tabId: "t2" },
      changes: false,
      editor: null,
      standalone: false,
      theater: true,
    }
    expect(mod.parseRoute(mod.routeHash(route))).toEqual(route)
  })

  it("never rides an editor or changes address", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    expect(
      mod.routeHash({
        target: { kind: "agent", sessionId: "s1", tabId: "s1" },
        changes: true,
        editor: null,
        standalone: false,
        theater: true,
      }),
    ).toBe("#/agent/s1/changes")
  })
})

describe("reconnecting an agent", () => {
  it("restores the pane's remembered mode like every other selection", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.selectSession("s2")
    expect(mod.getSnapshot().theater).toBe(false)

    mod.reconnectSession("s1", false)
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1?view=theater")
  })

  it("stays out of theater for a pane that was never in it", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s2")
    mod.enterTheater()
    mod.reconnectSession("s1", false)
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1")
  })
})

describe("the editor and the changes screen", () => {
  it("suspends theater when the editor opens, so state and URL agree", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()

    mod.openEditor({ kind: "agent", sessionId: "s1" })
    // The address has no room for the modifier over an editor, so the live
    // flag has to come off with it. The pane's MEMORY stays on.
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1/editor")
    expect(store.get("dux:theater:agent:s1")).toBe("on")
  })

  it("gives theater back when the editor closes", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.openEditor({ kind: "agent", sessionId: "s1" })

    mod.closeEditor()
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1?view=theater")
  })

  it("leaves the editor alone for a pane that was never in theater", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.openEditor({ kind: "agent", sessionId: "s1" })
    mod.closeEditor()
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1")
  })

  it("suspends and restores across the phone's changes screen too", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()

    mod.openChangesScreen()
    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1/changes")

    mod.navigateUp()
    expect(mod.getSnapshot().theater).toBe(true)
    expect(loc.hash).toBe("#/agent/s1?view=theater")
  })
})

describe("losing input ownership", () => {
  it("leaves theater and forgets the pane's memory", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.enterTheater()
    expect(store.get("dux:theater:agent:s1")).toBe("on")

    mod.noteTheaterOwnershipLost("agent", "s1")

    expect(mod.getSnapshot().theater).toBe(false)
    expect(loc.hash).toBe("#/agent/s1")
    expect(store.has("dux:theater:agent:s1")).toBe(false)
  })

  it("does not come back on re-selection, because the memory went with it", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.noteTheaterOwnershipLost("agent", "s1")
    mod.selectTab("s1", "t2")
    mod.selectTab("s1", "s1")
    expect(mod.getSnapshot().theater).toBe(false)
  })

  it("leaves another pane's mode alone", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectSession("s1")
    mod.enterTheater()
    mod.noteTheaterOwnershipLost("agent", "t2")
    expect(mod.getSnapshot().theater).toBe(true)
    expect(store.get("dux:theater:agent:s1")).toBe("on")
  })

  it("clears a terminal pane's memory too", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", terminals: ["tm1"] },
    ])
    mod.selectTerminal("tm1", { kind: "session", sessionId: "s1" })
    mod.enterTheater()
    mod.noteTheaterOwnershipLost("terminal", "tm1")
    expect(mod.getSnapshot().theater).toBe(false)
    expect(store.has("dux:theater:terminal:tm1")).toBe(false)
  })
})
