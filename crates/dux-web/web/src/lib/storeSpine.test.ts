import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"
import { partitionProjects } from "./projects"

// Exercises the spine slice end to end: the boot fetch populating the slice, a
// `projects.changed`/`sessions.changed` event triggering a refetch, the
// prune-on-gone and order-reconciliation behaviours that moved off the broadcast
// ViewModel onto the spine apply path, and a representative consumer
// (partitionProjects) reading from the slice. The `spineApi` wire behaviour (GET
// shape, error mapping) lives in `spineApi.test.ts`; here we drive the store's
// integration via a controllable fetch double.
//
// The store fires a `GET /api/v1/spine` at import. We serve the spine body from
// `spineBody`, which the fetch double reads at call time so a test can mutate it
// before a refetch.

function makeSpine(overrides: Partial<Spine> = {}): Spine {
  return {
    projects: [],
    sessions: [],
    sidebar: { groups: [], agentless_start: null },
    ...overrides,
  }
}

function session(id: string, projectId: string): Spine["sessions"][number] {
  return {
    id,
    project_id: projectId,
    terminals: [],
  } as unknown as Spine["sessions"][number]
}

function project(id: string): Spine["projects"][number] {
  return { id } as unknown as Spine["projects"][number]
}

let spineBody: Spine = makeSpine()
let spineFetches = 0
// When true, the spine GET rejects (simulated network failure) — used to
// exercise the failed-first-load → reconnect-retry recovery path.
let spineShouldFail = false

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/spine")) {
    spineFetches++
    if (spineShouldFail) throw new Error("network down")
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    // The selected session's changed-files fetch (selectSession kicks one off).
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // /api/v1/bootstrap and anything else: empty body.
  return {
    ok: true,
    status: 200,
    json: async () => ({}),
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
  spineBody = makeSpine()
  spineFetches = 0
  spineShouldFail = false
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

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Set the spine body, fire the invalidation event, and wait for the refetch to
// land. The applied spine is always a fresh object reference, so this detects
// application even when the content is unchanged.
async function pushSpine(
  mod: Awaited<ReturnType<typeof loadStore>>,
  body: Spine,
  event: "sessions.changed" | "projects.changed" = "sessions.changed",
): Promise<void> {
  const prev = mod.getSnapshot().spine
  spineBody = body
  mod.eventsSocket.onEvent({ event })
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBe(prev)
  })
}

describe("spine slice", () => {
  it("the boot fetch populates the slice", async () => {
    spineBody = makeSpine({
      projects: [project("p1")],
      sessions: [session("s1", "p1")],
    })
    const mod = await loadStore()
    const s = mod.getSnapshot().spine
    expect(s?.projects.map((p) => p.id)).toEqual(["p1"])
    expect(s?.sessions.map((x) => x.id)).toEqual(["s1"])
    // Exactly one spine GET on boot.
    expect(spineFetches).toBe(1)
  })

  it("a sessions.changed event triggers a refetch that replaces the slice", async () => {
    const mod = await loadStore()
    const before = spineFetches
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
      "sessions.changed",
    )
    expect(mod.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual([
      "s1",
      "s2",
    ])
    expect(spineFetches).toBe(before + 1)
  })

  it("a projects.changed event triggers a refetch that replaces the slice", async () => {
    const mod = await loadStore()
    const before = spineFetches
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p1"), project("p2")] }),
      "projects.changed",
    )
    expect(mod.getSnapshot().spine?.projects.map((p) => p.id)).toEqual([
      "p1",
      "p2",
    ])
    expect(spineFetches).toBe(before + 1)
  })

  it("an unrelated event does not refetch the spine", async () => {
    const mod = await loadStore()
    const before = spineFetches
    // A session.changes event for an unselected session must not touch the spine.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s-x", rev: 1 })
    expect(spineFetches).toBe(before)
  })

  it("prunes the selection when its session vanishes from the spine", async () => {
    const mod = await loadStore()
    await pushSpine(mod, makeSpine({ sessions: [session("s1", "p1")] }))
    mod.selectSession("s1")
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    // The session is gone in the next spine — the selection must clear.
    await pushSpine(mod, makeSpine({ sessions: [] }))
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("retires an optimistic session-order overlay once the spine matches", async () => {
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
    )
    // Optimistically reorder p1's sessions; the overlay holds until confirmed.
    mod.reorderSessions("p1", ["s2", "s1"])
    expect(mod.getSnapshot().pendingSessionOrder).not.toBeNull()
    // A spine whose order does NOT match keeps the overlay.
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
    )
    expect(mod.getSnapshot().pendingSessionOrder).not.toBeNull()
    // A spine confirming the new order retires it.
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s2", "p1"), session("s1", "p1")] }),
    )
    expect(mod.getSnapshot().pendingSessionOrder).toBeNull()
  })

  it("retires an optimistic project-order overlay once the spine matches", async () => {
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p1"), project("p2")] }),
      "projects.changed",
    )
    mod.reorderProjects(["p2", "p1"])
    expect(mod.getSnapshot().pendingProjectOrder).not.toBeNull()
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p2"), project("p1")] }),
      "projects.changed",
    )
    expect(mod.getSnapshot().pendingProjectOrder).toBeNull()
  })

  it("retries a failed first spine load on a reconnect onOpen", async () => {
    // The very first load (driven by boot()) fails, so the slice stays null.
    spineShouldFail = true
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().booted).toBe(true)
    })
    await vi.waitFor(() => {
      expect(spineFetches).toBeGreaterThanOrEqual(1)
    })
    expect(mod.getSnapshot().spine).toBeNull()

    // The initial connect's open consumes the skip flag and does NOT refetch
    // (boot() already drove the first load -- even though it failed).
    const afterInitialOpen = spineFetches
    mod.eventsSocket.onOpen()
    expect(spineFetches).toBe(afterInitialOpen)
    expect(mod.getSnapshot().spine).toBeNull()

    // A later RE-connect retries even though the slice is still null (the old
    // `spine !== null` guard would have skipped this forever). It now succeeds.
    spineShouldFail = false
    mod.eventsSocket.onOpen()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
  })

  it("a representative consumer (partitionProjects) reads from the slice", async () => {
    spineBody = makeSpine({
      projects: [project("p1")],
      sessions: [session("s1", "p1")],
      sidebar: {
        groups: [
          {
            project_id: "p1",
            name: "Project One",
            orphaned: false,
            path_missing: false,
            session_ids: ["s1"],
          },
        ],
        agentless_start: null,
      },
    })
    const mod = await loadStore()
    const s = mod.getSnapshot().spine
    const partitioned = partitionProjects(s?.sidebar, s?.projects ?? [], s?.sessions ?? [])
    expect(partitioned.withAgents).toEqual(["p1"])
    expect(partitioned.projectName("p1")).toBe("Project One")
    expect(partitioned.grouped.get("p1")?.map((x) => x.id)).toEqual(["s1"])
  })
})
