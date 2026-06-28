import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"

// Exercises the `loadSpine` in-flight guard: two rapid
// `sessions.changed`/`projects.changed` events fire concurrent `fetchSpine()`s,
// and the OLDER one resolving LAST must be dropped so it can't overwrite the
// newer spine. We control resolution order with a deferred fetch double: each
// `/api/v1/spine` call parks on a promise we resolve by hand, so the test can
// land the second (newer) fetch before the first (older) one.

function makeSpine(overrides: Partial<Spine> = {}): Spine {
  return {
    projects: [],
    sessions: [],
    sidebar: { groups: [], agentless_start: null },
    ...overrides,
  }
}

function session(id: string, projectId: string): Spine["sessions"][number] {
  return { id, project_id: projectId, terminals: [] } as unknown as Spine["sessions"][number]
}

// Pending resolvers for each in-flight `/api/v1/spine` fetch, in call order.
let spineResolvers: ((body: Spine) => void)[] = []

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/spine")) {
    // Park until the test resolves this specific call with a body.
    const body = await new Promise<Spine>((resolve) => {
      spineResolvers.push(resolve)
    })
    return {
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // /api/me, /api/v1/bootstrap, and anything else: auth off / empty body.
  return {
    ok: true,
    status: 200,
    json: async () => ({ auth: "disabled" }),
    text: async () => "",
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
  spineResolvers = []
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// Wait until a spine fetch has parked (its resolver is registered).
async function waitForResolvers(count: number): Promise<void> {
  await vi.waitFor(() => {
    expect(spineResolvers.length).toBeGreaterThanOrEqual(count)
  })
}

describe("loadSpine in-flight guard", () => {
  it("drops an older spine response that resolves after a newer one", async () => {
    const mod = await import("./store")

    // Boot fires one `/api/v1/spine` fetch; resolve it so the store settles.
    await waitForResolvers(1)
    spineResolvers[0](makeSpine({ sessions: [session("boot", "p1")] }))
    await vi.waitFor(() => {
      expect(mod.getSnapshot().auth.phase).not.toBe("checking")
      expect(mod.getSnapshot().spine).not.toBeNull()
    })

    // Fire two invalidations back-to-back: two concurrent fetches park.
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await waitForResolvers(3) // boot + two events

    // Resolve the SECOND (newer) fetch first, then the FIRST (older) one last.
    spineResolvers[2](makeSpine({ sessions: [session("new", "p1")] }))
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual(["new"])
    })
    spineResolvers[1](makeSpine({ sessions: [session("old", "p1")] }))

    // Give the older response a chance to (wrongly) apply, then assert the newer
    // spine still stands — the stale older result was discarded by the guard.
    await Promise.resolve()
    await Promise.resolve()
    expect(mod.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual(["new"])
  })
})
