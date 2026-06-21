import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { ViewModel } from "./types"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs a `DuxSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly (mirrors storeCommitMessage.test.ts's setup).

const fetchMock = vi.fn(async () => {
  return {
    status: 200,
    json: async () => ({ auth: "disabled" }),
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
  })
  return mod
}

// A minimal ViewModel carrying only the sessions the focus logic inspects; cast
// past the full shape (onViewModel reads just `sessions`/`projects` here).
function vm(sessions: { id: string; project_id: string }[]): ViewModel {
  return {
    sessions: sessions.map((s) => ({ ...s, terminals: [] })),
    projects: [],
  } as unknown as ViewModel
}

describe("auto-focus the agent this client created", () => {
  it("focuses the new agent when it appears in the next ViewModel", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "s2", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    // Selecting also points the changed-files watch at the new session.
    expect(spy).toHaveBeenCalledWith("watch_changed_files", { session_id: "s2" })
    // The token is consumed so a later ViewModel can't re-fire focus.
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
  })

  it("stays armed until the agent actually appears", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // A ViewModel with no new session (creation still in flight) selects nothing.
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().pendingCreateFocus).not.toBeNull()
  })

  it("ignores a new session in another project, then focuses the right one", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // Another client's agent lands in p2 — it must not satisfy our p1 token.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "other", project_id: "p2" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().pendingCreateFocus).not.toBeNull()
    // Our agent finally appears in p1.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "other", project_id: "p2" },
        { id: "mine", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("mine")
  })

  it("does not auto-focus a session this client did not create", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    // No create was armed on this client; a session created elsewhere arrives.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "s2", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("drops the pending focus on disconnect", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    mod.socket.onConn("closed")
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // After reconnect, the agent appearing must not yank focus (intent voided).
    mod.socket.onConn("open")
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "s2", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("focuses a forked agent, scoped to the source session's project", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "pA" }]))
    mod.openForkAgent("s1")
    mod.submitNameDialog("fork-name")
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "pA" },
        { id: "s1-fork", project_id: "pA" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("s1-fork")
  })

  it("focuses an agent adopted from a managed worktree", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.attachWorktree("p1", "/path/to/wt", "adopted")
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "adopted", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("adopted")
  })

  it("a second create supersedes an earlier pending one", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("first")
    // Submit a second create (in p2) before either agent appeared: the token is
    // overwritten so the latest create wins focus.
    mod.openCreateAgent("p2")
    mod.submitNameDialog("second")
    expect(mod.getSnapshot().pendingCreateFocus?.projectId).toBe("p2")
    // A session arriving in the superseded project (p1) must NOT be focused.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "a1", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    // The current target project (p2) gets the focus when its agent appears.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "a1", project_id: "p1" },
        { id: "a2", project_id: "p2" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("a2")
  })

  it("focuses an agent created from a PR", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgentFromPr("p1")
    mod.setCreateAgentPrInput("#123")
    // Empty name lets the server fall back to the PR head branch.
    mod.submitNameDialog("")
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "pr-agent", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBe("pr-agent")
  })

  it("clears the pending focus when the create reports an error", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    mod.openCreateAgent("p1")
    mod.submitNameDialog("my-agent")
    // The async agent launch fails — surfaced as an error-toned status.
    mod.socket.onStatus(null, "error", "Agent launch failed")
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // A later unrelated session in the same project must NOT be auto-focused.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "s2", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("does not arm focus when a fork's source session is unknown", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vm([{ id: "s1", project_id: "p1" }]))
    // Fork a session that isn't in the ViewModel: the project can't be resolved,
    // so no token is armed (rather than an unscoped, any-project one).
    mod.openForkAgent("ghost")
    mod.submitNameDialog("fork-name")
    expect(mod.getSnapshot().pendingCreateFocus).toBeNull()
    // With no token armed, a new session must not be auto-focused.
    mod.socket.onViewModel(
      vm([
        { id: "s1", project_id: "p1" },
        { id: "x", project_id: "p1" },
      ]),
    )
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })
})
