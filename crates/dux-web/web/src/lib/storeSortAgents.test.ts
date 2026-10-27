import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"

const notifyError = vi.hoisted(() => vi.fn())
vi.mock("./notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./notify")>()
  return { ...actual, notifyError }
})

// Exercises the one-shot agent sort and the optimistic reorder overlays: agents
// are one global list, so the sort persists ONE global order covering every
// agent, whatever project each belongs to.

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

function managed(id: string, projectId: string, createdAt: string) {
  return {
    id,
    workspace: {
      kind: "managed",
      project_id: projectId,
      branch_name: id,
      initial_branch: id,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${id}`,
    },
    created_at: createdAt,
    updated_at: createdAt,
  }
}

function terminal(id: string, sortOrder: number) {
  return {
    id,
    label: id,
    sort_order: sortOrder,
    owner: { kind: "standalone", cwd_label: "~" },
  }
}

// Listed so that the two projects' agents interleave, oldest first.
const spineBody = {
  projects: [
    { id: "p1", name: "One" },
    { id: "p2", name: "Two" },
  ],
  sessions: [
    managed("a", "p2", "2026-01-01T00:00:00Z"),
    managed("b", "p1", "2026-01-02T00:00:00Z"),
    managed("c", "p2", "2026-01-03T00:00:00Z"),
    managed("d", "p1", "2026-01-04T00:00:00Z"),
  ],
  terminals: [terminal("t1", 0), terminal("t2", 1)],
  sidebar: { groups: [] },
}
// What the workspace route answers; `pushSpine` replaces it.
let currentSpine: unknown = spineBody

let calls: [string, RequestInit | undefined][] = []
// When set, the next reorder answers only when the test settles it.
let holdNextReorder: ((settle: (ok: boolean) => void) => void) | null = null

function ok(body: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => null },
  } as unknown as Response
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  calls.push([u, init])
  if (u.includes("/api/v1/bootstrap")) return ok(makeBootstrap())
  if (u.includes("/api/v1/workspace")) return ok(currentSpine)
  if (u.startsWith("/api/v1/sessions/reorder") || u === "/api/v1/terminals/reorder") {
    const hold = holdNextReorder
    holdNextReorder = null
    if (!hold) return ok({})
    const answered = await new Promise<boolean>((resolve) => hold(resolve))
    if (answered) return ok({})
    return {
      ok: false,
      status: 400,
      text: async () => "nope",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) return ok({ rev: 1, staged: [], unstaged: [] })
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
  holdNextReorder = null
  currentSpine = spineBody
  notifyError.mockClear()
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

type Store = typeof import("./store")

// Serve `spine` from the workspace route, announce a change, and wait for the
// refetch to be applied.
async function pushSpine(mod: Store, spine: unknown): Promise<void> {
  const prev = mod.getSnapshot().spine
  currentSpine = spine
  mod.eventsSocket.onEvent({ event: "sessions.changed" })
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBe(prev)
  })
}

function sessionsIn(order: string[]) {
  return order.map((id, i) =>
    managed(id, "p1", `2026-01-0${i + 1}T00:00:00Z`),
  )
}

describe("sortAgents", () => {
  it("persists one global order over every project's agents", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    mod.sortAgents("created")

    await vi.waitFor(() => {
      expect(calls.some(([u]) => u.startsWith("/api/v1/sessions/reorder"))).toBe(true)
    })
    const reorders = calls.filter(([u]) => u.startsWith("/api/v1/sessions/reorder"))
    expect(reorders.map(([u]) => u)).toEqual(["/api/v1/sessions/reorder-global"])
    expect(JSON.parse(String(reorders[0][1]?.body))).toEqual({
      session_ids: ["d", "c", "b", "a"],
    })
  })

  it("a failed earlier reorder leaves a later sort's order on screen", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    let settleFirst: ((ok: boolean) => void) | null = null
    holdNextReorder = (settle) => {
      settleFirst = settle
    }

    mod.reorderAgents(["a", "b", "c", "d"])
    await vi.waitFor(() => expect(settleFirst).not.toBeNull())
    mod.sortAgents("created")
    settleFirst!(false)

    await vi.waitFor(() => {
      expect(notifyError).toHaveBeenCalledWith("nope")
    })
    expect(mod.getSnapshot().pendingAgentOrder).toEqual(["d", "c", "b", "a"])
  })
})

describe("reorderTerminals", () => {
  it("a failed earlier reorder leaves a later reorder's order on screen", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    let settleFirst: ((ok: boolean) => void) | null = null
    holdNextReorder = (settle) => {
      settleFirst = settle
    }

    mod.reorderTerminals(["t1", "t2"])
    await vi.waitFor(() => expect(settleFirst).not.toBeNull())
    mod.reorderTerminals(["t2", "t1"])
    settleFirst!(false)

    await vi.waitFor(() => {
      expect(notifyError).toHaveBeenCalledWith("nope")
    })
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
  })

  it("an events-socket close drops an in-flight reorder's overlay", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    holdNextReorder = () => {}

    mod.reorderTerminals(["t2", "t1"])
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
    mod.eventsSocket.onConn("closed")

    expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
  })
})

describe("the reorder overlays clear once the server shows their order", () => {
  it("clears the agent overlay when an agent arrived alongside the confirmed order", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    mod.reorderAgents(["b", "a", "c", "d"])
    await pushSpine(mod, { ...spineBody, sessions: sessionsIn(["a", "b", "c", "d", "e"]) })
    expect(mod.getSnapshot().pendingAgentOrder).toEqual(["b", "a", "c", "d"])

    await pushSpine(mod, { ...spineBody, sessions: sessionsIn(["b", "a", "c", "d", "e"]) })
    expect(mod.getSnapshot().pendingAgentOrder).toBeNull()
  })

  it("clears the agent overlay when an agent left alongside the confirmed order", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    mod.reorderAgents(["b", "a", "d", "c"])
    await pushSpine(mod, { ...spineBody, sessions: sessionsIn(["b", "a", "d"]) })
    expect(mod.getSnapshot().pendingAgentOrder).toBeNull()
  })

  it("clears the terminal overlay when a terminal arrived alongside the confirmed order", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    mod.reorderTerminals(["t2", "t1"])
    await pushSpine(mod, {
      ...spineBody,
      terminals: [terminal("t1", 0), terminal("t2", 1), terminal("t3", 2)],
    })
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])

    await pushSpine(mod, {
      ...spineBody,
      terminals: [terminal("t1", 1), terminal("t2", 0), terminal("t3", 2)],
    })
    expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
  })

  it("clears the terminal overlay when a terminal left alongside the confirmed order", async () => {
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    mod.reorderTerminals(["t2", "t1"])
    await pushSpine(mod, { ...spineBody, terminals: [terminal("t2", 0)] })
    expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
  })
})
