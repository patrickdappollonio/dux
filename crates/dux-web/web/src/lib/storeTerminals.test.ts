import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

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

afterEach(() => {
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
    // "Stop all" must cover project terminals too, not only sessions'
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
    const mod = await loadStore()
    mod.stopAllRunning()
    await tick()
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
