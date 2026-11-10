import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Loaded once at collection, where no test timeout applies and, with no window
// stubbed yet, the store boots nothing. Its first load is by far the slowest
// step in the file; left to the first test, a busy machine pushes that test
// past its timeout. Every test still imports its own fresh instance after
// `vi.resetModules`.
import "./store"

import type { Bootstrap } from "./bootstrapApi"

// Exercises the store's companion-terminal lifecycle wiring: createTerminal
// POSTs the nested REST endpoint and focuses the returned terminal; deleteTerminal
// resolves the owning session from the spine and DELETEs the nested endpoint. The
// REST client's own wire behaviour is in terminalsApi.test.ts; here we assert the
// store calls it correctly and reacts (focus) to the result.
//
// The store fetches bootstrap + spine at import. We serve a configurable spine
// so deleteTerminal can resolve a terminal's owner.

function makeBootstrap(): Bootstrap {
  return {
    available_providers: [],
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
  }
}

// The spine served by the boot fetch. A session with one companion terminal so
// deleteTerminal can resolve its owner.
let spineBody: unknown = {
  projects: [{ id: "p1", name: "Repo" }],
  sessions: [{ id: "s1", project_id: "p1" }],
  terminals: [
    {
      id: "t1",
      label: "Terminal 1",
      owner: { kind: "session", session_id: "s1" },
    },
  ],
  sidebar: { groups: [] },
}

// Every fetch call, as [url, init], for assertions.
let calls: [string, RequestInit | undefined][] = []
// When set, the terminals reorder POST responds 400 so the overlay-clear-on-error
// path can be exercised.
let reorderFail = false
// A terminal DELETE to this exact URL is refused because somebody else is
// attached to it.
let attachedUrl: string | null = null

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  calls.push([u, init])
  if (u === "/api/v1/terminals/reorder") {
    return {
      ok: !reorderFail,
      status: reorderFail ? 400 : 204,
      text: async () => (reorderFail ? "nope" : ""),
      headers: { get: () => null },
    } as unknown as Response
  }
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
  if (u.endsWith("/terminals")) {
    // POST create.
    return {
      ok: true,
      status: 201,
      json: async () => ({ terminal_id: "t9", label: "Terminal 2" }),
      text: async () => JSON.stringify({ terminal_id: "t9", label: "Terminal 2" }),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u === attachedUrl) {
    const body = {
      error: "attached",
      blockers: [
        {
          surface: "browser",
          device: null,
          address: "10.0.0.7",
          verified: false,
          driving: false,
          target: { kind: "terminal", id: "t1" },
          key: "k1",
        },
      ],
    }
    return {
      ok: false,
      status: 409,
      text: async () => JSON.stringify(body),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/terminals/")) {
    // DELETE.
    return {
      ok: true,
      status: 204,
      text: async () => "",
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
  calls = []
  reorderFail = false
  spineBody = {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [{ id: "s1", project_id: "p1" }],
    terminals: [
      {
        id: "t1",
        label: "Terminal 1",
        owner: { kind: "session", session_id: "s1" },
      },
    ],
    sidebar: { groups: [] },
  }
  vi.stubGlobal("location", { host: "localhost:0", protocol: "http:" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {}, pushState: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

// `vi.resetModules` gives the next test a fresh store but does not stop this
// one: its app socket would keep reconnecting on real timers, and every socket
// and request it makes would land on the next test's fakes.
afterEach(async () => {
  const store = await import("./store")
  store.eventsSocket.dispose()
  vi.unstubAllGlobals()
})

// Load the store and wait for the initial spine to settle.
async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

const tick = () => new Promise((r) => setTimeout(r, 0))

function find(predicate: (url: string, init?: RequestInit) => boolean) {
  return calls.find(([u, init]) => predicate(u, init))
}

describe("store companion-terminal lifecycle", () => {
  it("createTerminal POSTs the nested endpoint and focuses the new terminal", async () => {
    const mod = await loadStore()
    mod.createTerminal("s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "session", sessionId: "s1" },
      })
    })
    const post = find(
      (u, init) => u === "/api/v1/sessions/s1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
  })

  it("deleteTerminal resolves the owning session and DELETEs the nested endpoint", async () => {
    const mod = await loadStore()
    mod.deleteTerminal("t1")
    await tick()
    const del = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/terminals/t1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("deleteTerminal is a no-op for an unknown terminal (no owner in the spine)", async () => {
    const mod = await loadStore()
    mod.deleteTerminal("does-not-exist")
    await tick()
    const del = find((u, init) => init?.method === "DELETE")
    expect(del).toBeUndefined()
  })

  it("createProjectTerminal POSTs the project endpoint and focuses with a project owner", async () => {
    const mod = await loadStore()
    mod.createProjectTerminal("p1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "project", projectId: "p1" },
      })
    })
    const post = find(
      (u, init) => u === "/api/v1/projects/p1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    // A project terminal has no session context.
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("deleteTerminal routes a PROJECT-owned terminal to the project endpoint", async () => {
    // An owner scan that only walks sessions resolves nothing for a project
    // terminal, so Close would silently do nothing.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [],
      terminals: [
        {
          id: "pt1",
          label: "Terminal 1",
          owner: { kind: "project", project_id: "p1" },
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.deleteTerminal("pt1")
    await tick()
    const del = find(
      (u, init) =>
        u === "/api/v1/projects/p1/terminals/pt1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("createStandaloneTerminal POSTs the un-nested endpoint and focuses it", async () => {
    const mod = await loadStore()
    mod.createStandaloneTerminal()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "standalone" },
      })
    })
    // No owner id anywhere in the address: that is the whole shape of it.
    const post = find(
      (u, init) => u === "/api/v1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("deleteTerminal routes a STANDALONE terminal to the un-nested endpoint", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "solo1",
          label: "Terminal 1",
          owner: { kind: "standalone", cwd_label: "~" },
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.deleteTerminal("solo1")
    await tick()
    const del = find(
      (u, init) => u === "/api/v1/terminals/solo1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("stopAllRunning deletes project terminals too", async () => {
    // "Force stop everything" must cover project terminals too, not only sessions'
    // terminals.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t1",
          label: "Terminal 1",
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "pt1",
          label: "Terminal 1",
          owner: { kind: "project", project_id: "p1" },
        },
      ],
      sidebar: { groups: [] },
    }
    // Somebody else is attached to the session's terminal: that one is
    // refused and named, the project's goes.
    attachedUrl = "/api/v1/sessions/s1/terminals/t1"
    const mod = await loadStore()
    const refused = await mod.stopAllRunning()
    attachedUrl = null
    const sessionDel = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/terminals/t1" && init?.method === "DELETE",
    )
    const projectDel = find(
      (u, init) =>
        u === "/api/v1/projects/p1/terminals/pt1" && init?.method === "DELETE",
    )
    expect(sessionDel).toBeDefined()
    expect(projectDel).toBeDefined()
    expect(refused?.agents).toEqual([])
    expect(refused?.terminals).toEqual(["t1"])
    expect(refused?.blockers.map((b) => b.key)).toEqual(["k1"])

    // Going ahead stops only what was refused, over exactly who was shown.
    calls = []
    expect(await mod.stopAllRunning(refused, ["k1"])).toBeNull()
    expect(
      calls
        .filter(([, init]) => init?.method === "DELETE" || init?.method === "POST")
        .map(([u]) => u),
    ).toEqual(["/api/v1/sessions/s1/terminals/t1?force_connected=k1"])
  })

  // The panic button is the ONE caller that skips the shutdown grace. A polite
  // wait per agent is the opposite of what somebody pressing "Stop everything"
  // is asking for, so the flag has to be on this path and off every other one.
  it("stopAllRunning forces the agent stop rather than waiting out the grace", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status: "active" }],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.stopAllRunning()
    await tick()
    const kill = find(
      (u, init) => u === "/api/v1/sessions/s1/kill" && init?.method === "POST",
    )
    expect(kill).toBeDefined()
    expect(JSON.parse(String(kill?.[1]?.body))).toEqual({ force: true })
  })

  it("the per-agent detach asks for the grace instead of forcing", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status: "active" }],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.killSessionPty("s1")
    await tick()
    const kill = find(
      (u, init) => u === "/api/v1/sessions/s1/kill" && init?.method === "POST",
    )
    expect(JSON.parse(String(kill?.[1]?.body))).toEqual({ force: false })
  })
})

describe("store terminal reorder overlay", () => {
  it("reorderTerminals sets the optimistic overlay and POSTs the full id order", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t3", "t1", "t2"])
    // The overlay is applied synchronously so the UI reflects the drop instantly.
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t3", "t1", "t2"])
    await tick()
    const post = find(
      (u, init) => u === "/api/v1/terminals/reorder" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      terminal_ids: ["t3", "t1", "t2"],
    })
  })

  it("clears the overlay once a spine confirms the order (reconcile)", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
    await tick()
    // A spine whose terminals' global sort_order matches the dragged order: t2
    // before t1. The reconcile sorts every terminal by sort_order and clears the
    // overlay when that matches what we optimistically applied.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t2",
          label: "Terminal 2",
          sort_order: 0,
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "t1",
          label: "Terminal 1",
          sort_order: 1,
          owner: { kind: "session", session_id: "s1" },
        },
      ],
      sidebar: { groups: [] },
    }
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
    })
  })

  it("keeps the overlay when the spine order does not yet match", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    await tick()
    // Spine still reflects the OLD order (t1 before t2): overlay must persist.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t1",
          label: "Terminal 1",
          sort_order: 0,
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "t2",
          label: "Terminal 2",
          sort_order: 1,
          owner: { kind: "session", session_id: "s1" },
        },
      ],
      sidebar: { groups: [] },
    }
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine?.terminals).toHaveLength(2)
    })
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
  })

  it("clears the overlay and does not throw when the reorder POST fails", async () => {
    reorderFail = true
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
    // The rejected POST clears the overlay so the UI snaps back to server order.
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
    })
  })
})

// An agent's stack: which layout it opens in when one of its terminals is
// brought to the front, how focus moves between its parts, and how its dock
// reorders.
describe("store agent stack", () => {
  function withSessionStatus(status: string, terminals: unknown[] = []) {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status }],
      terminals,
      sidebar: { groups: [] },
    }
  }
  const companion = (id: string, sortOrder: number) => ({
    id,
    label: id,
    owner: { kind: "session", session_id: "s1" },
    sort_order: sortOrder,
  })

  it("opens a stopped agent's new terminal maximized, so nothing starts the agent", async () => {
    withSessionStatus("detached")
    const mod = await loadStore()
    mod.createTerminal("s1")
    await vi.waitFor(() => expect(mod.getSnapshot().stackLayouts.s1).toBe("terminal"))
  })

  it("opens at least the split when a terminal is brought to the front of a folded dock", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.setStackLayout("s1", "agent")
    mod.selectTerminal("t1", { kind: "session", sessionId: "s1" })
    expect(mod.getSnapshot().stackLayouts.s1).toBe("split")
    expect(mod.getSnapshot().stackTerminals.s1).toBe("t1")
  })

  it("judges every arrival at an agent afresh", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.setStackLayout("s1", "terminal")
    mod.selectSession(null)
    mod.selectSession("s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBeUndefined()
  })

  it("moves typing between the parts on screen without a new history entry", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const pushState = vi.fn()
    const replaceState = vi.fn()
    vi.stubGlobal("history", { go: () => {}, pushState, replaceState, state: null })
    const mod = await loadStore()
    mod.selectSession("s1")
    const pushesBefore = pushState.mock.calls.length
    mod.chooseStackPart("s1", "t1")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "session", sessionId: "s1" },
    })
    const slot = mod.getSnapshot().spine!.sessions[0].slot_tab_id
    mod.chooseStackPart("s1", slot)
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: slot,
    })
    expect(pushState.mock.calls.length).toBe(pushesBefore)
    expect(replaceState).toHaveBeenCalled()
  })

  it("pushes for a new terminal, which is a new position", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const pushState = vi.fn()
    vi.stubGlobal("history", { go: () => {}, pushState, replaceState: vi.fn(), state: null })
    const mod = await loadStore()
    mod.selectSession("s1")
    const before = pushState.mock.calls.length
    mod.createTerminal("s1")
    await vi.waitFor(() => expect(pushState.mock.calls.length).toBe(before + 1))
  })

  it("leaves the list's sort mode alone when the dock is reordered", async () => {
    withSessionStatus("active", [companion("t1", 1), companion("t2", 2)])
    const mod = await loadStore()
    const sortBefore = mod.agentSortValue(mod.getSnapshot())
    mod.reorderDock("s1", ["t2", "t1"])
    await tick()
    expect(mod.agentSortValue(mod.getSnapshot())).toBe(sortBefore)
    expect(calls.some(([u]) => u.includes("/config"))).toBe(false)
  })

  it("puts a started agent back in the layout this device last chose, or the split", async () => {
    withSessionStatus("detached", [companion("t1", 1)])
    const mem = new Map<string, string>([["dux:stack:s1", JSON.stringify({ layout: "agent" })]])
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: (k: string, v: string) => void mem.set(k, v),
      removeItem: (k: string) => void mem.delete(k),
    })
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.startStoppedAgent("s1", "s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBe("agent")
    mem.clear()
    mod.startStoppedAgent("s1", "s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBe("split")
  })

  it("keeps the tab shown and its resting state while typing moves to the dock", async () => {
    withSessionStatus("detached", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    const slot = mod.getSnapshot().spine!.sessions[0].slot_tab_id
    mod.holdStoppedAgent("s1", slot)
    mod.chooseStackPart("s1", "t1")
    mod.selectTerminal("t1", { kind: "session", sessionId: "s1" })
    expect(mod.getSnapshot().endedInViewTab).toBe(slot)
    expect(mod.getSnapshot().stackTabs.s1).toBe(slot)
  })

  it("reorders one dock among the places its terminals hold, leaving every other terminal put", async () => {
    withSessionStatus("active", [
      companion("t1", 1),
      { id: "p-term", label: "p", owner: { kind: "project", project_id: "p1" }, sort_order: 2 },
      companion("t2", 3),
    ])
    const mod = await loadStore()
    mod.reorderDock("s1", ["t2", "t1"])
    await tick()
    const post = find((u, init) => u === "/api/v1/terminals/reorder" && init?.method === "POST")
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      terminal_ids: ["t2", "p-term", "t1"],
    })
  })
})
