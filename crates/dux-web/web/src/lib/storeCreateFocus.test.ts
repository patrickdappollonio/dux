import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs an `EventsSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly.
//
// The create-focus logic moved off the broadcast ViewModel onto the spine apply
// path (`GET /api/v1/spine`, refetched on `projects.changed`/`sessions.changed`).
// So these tests drive it by mutating `spineBody` and dispatching a
// `sessions.changed` event, then awaiting the refetch landing in state.

function makeSpine(sessions: { id: string; project_id: string }[]): Spine {
  return {
    projects: [],
    sessions: sessions.map((s) => ({ ...s, terminals: [] })) as Spine["sessions"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/spine")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
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
  spineBody = makeSpine([])
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
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Push a new spine to the store the way the server would: set the body, fire the
// invalidation event, and wait for the refetch to land (the spine reference
// always changes on apply, so this detects application even for unchanged
// content). Returns once `applySpine` (and its focus/prune reconciliation) ran.
async function pushSpine(
  mod: Awaited<ReturnType<typeof loadStore>>,
  sessions: { id: string; project_id: string }[],
): Promise<void> {
  const prev = mod.getSnapshot().spine
  spineBody = makeSpine(sessions)
  mod.eventsSocket.onEvent({ event: "sessions.changed" })
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBe(prev)
  })
}

describe("auto-focus the agent this client created", () => {
  it("focuses the new agent when it appears in the next spine", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    const subSpy = vi.spyOn(mod.eventsSocket, "subscribe")
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    // Selecting also subscribes the new session's changed-files topic.
    expect(subSpy).toHaveBeenCalledWith(["session:s2:changes"])
    // The coarse app-wide topics are NOT clobbered by the focus change — the
    // interest set still carries them alongside the new fine topic.
    expect(new Set(mod.eventsSocket.topics)).toEqual(
      new Set(["sessions", "projects", "config", "session:s2:changes"]),
    )
    // The token is consumed so a later spine can't re-fire focus.
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
  })

  it("stays armed until the agent actually appears", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // A spine with no new session (creation still in flight) selects nothing.
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().pendingCreateFocus).not.toBeNull()
  })

  it("ignores a new session in another project, then focuses the right one", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // Another client's agent lands in p2 — it must not satisfy our p1 token.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "other", project_id: "p2" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().pendingCreateFocus).not.toBeNull()
    // Our agent finally appears in p1.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "other", project_id: "p2" },
      { id: "mine", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("mine")
  })

  it("does not auto-focus a session this client did not create", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    // No create was armed on this client; a session created elsewhere arrives.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("drops the pending focus on disconnect", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    mod.eventsSocket.onConn("closed")
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // After reconnect, the agent appearing must not yank focus (intent voided).
    mod.eventsSocket.onConn("open")
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("focuses a forked agent, scoped to the source session's project", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "pA" }])
    mod.openForkAgent("s1")
    mod.submitNameDialog("fork-name")
    await pushSpine(mod, [
      { id: "s1", project_id: "pA" },
      { id: "s1-fork", project_id: "pA" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s1-fork")
  })

  it("focuses an agent adopted from a managed worktree", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.attachWorktree("p1", "/path/to/wt", "adopted")
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "adopted", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("adopted")
  })

  it("a second create supersedes an earlier pending one", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("first")
    // Submit a second create (in p2) before either agent appeared: the token is
    // overwritten so the latest create wins focus.
    mod.openCreateAgent("p2")
    mod.submitNameDialog("second")
    expect(mod.getSnapshot().pendingCreateFocus?.projectId).toBe("p2")
    // A session arriving in the superseded project (p1) must NOT be focused.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "a1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    // The current target project (p2) gets the focus when its agent appears.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "a1", project_id: "p1" },
      { id: "a2", project_id: "p2" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("a2")
  })

  it("focuses an agent created from a PR", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgentFromPr("p1")
    mod.setCreateAgentPrInput("#123")
    // Empty name lets the server fall back to the PR head branch.
    mod.submitNameDialog("")
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "pr-agent", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("pr-agent")
  })

  it("clears the pending focus when the create reports an error", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // The async agent launch fails — surfaced as an error-toned status event.
    mod.eventsSocket.onEvent({
      event: "status",
      tone: "error",
      message: "Agent launch failed",
    })
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // A later unrelated session in the same project must NOT be auto-focused.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("expires a stale pending focus instead of grabbing a later session", async () => {
    // A create that never lands must not leave the token armed forever: once it
    // ages past the TTL, a later unrelated session in the same project is ignored
    // and the token disarms itself.
    const realNow = Date.now
    const spy = vi.spyOn(Date, "now").mockImplementation(() => realNow.call(Date))
    try {
      const mod = await loadStore()
      await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
      mod.openCreateAgent("p1")
      mod.submitNameDialog("my-agent")
      expect(mod.getSnapshot().pendingCreateFocus).not.toBeNull()
      // Jump the clock past the focus TTL (90s) before the agent ever arrives.
      spy.mockImplementation(() => realNow.call(Date) + 91_000)
      await pushSpine(mod, [
        { id: "s1", project_id: "p1" },
        { id: "s2", project_id: "p1" },
      ])
      expect(mod.getSnapshot().selectedSessionId).toBeNull()
      expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    } finally {
      spy.mockRestore()
    }
  })

  it("does not arm focus when a fork's source session is unknown", async () => {
    const mod = await loadStore()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    // Fork a session that isn't in the spine: the project can't be resolved,
    // so no token is armed (rather than an unscoped, any-project one).
    mod.openForkAgent("ghost")
    mod.submitNameDialog("fork-name")
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // With no token armed, a new session must not be auto-focused.
    await pushSpine(mod, [
      { id: "s1", project_id: "p1" },
      { id: "x", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })
})
