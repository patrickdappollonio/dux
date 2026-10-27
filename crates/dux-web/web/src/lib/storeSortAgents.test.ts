import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"

// Exercises the one-shot agent sort: agents are one global list, so the sort
// persists ONE global order covering every agent, whatever project each
// belongs to.

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
  terminals: [],
  sidebar: { groups: [] },
}

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
  if (u.includes("/api/v1/workspace")) return ok(spineBody)
  if (u.startsWith("/api/v1/sessions/reorder")) {
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
      expect(calls.filter(([u]) => u.startsWith("/api/v1/sessions/reorder"))).toHaveLength(2)
    })
    await new Promise((r) => setTimeout(r, 0))
    expect(mod.getSnapshot().pendingAgentOrder).toEqual(["d", "c", "b", "a"])
  })
})
