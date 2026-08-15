import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./workspaceApi"

// The workspace document arrives two ways: fetched at boot (and on recovery) and
// PUSHED over `/ws/events` on every change. This file exercises the client half
// of that: the shared normalization, the revision discipline that lets a fetch
// and a push land in either order, the reset that keeps a server restart from
// freezing the sidebar, and the fallback that keeps the old ping-then-refetch
// path working against a server that does not push.
//
// The fetch double parks each `/api/v1/workspace` call on a resolver the test
// controls, so "a fetch that resolves late" is a fact the test states rather
// than a race it hopes for.

type RawWorkspace = Spine & { rev?: number }

function makeWorkspace(overrides: Partial<RawWorkspace> = {}): RawWorkspace {
  return {
    projects: [],
    sessions: [],
    terminals: [],
    sidebar: { groups: [], agentless_start: null },
    ...overrides,
  }
}

function session(id: string, projectId = "p1"): Spine["sessions"][number] {
  return { id, project_id: projectId } as unknown as Spine["sessions"][number]
}

// Pending resolvers for each in-flight `/api/v1/workspace` fetch, in call order.
let workspaceResolvers: ((body: RawWorkspace) => void)[] = []
let workspaceFetches = 0

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/workspace")) {
    workspaceFetches++
    const body = await new Promise<RawWorkspace>((resolve) => {
      workspaceResolvers.push(resolve)
    })
    return {
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
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
  workspaceResolvers = []
  workspaceFetches = 0
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

async function waitForResolvers(count: number): Promise<void> {
  await vi.waitFor(() => {
    expect(workspaceResolvers.length).toBeGreaterThanOrEqual(count)
  })
}

// Import the store and settle its boot fetch with `body`.
async function bootStore(body: RawWorkspace) {
  const mod = await import("./store")
  await waitForResolvers(1)
  workspaceResolvers[0](body)
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Let any pending microtasks (a resolved fetch's `.then` chain) run, so
// "nothing applied" is asserted after the apply had its chance.
async function settle(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

function sessionIds(mod: { getSnapshot: () => { spine: Spine | null } }) {
  return mod.getSnapshot().spine?.sessions.map((s) => s.id)
}

describe("the pushed workspace document", () => {
  it("applies a pushed document with no fetch at all", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    const fetchesAfterBoot = workspaceFetches

    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 2,
      workspace: makeWorkspace({ rev: 2, sessions: [session("s1")] }),
    })

    await settle()
    expect(sessionIds(mod)).toEqual(["s1"])
    expect(workspaceFetches).toBe(fetchesAfterBoot)
  })

  it("normalizes a pushed document exactly as it normalizes a fetched one", async () => {
    // An older server's shape, which the ingestion boundary has to repair:
    // terminals nested under their owner rather than flat, and a session
    // missing the fields every consumer treats as required.
    const legacy = {
      projects: [{ id: "p1", name: "p1" }],
      sessions: [{ id: "s1", project_id: "p1", terminals: [{ id: "t1" }] }],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as RawWorkspace

    const fetched = await bootStore(legacy)
    const viaFetch = fetched.getSnapshot().spine
    vi.resetModules()
    workspaceResolvers = []

    const pushed = await bootStore(makeWorkspace({ rev: 1 }))
    pushed.eventsSocket.onEvent({ event: "workspace", rev: 2, workspace: legacy })
    await settle()
    const viaPush = pushed.getSnapshot().spine

    expect(viaPush).toEqual(viaFetch)
    // And the repair actually happened, so the comparison is not two nulls.
    expect(viaPush?.terminals.map((t) => t.id)).toEqual(["t1"])
    expect(viaPush?.terminals[0]?.owner).toEqual({
      kind: "session",
      session_id: "s1",
    })
  })

  it("discards a pushed document whose revision it has already applied", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 5,
      workspace: makeWorkspace({ rev: 5, sessions: [session("current")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["current"])

    // The lag catch-up re-sends the document the client already has; applying
    // it again is harmless but re-running focus/prune is not free, and an
    // out-of-order arrival would be actively wrong.
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 4,
      workspace: makeWorkspace({ rev: 4, sessions: [session("older")] }),
    })
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 5,
      workspace: makeWorkspace({ rev: 5, sessions: [session("duplicate")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["current"])
  })

  it("discards a fetched document older than the pushed one it already applied", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    // A refetch is in flight (a reconnect, say) when a newer push lands.
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await waitForResolvers(2)

    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 9,
      workspace: makeWorkspace({ rev: 9, sessions: [session("pushed")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["pushed"])

    // The slow fetch answers with the state as of rev 2. It must not win.
    workspaceResolvers[1](makeWorkspace({ rev: 2, sessions: [session("stale")] }))
    await settle()
    expect(sessionIds(mod)).toEqual(["pushed"])
  })

  it("discards a pushed document older than the fetched one it already applied", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 9, sessions: [session("fetched")] }))
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 8,
      workspace: makeWorkspace({ rev: 8, sessions: [session("stale")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["fetched"])
  })

  it("takes the subscribe replay when it lands before the boot fetch", async () => {
    const mod = await import("./store")
    await waitForResolvers(1)

    // The socket opened and replayed the current document while the boot GET
    // was still in flight.
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 4,
      workspace: makeWorkspace({ rev: 4, sessions: [session("replayed")] }),
    })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    expect(sessionIds(mod)).toEqual(["replayed"])

    // The boot GET, answered from the same revision, adds nothing.
    workspaceResolvers[0](makeWorkspace({ rev: 4, sessions: [session("replayed")] }))
    await settle()
    expect(sessionIds(mod)).toEqual(["replayed"])
  })

  it("forgets the applied revision when the events socket reopens", async () => {
    // Revisions are per run of the server. A restarted dux starts again at 1,
    // and a client still holding a high-water mark from the previous run would
    // discard every push it is ever sent: a permanently frozen sidebar.
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 7,
      workspace: makeWorkspace({ rev: 7, sessions: [session("before-restart")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["before-restart"])

    mod.eventsSocket.onOpen()
    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 1,
      workspace: makeWorkspace({ rev: 1, sessions: [session("after-restart")] }),
    })
    await settle()
    expect(sessionIds(mod)).toEqual(["after-restart"])
  })

  it("stops refetching on the coarse pings once a push has landed", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    // Before any push, the ping is the only way to learn about a change.
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await waitForResolvers(2)
    workspaceResolvers[1](makeWorkspace({ rev: 2 }))
    await settle()

    mod.eventsSocket.onEvent({
      event: "workspace",
      rev: 3,
      workspace: makeWorkspace({ rev: 3, sessions: [session("pushed")] }),
    })
    await settle()

    const fetchesBefore = workspaceFetches
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    mod.eventsSocket.onEvent({ event: "projects.changed" })
    await settle()
    expect(workspaceFetches).toBe(fetchesBefore)
  })

  it("keeps the ping fallback armed while the pushed frames are unusable", async () => {
    const mod = await bootStore(makeWorkspace({ rev: 1 }))
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})

    // A frame dux cannot make sense of must not disable the path that still
    // works, or a single bad server build freezes every client.
    mod.eventsSocket.onEvent({ event: "workspace", rev: 2, workspace: 42 })
    await settle()
    expect(warn).toHaveBeenCalled()

    const fetchesBefore = workspaceFetches
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await waitForResolvers(fetchesBefore + 1)
    expect(workspaceFetches).toBe(fetchesBefore + 1)
    warn.mockRestore()
  })
})
