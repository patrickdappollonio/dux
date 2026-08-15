import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./workspaceApi"

// Per-agent "remember last-focused tab": `selectSession` restores an agent's
// remembered tab (SessionView.last_focused_tab) by routing through `selectTab`
// when the remembered tab is still a live extra tab, and `selectTab` persists
// every explicit switch via `PUT .../focused-tab` (fire-and-forget, J3/J4).
// An explicit boot deep-link (#/agent/<id>/tab/<other>) still wins over the
// remembered tab — `restoreDeepLink` is untouched by this feature.

function makeSpine(
  sessions: {
    id: string
    project_id: string
    tabs?: string[]
    last_focused_tab?: string | null
  }[],
): Spine {
  return {
    projects: [],
    sessions: sessions.map((s) => ({
      id: s.id,
      project_id: s.project_id,
      terminals: [],
      // The session-slot tab's id always equals the session id; any extra ids
      // are extra tabs.
      tabs: [{ id: s.id }, ...(s.tabs ?? []).map((id) => ({ id }))],
      last_focused_tab: s.last_focused_tab ?? null,
    })) as unknown as Spine["sessions"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])
let putCalls: { url: string; body: unknown }[] = []
let replaceStateMock: ReturnType<typeof vi.fn>

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/focused-tab") && init?.method === "PUT") {
    putCalls.push({
      url: u,
      body: init.body ? JSON.parse(init.body as string) : undefined,
    })
    return {
      ok: true,
      status: 200,
      json: async () => ({}),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // bootstrap + anything else.
  return {
    ok: true,
    status: 200,
    json: async () => ({}),
    text: async () => "{}",
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
  putCalls = []
  replaceStateMock = vi.fn()
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    go: () => {},
    replaceState: replaceStateMock,
    state: null,
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore(
  hash: string,
  sessions: {
    id: string
    project_id: string
    tabs?: string[]
    last_focused_tab?: string | null
  }[],
) {
  vi.stubGlobal("location", {
    protocol: "http:",
    host: "localhost:0",
    hash,
    pathname: "/",
    search: "",
  })
  spineBody = makeSpine(sessions)
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

describe("selectSession restores the remembered tab", () => {
  it("selects the remembered extra tab and writes the /tab/ hash", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"], last_focused_tab: "t2" },
    ])
    replaceStateMock.mockClear()

    mod.selectSession("s1")

    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t2",
    })
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "#/agent/s1/tab/t2")
  })

  it("falls back to the session-slot tab when the remembered tab was closed", async () => {
    const mod = await loadStore("", [
      {
        id: "s1",
        project_id: "p1",
        tabs: ["t-other"],
        last_focused_tab: "gone",
      },
    ])
    replaceStateMock.mockClear()

    mod.selectSession("s1")

    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "#/agent/s1")
  })

  it("lands on the session-slot tab when there is no remembered tab", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"], last_focused_tab: null },
    ])
    mod.selectSession("s1")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("an explicit boot deep-link still wins over the remembered tab", async () => {
    // The session remembers t2, but the boot URL explicitly deep-links to a
    // DIFFERENT tab (t3): the deep link must win (restoreDeepLink runs first
    // and is untouched by the focus-memory feature).
    const mod = await loadStore("#/agent/s1/tab/t3", [
      {
        id: "s1",
        project_id: "p1",
        tabs: ["t2", "t3"],
        last_focused_tab: "t2",
      },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t3",
    })
  })
})

describe("restoreDeepLink does not persist", () => {
  it("following a deep link to an extra tab selects it but fires no PUT", async () => {
    const mod = await loadStore("#/agent/s1/tab/t2", [
      { id: "s1", project_id: "p1", tabs: ["t2"], last_focused_tab: null },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t2",
    })
    // Merely following the link must not rewrite the workspace-shared
    // remembered tab for everyone.
    expect(putCalls).toHaveLength(0)
  })
})

describe("selectTab / selectSession persist the focus choice", () => {
  it("selectTab PUTs the focused-tab endpoint with the chosen extra tab", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    putCalls = []

    mod.selectTab("s1", "t2")

    await vi.waitFor(() => {
      expect(putCalls).toHaveLength(1)
    })
    expect(putCalls[0]).toEqual({
      url: "/api/v1/sessions/s1/focused-tab",
      body: { tab_id: "t2" },
    })
  })

  it("selectTab back to the session-slot tab PUTs a null tab_id", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    mod.selectTab("s1", "t2")
    await vi.waitFor(() => expect(putCalls).toHaveLength(1))
    putCalls = []

    mod.selectTab("s1", "s1")

    await vi.waitFor(() => {
      expect(putCalls).toHaveLength(1)
    })
    expect(putCalls[0]).toEqual({
      url: "/api/v1/sessions/s1/focused-tab",
      body: { tab_id: null },
    })
  })

  it("selectSession restoring a remembered extra tab also persists (idempotent)", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"], last_focused_tab: "t2" },
    ])
    putCalls = []

    mod.selectSession("s1")

    await vi.waitFor(() => {
      expect(putCalls).toHaveLength(1)
    })
    expect(putCalls[0]).toEqual({
      url: "/api/v1/sessions/s1/focused-tab",
      body: { tab_id: "t2" },
    })
  })

  it("selectSession landing on the plain session-slot tab does not PUT", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"], last_focused_tab: null },
    ])
    putCalls = []

    mod.selectSession("s1")

    expect(putCalls).toHaveLength(0)
  })
})
