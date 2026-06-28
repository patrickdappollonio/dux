import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Mock sonner before importing the store so the store's top-level
// `import { toast } from "sonner"` picks up our spies (mirrors
// storeStatusToasts.test.ts). A REST 4xx/5xx no longer rides a `/ws`
// CommandResult, so the store's own `.catch` must surface it as `toast.error`.
vi.mock("sonner", () => {
  const toast = Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  })
  return { toast }
})

import { toast } from "sonner"

// Whether the next ACTION endpoint (anything other than the boot reads) should
// fail. Toggled per test before invoking a store action.
let actionFails = false
// The HTTP status an action returns when `actionFails` is set. Defaults to 400;
// the create-conflict test bumps it to 409 to exercise the double-toast guard.
let actionStatus = 400
// Records the [url, init] of every action fetch the store fired.
const actionCalls: Array<{ url: string; method?: string; body: unknown }> = []

function isBootRead(u: string): boolean {
  return (
    u.includes("/api/me") ||
    u.includes("/api/v1/spine") ||
    u.includes("/api/v1/bootstrap") ||
    u.includes("/changes")
  )
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (isBootRead(u)) {
    if (u.includes("/api/v1/spine")) {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          projects: [],
          sessions: [],
          sidebar: { groups: [], agentless_start: null },
        }),
        text: async () => "",
        headers: { get: () => null },
      } as unknown as Response
    }
    if (u.includes("/api/v1/bootstrap")) {
      // Provide a configured provider list so the store's up-front provider
      // validation (which guards against a partial multi-field PATCH) lets a
      // `codex`/`claude` change through.
      return {
        ok: true,
        status: 200,
        json: async () => ({ available_providers: ["codex", "claude"] }),
        text: async () => "",
        headers: { get: () => null },
      } as unknown as Response
    }
    return {
      ok: true,
      status: 200,
      json: async () => ({ auth: "disabled" }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // An action endpoint.
  actionCalls.push({
    url: u,
    method: init?.method,
    body: init?.body ? JSON.parse(init.body as string) : undefined,
  })
  if (actionFails) {
    return {
      ok: false,
      status: actionStatus,
      text: async () => "the server said no",
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
  vi.clearAllMocks()
  actionFails = false
  actionStatus = 400
  actionCalls.length = 0
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
    // Wait for the bootstrap document too: the store's up-front provider
    // validation reads `available_providers` from it.
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  // Drop the boot reads so per-test assertions only see action fetches.
  actionCalls.length = 0
  return mod
}

describe("store write actions route to REST", () => {
  it("createAgent POSTs the new-session endpoint", async () => {
    const mod = await loadStore()
    mod.createAgent("p1", "feat")
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    expect(actionCalls[0].url).toBe("/api/v1/sessions")
    expect(actionCalls[0].method).toBe("POST")
    expect(actionCalls[0].body).toEqual({ kind: "new", project_id: "p1", name: "feat" })
  })

  it("deleteSession DELETEs with the delete_worktree flag", async () => {
    const mod = await loadStore()
    mod.deleteSession("s1", true)
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    expect(actionCalls[0].url).toBe("/api/v1/sessions/s1?delete_worktree=true")
    expect(actionCalls[0].method).toBe("DELETE")
  })

  it("toggleSessionAutoReopen PATCHes auto_reopen", async () => {
    const mod = await loadStore()
    mod.toggleSessionAutoReopen("s1", false)
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    expect(actionCalls[0].url).toBe("/api/v1/sessions/s1")
    expect(actionCalls[0].method).toBe("PATCH")
    expect(actionCalls[0].body).toEqual({ auto_reopen: false })
  })

  it("updateProjectSettings PATCHes the project (and no-ops on an empty patch)", async () => {
    const mod = await loadStore()
    mod.updateProjectSettings("p1", {})
    // Empty patch must not hit the wire.
    expect(actionCalls.length).toBe(0)
    mod.updateProjectSettings("p1", { provider: "codex" })
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    expect(actionCalls[0].url).toBe("/api/v1/projects/p1")
    expect(actionCalls[0].method).toBe("PATCH")
    expect(actionCalls[0].body).toEqual({ provider: "codex" })
  })

  it("pullProject POSTs the new project pull endpoint", async () => {
    const mod = await loadStore()
    mod.pullProject("p1")
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    expect(actionCalls[0].url).toBe("/api/v1/projects/p1/pull")
  })
})

describe("store write actions surface REST errors as a toast", () => {
  it("createAgent shows an error toast on a non-2xx", async () => {
    const mod = await loadStore()
    actionFails = true
    mod.createAgent("p1", "feat")
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the server said no"),
    )
  })

  it("deleteSession shows an error toast on a non-2xx", async () => {
    const mod = await loadStore()
    actionFails = true
    mod.deleteSession("s1", false)
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the server said no"),
    )
  })

  it("reorderProjects clears its optimistic overlay and toasts on error", async () => {
    const mod = await loadStore()
    actionFails = true
    mod.reorderProjects(["p2", "p1"])
    // The overlay is applied synchronously, before the REST call resolves.
    expect(mod.getSnapshot().pendingProjectOrder).toEqual(["p2", "p1"])
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the server said no"),
    )
    // A rejected reorder is never reconciled by a spine, so the overlay must be
    // cleared back to the authoritative order rather than lingering forever.
    expect(mod.getSnapshot().pendingProjectOrder).toBeNull()
  })

  it("reorderSessions clears its optimistic overlay and toasts on error", async () => {
    const mod = await loadStore()
    actionFails = true
    mod.reorderSessions("p1", ["s2", "s1"])
    expect(mod.getSnapshot().pendingSessionOrder).toEqual({
      projectId: "p1",
      ids: ["s2", "s1"],
    })
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the server said no"),
    )
    expect(mod.getSnapshot().pendingSessionOrder).toBeNull()
  })

  it("createAgent does NOT toast a 409 (already surfaced via the /ws status stream)", async () => {
    const mod = await loadStore()
    actionFails = true
    actionStatus = 409
    mod.createAgent("p1", "feat")
    // Wait until the create POST has fired and let its rejection chain fully
    // settle (a macrotask flush drains the request's await chain + the .catch).
    await vi.waitFor(() => expect(actionCalls.length).toBe(1))
    await new Promise((resolve) => setTimeout(resolve, 0))
    // The in-flight guard's 409 is broadcast over /ws (scoped to this client); the
    // REST .catch must stay silent so the user sees exactly one toast, not two.
    expect(toast.error).not.toHaveBeenCalled()
  })

  it("createAgent still toasts a non-409 REST error", async () => {
    const mod = await loadStore()
    actionFails = true
    actionStatus = 400
    mod.createAgent("p1", "feat")
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the server said no"),
    )
  })

  it("clears the connection id when the socket drops", async () => {
    const mod = await loadStore()
    // Read connection from the SAME (post-resetModules) module graph the store
    // imported, so we observe the id the store's handlers actually mutate.
    const conn = await import("./connection")
    // Simulate the server's `connected` first frame on `/ws/events`, then a drop.
    mod.eventsSocket.onEvent({ event: "connected", id: "conn-9" })
    expect(conn.getConnectionId()).toBe("conn-9")
    mod.eventsSocket.onConn("closed")
    // The dead id must not linger; subsequent REST actions fall back to scope All.
    expect(conn.getConnectionId()).toBeNull()
  })
})
