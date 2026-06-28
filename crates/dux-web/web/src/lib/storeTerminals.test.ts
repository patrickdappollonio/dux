import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"

// Exercises the store's Phase 5 companion-terminal lifecycle wiring: createTerminal
// POSTs the nested REST endpoint and focuses the returned terminal; deleteTerminal
// resolves the owning session from the spine and DELETEs the nested endpoint. The
// REST client's own wire behaviour is in terminalsApi.test.ts; here we assert the
// store calls it correctly and reacts (focus) to the result.
//
// The store fires a boot `/api/me` probe and fetches bootstrap + spine at import.
// We steer the probe to auth-off and serve a configurable spine so deleteTerminal
// can resolve a terminal's owner.

function makeBootstrap(): Bootstrap {
  return {
    available_providers: [],
    macros: [],
    palette_commands: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    global_env: {},
    status_clear_seconds: 6,
  }
}

// The spine served by the boot fetch. A session with one companion terminal so
// deleteTerminal can resolve its owner.
let spineBody: unknown = {
  projects: [{ id: "p1", name: "Repo" }],
  sessions: [
    {
      id: "s1",
      project_id: "p1",
      terminals: [{ id: "t1", label: "Terminal 1" }],
    },
  ],
  sidebar: { groups: [] },
}

// Every fetch call, as [url, init], for assertions.
let calls: [string, RequestInit | undefined][] = []

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  calls.push([u, init])
  if (u.includes("/api/me")) {
    return {
      status: 200,
      json: async () => ({ auth: "disabled" }),
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
  if (u.includes("/api/v1/spine")) {
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
  spineBody = {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        project_id: "p1",
        terminals: [{ id: "t1", label: "Terminal 1" }],
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

// Load the store and wait for the boot probe AND the initial spine to settle.
async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
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
        sessionId: "s1",
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
})
