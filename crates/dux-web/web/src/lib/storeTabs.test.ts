import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"

// Exercises the store's agent-tab lifecycle wiring (mirrors storeTerminals.test.ts):
// addTab POSTs the nested endpoint and focuses the returned tab; closeTab retargets
// a focused Support tab back to Main and DELETEs; retargetTab validates + PATCHes.
// The tabsApi wire behaviour itself is in tabsApi.test.ts.

function makeBootstrap(): Bootstrap {
  return {
    available_providers: ["claude", "codex", "opencode"],
    macros: [],
    palette_commands: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    global_env: {},
    status_clear_seconds: 6,
    agent_tabs_max: 20,
  }
}

// A session with a Main tab (id === s1) and one Support tab (b2).
function makeSpine() {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        project_id: "p1",
        terminals: [],
        tabs: [{ id: "s1" }, { id: "b2" }],
      },
    ],
    sidebar: { groups: [] },
  }
}

let spineBody: unknown = makeSpine()
let calls: [string, RequestInit | undefined][] = []

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  calls.push([u, init])
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => makeBootstrap(),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/spine")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => JSON.stringify(spineBody),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.endsWith("/tabs") && init?.method === "POST") {
    return {
      ok: true,
      status: 201,
      json: async () => ({ tab_id: "b9", provider: "codex" }),
      text: async () => JSON.stringify({ tab_id: "b9", provider: "codex" }),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/tabs/")) {
    return {
      ok: true,
      status: init?.method === "DELETE" ? 204 : 200,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => JSON.stringify({ rev: 1, staged: [], unstaged: [] }),
      headers: { get: () => null },
    } as unknown as Response
  }
  throw new Error(`unexpected fetch: ${u}`)
})

let sockets: FakeWebSocket[] = []

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  binaryType = ""
  readyState = 1
  constructor() {
    sockets.push(this)
  }
  close() {}
  send() {}
}

// Deliver a `sessions.changed` events-socket frame, which makes the store
// re-fetch the current `spineBody` and re-run applySpine.
function fireSessionsChanged() {
  sockets.at(-1)?.onmessage?.({
    data: JSON.stringify({ event: "sessions.changed" }),
  })
}

// A spine with a Main tab (s1) and one Support tab (b2) whose liveness varies.
function spineWithSupportTab(b2Live: boolean) {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        project_id: "p1",
        terminals: [],
        tabs: [
          { id: "s1", has_live_process: true },
          { id: "b2", has_live_process: b2Live },
        ],
      },
    ],
    sidebar: { groups: [] },
  }
}

beforeEach(() => {
  calls = []
  sockets = []
  spineBody = makeSpine()
  vi.stubGlobal("location", { host: "localhost:0", protocol: "http:" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    go: () => {},
    pushState: () => {},
    replaceState: () => {},
  })
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

const tick = () => new Promise((r) => setTimeout(r, 0))

function find(predicate: (url: string, init?: RequestInit) => boolean) {
  return calls.find(([u, init]) => predicate(u, init))
}

describe("store agent-tab lifecycle", () => {
  it("addTab POSTs the nested endpoint and focuses the returned tab", async () => {
    const mod = await loadStore()
    mod.addTab("s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "agent",
        sessionId: "s1",
        tabId: "b9",
      })
    })
    const post = find(
      (u, init) => u === "/api/v1/sessions/s1/tabs" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    // The in-flight guard cleared once the create resolved.
    expect(mod.getSnapshot().createTabInFlight).not.toContain("s1")
  })

  it("closeTab on the focused Support tab retargets to Main and DELETEs", async () => {
    const mod = await loadStore()
    mod.selectTab("s1", "b2")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "b2",
    })
    mod.closeTab("s1", "b2")
    await tick()
    // Selection snapped back to the Main tab immediately (not left on the dead id).
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    const del = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/tabs/b2" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("closeTab on the focused session-slot tab moves focus to a live sibling", async () => {
    // Closing the focused session-slot tab while a sibling is live must move
    // focus off it, so the pane doesn't re-subscribe the just-closed tab (which
    // would force-relaunch the provider).
    spineBody = spineWithSupportTab(true) // b2 is live
    const mod = await loadStore()
    mod.selectTab("s1", "s1") // focus the session-slot tab
    mod.closeTab("s1", "s1")
    await tick()
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "b2",
    })
    const del = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/tabs/s1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("closeTab on the focused session-slot tab with no live sibling leaves focus put", async () => {
    // No live sibling → the agent is detaching → selection stays on the
    // session-slot tab (there is nothing live to move to).
    spineBody = spineWithSupportTab(false) // b2 is dormant
    const mod = await loadStore()
    mod.selectTab("s1", "s1")
    mod.closeTab("s1", "s1")
    await tick()
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("retargetTab PATCHes the tab with a configured provider", async () => {
    const mod = await loadStore()
    const ok = await mod.retargetTab("s1", "b2", "opencode")
    expect(ok).toBe(true)
    const patch = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/tabs/b2" && init?.method === "PATCH",
    )
    expect(patch).toBeDefined()
    expect(JSON.parse((patch?.[1]?.body as string) ?? "{}")).toEqual({
      provider: "opencode",
    })
  })

  it("retargetTab rejects an unconfigured provider without a request", async () => {
    const mod = await loadStore()
    const ok = await mod.retargetTab("s1", "b2", "not-a-provider")
    expect(ok).toBe(false)
    expect(find((u) => u.includes("/tabs/"))).toBeUndefined()
  })

  it("clears the started-dormant latch once a tab is live, so a later exit re-shows the card", async () => {
    spineBody = spineWithSupportTab(false)
    const mod = await loadStore()
    // User clicks "Start fresh session" on the dormant Support tab.
    mod.startDormantTab("s1", "b2")
    expect(mod.getSnapshot().startedDormantTabs).toContain("b2")

    // The provider comes up: the latch is no longer needed and gets cleared.
    spineBody = spineWithSupportTab(true)
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
    })

    // The provider later exits: the tab is dormant again with no lingering latch,
    // so a plain refocus would show the card (not force-launch).
    spineBody = spineWithSupportTab(false)
    fireSessionsChanged()
    await vi.waitFor(() => {
      const tab = mod
        .getSnapshot()
        .spine?.sessions[0].tabs?.find((t) => t.id === "b2")
      expect(tab?.has_live_process).toBe(false)
    })
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
  })

  it("clears the started-dormant latch on a tab-launch-failure status, so the retry card returns", async () => {
    spineBody = spineWithSupportTab(false)
    const mod = await loadStore()
    // User clicks "Start fresh session"; the latch is set.
    mod.startDormantTab("s1", "b2")
    expect(mod.getSnapshot().startedDormantTabs).toContain("b2")

    // The fresh launch fails: the server sends a `tab-launch-<id>` keyed warning.
    // The latch clears so the dormant retry card reappears (no reconnect loop).
    sockets.at(-1)?.onmessage?.({
      data: JSON.stringify({
        event: "status",
        key: "tab-launch-b2",
        tone: "warning",
        message: 'Support tab launch failed for "s1": boom',
      }),
    })
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
  })

  it("keeps the started-dormant latch on a tab-launch-SUCCESS status, so the pane does not flash back to the card", async () => {
    spineBody = spineWithSupportTab(false)
    const mod = await loadStore()
    // User clicks "Start fresh session"; the latch is set and the pane mounts.
    mod.startDormantTab("s1", "b2")
    expect(mod.getSnapshot().startedDormantTabs).toContain("b2")

    // The fresh launch SUCCEEDS: the server sends a `tab-launch-<id>` keyed `info`
    // status (same key prefix as the failure, so it isn't clobbered). This must NOT
    // strip the latch — otherwise the tab would re-mark dormant before the spine's
    // `has_live_process` catches up, unmounting the just-launched pane. The latch is
    // instead cleared by `applySpine` once the spine reports the live process.
    sockets.at(-1)?.onmessage?.({
      data: JSON.stringify({
        event: "status",
        key: "tab-launch-b2",
        tone: "info",
        message: "Started a fresh codex tab.",
      }),
    })
    expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
  })

  it("normalizes a spine session that omits tabs to an empty array without throwing", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", terminals: [] }], // no `tabs` (old server)
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    expect(mod.getSnapshot().spine?.sessions[0].tabs).toEqual([])
  })
})
