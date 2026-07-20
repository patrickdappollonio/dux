import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"

// Phase 6 deep-linking: a tiny hash router mirrors the selected target into
// `location.hash` (#/agent/<id> | #/agent/<id>/terminal/<tid>). On load the hash
// is parsed and, once the first spine lands, the selection is restored (falling
// back to the agent when the terminal id is gone; ignoring the link when the
// session id is gone). On selection change the hash is rewritten with
// `history.replaceState` (never `pushState`, so the mobile back-stack is intact).

function makeSpine(
  sessions: {
    id: string
    project_id: string
    terminals?: string[]
    tabs?: string[]
  }[],
  projects: { id: string; terminals?: string[] }[] = [],
): Spine {
  return {
    projects: projects.map((p) => ({
      id: p.id,
      name: p.id,
      terminals: (p.terminals ?? []).map((id) => ({ id })),
    })) as unknown as Spine["projects"],
    sessions: sessions.map((s) => ({
      id: s.id,
      project_id: s.project_id,
      terminals: (s.terminals ?? []).map((id) => ({ id })),
      // The session-slot tab's id always equals the session id; any extra ids are
      // extra tabs.
      tabs: [{ id: s.id }, ...(s.tabs ?? []).map((id) => ({ id }))],
    })) as unknown as Spine["sessions"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])
let replaceStateMock: ReturnType<typeof vi.fn>

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

// Set the boot URL hash (parsed at module import) and load the store with the
// given spine, waiting for the first spine to land (which runs the deep-link
// restore).
async function loadStore(
  hash: string,
  sessions: { id: string; project_id: string; terminals?: string[] }[],
  projects: { id: string; terminals?: string[] }[] = [],
) {
  vi.stubGlobal("location", {
    protocol: "http:",
    host: "localhost:0",
    hash,
    pathname: "/",
    search: "",
  })
  spineBody = makeSpine(sessions, projects)
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

describe("deep-link restore on load", () => {
  it("restores an agent selection from #/agent/<id>", async () => {
    const mod = await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("advances the mobile shell to the terminal spoke on a resolved deep-link", async () => {
    // Otherwise the hub covers the deep-linked agent on mobile and its PTY never
    // mounts (desktop renders straight from selectedTarget and is unaffected).
    const mod = await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
  })

  it("leaves the mobile shell on home when the deep-link session is gone", async () => {
    const mod = await loadStore("#/agent/missing", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
  })

  it("restores an extra-tab selection from #/agent/<id>/tab/<tabId>", async () => {
    const mod = await loadStore("#/agent/s1/tab/t2", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t2",
    })
  })

  it("falls back to the session-slot tab when the extra tab id is gone", async () => {
    const mod = await loadStore("#/agent/s1/tab/gone", [
      { id: "s1", project_id: "p1", tabs: ["t-other"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("normalizes a self-aliased #/agent/<id>/tab/<id> to the session-slot tab", async () => {
    const mod = await loadStore("#/agent/s1/tab/s1", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("restores a terminal selection from #/agent/<id>/terminal/<tid>", async () => {
    const mod = await loadStore("#/agent/s1/terminal/t1", [
      { id: "s1", project_id: "p1", terminals: ["t1"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "session", sessionId: "s1" },
    })
  })

  it("restores a project terminal from #/project/<pid>/terminal/<tid> on boot", async () => {
    // The trap this guards (T11): the old grammar embedded a session id, so a
    // project-terminal bookmark was silently dropped on an ordinary page load.
    const mod = await loadStore(
      "#/project/p1/terminal/pt1",
      [],
      [{ id: "p1", terminals: ["pt1"] }],
    )
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "pt1",
      owner: { kind: "project", projectId: "p1" },
    })
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("leaves nothing selected when the project-terminal link's terminal is gone", async () => {
    // A vanished project terminal has no agent to fall back to.
    const mod = await loadStore(
      "#/project/p1/terminal/gone",
      [],
      [{ id: "p1", terminals: ["pt-other"] }],
    )
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("ignores a malformed project hash", async () => {
    const mod = await loadStore(
      "#/project/p1/terminal/",
      [],
      [{ id: "p1", terminals: ["pt1"] }],
    )
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("falls back to the agent when the terminal id is gone", async () => {
    const mod = await loadStore("#/agent/s1/terminal/gone", [
      { id: "s1", project_id: "p1", terminals: ["t-other"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("ignores the link when the session id is gone", async () => {
    const mod = await loadStore("#/agent/missing", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("a malformed percent-encoded hash does not crash the app at load", async () => {
    // `#/agent/%ZZ` makes `decodeURIComponent` throw a URIError. Parsing runs at
    // module init, so an unguarded throw would blank the whole app. The store
    // must import cleanly and treat the link as absent.
    const mod = await loadStore("#/agent/%ZZ", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("is a one-shot: a later spine refetch does not re-restore", async () => {
    const mod = await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    // The user navigates away, then a spine refetch arrives — it must NOT yank
    // the selection back to the boot deep-link.
    mod.selectSession(null)
    spineBody = makeSpine([{ id: "s1", project_id: "p1" }])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine?.sessions.length).toBe(1)
    })
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })
})

describe("selection writes the hash", () => {
  it("selecting an agent replaces the hash with #/agent/<id>", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    replaceStateMock.mockClear()
    mod.selectSession("s1")
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "#/agent/s1")
  })

  it("selecting an extra tab writes the /tab/ form; the session-slot tab stays bare", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    replaceStateMock.mockClear()
    mod.selectTab("s1", "t2")
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "#/agent/s1/tab/t2")
    replaceStateMock.mockClear()
    // Focusing the session-slot tab (tabId === sessionId) collapses back to the bare form.
    mod.selectTab("s1", "s1")
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "#/agent/s1")
  })

  it("selecting a terminal replaces the hash with the terminal form", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", terminals: ["t1"] },
    ])
    replaceStateMock.mockClear()
    mod.selectTerminal("t1", { kind: "session", sessionId: "s1" })
    expect(replaceStateMock).toHaveBeenCalledWith(
      null,
      "",
      "#/agent/s1/terminal/t1",
    )
  })

  it("selecting a project terminal writes the project hash form", async () => {
    const mod = await loadStore("", [], [{ id: "p1", terminals: ["pt1"] }])
    replaceStateMock.mockClear()
    mod.selectTerminal("pt1", { kind: "project", projectId: "p1" })
    expect(replaceStateMock).toHaveBeenCalledWith(
      null,
      "",
      "#/project/p1/terminal/pt1",
    )
  })

  it("clearing the selection collapses the hash to the bare path", async () => {
    const mod = await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    replaceStateMock.mockClear()
    mod.selectSession(null)
    expect(replaceStateMock).toHaveBeenCalledWith(null, "", "/")
  })
})
