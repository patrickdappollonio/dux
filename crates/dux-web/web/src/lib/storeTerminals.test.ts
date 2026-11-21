import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Loaded once at collection, where no test timeout applies and, with no window
// stubbed yet, the store boots nothing. Its first load is by far the slowest
// step in the file; left to the first test, a busy machine pushes that test
// past its timeout. Every test still imports its own fresh instance after
// `vi.resetModules`.
import "./store"

import type { Bootstrap } from "./bootstrapApi"

const warned = vi.hoisted(() => vi.fn())
vi.mock("./notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./notify")>()
  return {
    ...actual,
    notifyWarning: (...args: Parameters<typeof actual.notifyWarning>) => {
      warned(args[0])
      return actual.notifyWarning(...args)
    },
  }
})

// Exercises the store's companion-terminal lifecycle wiring: createTerminal
// POSTs the nested REST endpoint and focuses the returned terminal; deleteTerminal
// resolves the owning session from the spine and DELETEs the nested endpoint. The
// REST client's own wire behaviour is in terminalsApi.test.ts; here we assert the
// store calls it correctly and reacts (focus) to the result.
//
// The store fetches bootstrap + spine at import. We serve a configurable spine
// so deleteTerminal can resolve a terminal's owner.

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

// The spine served by the boot fetch. A session with one companion terminal so
// deleteTerminal can resolve its owner.
let spineBody: unknown = {
  projects: [{ id: "p1", name: "Repo" }],
  sessions: [{ id: "s1", project_id: "p1" }],
  terminals: [
    {
      id: "t1",
      label: "Terminal 1",
      owner: { kind: "session", session_id: "s1" },
    },
  ],
  sidebar: { groups: [] },
}

// Every fetch call, as [url, init], for assertions.
let calls: [string, RequestInit | undefined][] = []
// When set, the terminals reorder POST responds 400 so the overlay-clear-on-error
// path can be exercised.
let reorderFail = false
// A terminal DELETE to this exact URL is refused because somebody else is
// attached to it.
let attachedUrl: string | null = null
// A change sent to one of these URLs is refused as the server refuses a change
// to a row another device drives, with this sentence.
let drivenUrls = new Map<string, string>()
// When set, a terminal create waits for it before answering.
let createGate: Promise<void> | null = null
// What the server's live terminal list holds right now. The workspace document
// is a cache the server rebuilds a moment after a change, so the two can differ.
let liveTerminals: { id: string }[] = []

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  calls.push([u, init])
  if (u === "/api/v1/terminals/reorder") {
    return {
      ok: !reorderFail,
      status: reorderFail ? 400 : 204,
      text: async () => (reorderFail ? "nope" : ""),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => makeBootstrap(),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => JSON.stringify(spineBody),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u === "/api/v1/terminals" && (init?.method ?? "GET") === "GET") {
    return {
      ok: true,
      status: 200,
      json: async () => liveTerminals,
      text: async () => JSON.stringify(liveTerminals),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.endsWith("/terminals")) {
    // POST create.
    if (createGate) await createGate
    return {
      ok: true,
      status: 201,
      json: async () => ({ terminal_id: "t9", label: "Terminal 2" }),
      text: async () => JSON.stringify({ terminal_id: "t9", label: "Terminal 2" }),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (drivenUrls.has(u)) {
    return {
      ok: false,
      status: 423,
      text: async () => drivenUrls.get(u),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.endsWith("/kill")) {
    return {
      ok: true,
      status: 204,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u === attachedUrl) {
    const body = {
      error: "attached",
      blockers: [
        {
          surface: "browser",
          device: null,
          address: "10.0.0.7",
          verified: false,
          driving: false,
          target: { kind: "terminal", id: "t1" },
          key: "k1",
        },
      ],
    }
    return {
      ok: false,
      status: 409,
      text: async () => JSON.stringify(body),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/terminals/")) {
    // DELETE.
    return {
      ok: true,
      status: 204,
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
  drivenUrls = new Map()
  warned.mockClear()
  reorderFail = false
  liveTerminals = []
  spineBody = {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [{ id: "s1", project_id: "p1" }],
    terminals: [
      {
        id: "t1",
        label: "Terminal 1",
        owner: { kind: "session", session_id: "s1" },
      },
    ],
    sidebar: { groups: [] },
  }
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

// `vi.resetModules` gives the next test a fresh store but does not stop this
// one: its app socket would keep reconnecting on real timers, and every socket
// and request it makes would land on the next test's fakes.
afterEach(async () => {
  const store = await import("./store")
  store.eventsSocket.dispose()
  vi.unstubAllGlobals()
})

// Load the store and wait for the initial spine to settle.
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

describe("store companion-terminal lifecycle", () => {
  it("createTerminal POSTs the nested endpoint and focuses the new terminal", async () => {
    const mod = await loadStore()
    mod.createTerminal("s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "session", sessionId: "s1" },
      })
    })
    const post = find(
      (u, init) => u === "/api/v1/sessions/s1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
  })

  // A workspace document lands while the create is in flight and does not list
  // the new terminal; the server's cached document still does not when asked
  // again. Whether the terminal is opened rests on the live list alone.
  async function createWithStaleWorkspace(mod: Awaited<ReturnType<typeof loadStore>>) {
    let release!: () => void
    createGate = new Promise<void>((r) => {
      release = r
    })
    try {
      mod.createTerminal("s1")
      const workspaceFetches = () => calls.filter(([u]) => u.includes("/api/v1/workspace")).length
      const fetchesBefore = workspaceFetches()
      mod.eventsSocket.onEvent({ event: "sessions.changed" })
      await vi.waitFor(() => expect(workspaceFetches()).toBe(fetchesBefore + 1))
      await tick()
      release()
      for (let i = 0; i < 10; i++) await tick()
    } finally {
      createGate = null
    }
  }

  it("does not open a new terminal the server no longer lists when the create answers", async () => {
    const mod = await loadStore()
    const before = mod.getSnapshot().selectedTarget
    await createWithStaleWorkspace(mod)
    expect(mod.getSnapshot().selectedTarget).toEqual(before)
    // The answer came from the server's live list, not the stale workspace.
    expect(
      calls.some(([u, init]) => u === "/api/v1/terminals" && (init?.method ?? "GET") === "GET"),
    ).toBe(true)
  })

  it("opens a new terminal the server lists live even while its cached workspace does not yet", async () => {
    liveTerminals = [{ id: "t1" }, { id: "t9" }]
    const mod = await loadStore()
    await createWithStaleWorkspace(mod)
    await vi.waitFor(() =>
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "session", sessionId: "s1" },
      }),
    )
  })

  it("deleteTerminal resolves the owning session and DELETEs the nested endpoint", async () => {
    const mod = await loadStore()
    mod.deleteTerminal("t1")
    await tick()
    const del = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/terminals/t1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("deleteTerminal is a no-op for an unknown terminal (no owner in the spine)", async () => {
    const mod = await loadStore()
    mod.deleteTerminal("does-not-exist")
    await tick()
    const del = find((u, init) => init?.method === "DELETE")
    expect(del).toBeUndefined()
  })

  it("createProjectTerminal POSTs the project endpoint and focuses with a project owner", async () => {
    const mod = await loadStore()
    mod.createProjectTerminal("p1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "project", projectId: "p1" },
      })
    })
    const post = find(
      (u, init) => u === "/api/v1/projects/p1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    // A project terminal has no session context.
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("deleteTerminal routes a PROJECT-owned terminal to the project endpoint", async () => {
    // An owner scan that only walks sessions resolves nothing for a project
    // terminal, so Close would silently do nothing.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [],
      terminals: [
        {
          id: "pt1",
          label: "Terminal 1",
          owner: { kind: "project", project_id: "p1" },
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.deleteTerminal("pt1")
    await tick()
    const del = find(
      (u, init) =>
        u === "/api/v1/projects/p1/terminals/pt1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("createStandaloneTerminal POSTs the un-nested endpoint and focuses it", async () => {
    const mod = await loadStore()
    mod.createStandaloneTerminal()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "standalone" },
      })
    })
    // No owner id anywhere in the address: that is the whole shape of it.
    const post = find(
      (u, init) => u === "/api/v1/terminals" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("deleteTerminal routes a STANDALONE terminal to the un-nested endpoint", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "solo1",
          label: "Terminal 1",
          owner: { kind: "standalone", cwd_label: "~" },
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.deleteTerminal("solo1")
    await tick()
    const del = find(
      (u, init) => u === "/api/v1/terminals/solo1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  it("stopAllRunning deletes project terminals too", async () => {
    // "Force stop everything" must cover project terminals too, not only sessions'
    // terminals.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t1",
          label: "Terminal 1",
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "pt1",
          label: "Terminal 1",
          owner: { kind: "project", project_id: "p1" },
        },
      ],
      sidebar: { groups: [] },
    }
    // Somebody else is attached to the session's terminal: that one is
    // refused and named, the project's goes.
    attachedUrl = "/api/v1/sessions/s1/terminals/t1"
    const mod = await loadStore()
    const refused = await mod.stopAllRunning()
    attachedUrl = null
    const sessionDel = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/terminals/t1" && init?.method === "DELETE",
    )
    const projectDel = find(
      (u, init) =>
        u === "/api/v1/projects/p1/terminals/pt1" && init?.method === "DELETE",
    )
    expect(sessionDel).toBeDefined()
    expect(projectDel).toBeDefined()
    expect(refused?.agents).toEqual([])
    expect(refused?.terminals).toEqual(["t1"])
    expect(refused?.blockers.map((b) => b.key)).toEqual(["k1"])

    // Going ahead stops only what was refused, over exactly who was shown.
    calls = []
    expect(await mod.stopAllRunning(refused, ["k1"])).toBeNull()
    expect(
      calls
        .filter(([, init]) => init?.method === "DELETE" || init?.method === "POST")
        .map(([u]) => u),
    ).toEqual(["/api/v1/sessions/s1/terminals/t1?force_connected=k1"])
  })

  it("stopAllRunning leaves alone the rows another device drives, and counts only the rest", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: "agentx",
          status: "active",
          tabs: [{ id: "s1", input_owner: "conn-other" }],
        },
        { id: "s2", project_id: "p1", title: "agenty", status: "active", tabs: [{ id: "s2" }] },
      ],
      terminals: [
        { id: "t1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
        { id: "pt1", label: "Terminal 2", owner: { kind: "project", project_id: "p1" } },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().spine).not.toBeNull())
    const plan = mod.stopAllPlan(mod.getSnapshot())
    expect(plan.agents).toEqual(["s2"])
    expect(plan.terminals).toEqual(["pt1"])
    expect(plan.left).toEqual([
      {
        kind: "agent",
        id: "s1",
        name: "agentx",
        terminals: 1,
        terminalsOnly: false,
        device: null,
        block: "other",
      },
    ])

    calls = []
    await mod.stopAllRunning()
    const sent = calls
      .filter(([, init]) => init?.method === "DELETE" || init?.method === "POST")
      .map(([u]) => u)
    expect(sent).toEqual(["/api/v1/sessions/s2/kill", "/api/v1/projects/p1/terminals/pt1"])
  })

  // The bulk stop asks each row the question its per-row Force stop asks, so a
  // row nobody drives whose card is up here, or one this device is still
  // connecting to, is left running and named with why.
  it("stopAllRunning leaves every row this device may not change, saying why", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        { id: "s1", project_id: "p1", title: "agentx", status: "active", tabs: [{ id: "s1" }] },
      ],
      terminals: [
        { id: "pt1", label: "Terminal 2", owner: { kind: "project", project_id: "p1" } },
        { id: "t9", label: "Terminal 3", owner: { kind: "standalone" } },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    const { agentRowKey, registerPaneCover } = await import("./paneCover")
    registerPaneCover("pt1", "card", { row: "terminal:pt1", driver: "nobody" })
    registerPaneCover("s1", "spinner", { row: agentRowKey("s1"), driver: "pending" })
    const plan = mod.stopAllPlan(mod.getSnapshot())
    expect(plan.agents).toEqual([])
    expect(plan.terminals).toEqual(["t9"])
    expect(plan.left.map((part) => [part.id, part.block])).toEqual([
      ["s1", "pending"],
      ["pt1", "nobody"],
    ])
    expect(plan.left.map((part) => mod.partChangeBlock(mod.getSnapshot(),
      part.kind === "agent" ? { sessionId: part.id } : { terminalId: part.id }))).toEqual([
      "pending",
      "nobody",
    ])
    const { proseText } = await import("./prose")
    expect(proseText(mod.leftRunningProse(plan.left))).toBe(
      "Left running, because this device is still connecting to it: agent agentx. " +
        "It can be stopped once this device hears who drives it. " +
        "Left running, because nobody drives it, this device included: terminal Terminal 2. " +
        "Take it over on this device first to stop it.",
    )

    calls = []
    await mod.stopAllRunning()
    const sent = calls
      .filter(([, init]) => init?.method === "DELETE" || init?.method === "POST")
      .map(([u]) => u)
    expect(sent).toEqual(["/api/v1/terminals/t9"])
  })

  const DRIVEN =
    "This agent is being driven from Firefox on Linux right now, and only the device driving it can change it, so dux did not make the change. Take it over on this device first, then try again."

  async function stopAllSaid(mod: Awaited<ReturnType<typeof loadStore>>): Promise<string> {
    const { proseText } = await import("./prose")
    await mod.stopAllRunning()
    expect(warned).toHaveBeenCalledOnce()
    return proseText(warned.mock.calls[0][0])
  }

  it("stopAllRunning says what the server said of a row it refused", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        { id: "s1", project_id: "p1", title: "agentx", status: "active", tabs: [{ id: "a1" }] },
      ],
      terminals: [
        { id: "t1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
      ],
      sidebar: { groups: [] },
    }
    drivenUrls.set("/api/v1/sessions/s1/kill", DRIVEN)
    const mod = await loadStore()
    expect(await stopAllSaid(mod)).toBe(
      `Nothing was stopped. Left running: agent agentx with its 1 terminal. ${DRIVEN}`,
    )
  })

  it("stopAllRunning counts a stopped agent once and leaves only its refused terminals", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        { id: "s1", project_id: "p1", title: "agentx", status: "active", tabs: [{ id: "a1" }] },
      ],
      terminals: [
        { id: "t1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
        { id: "t2", label: "Terminal 2", owner: { kind: "session", session_id: "s1" } },
      ],
      sidebar: { groups: [] },
    }
    drivenUrls.set("/api/v1/sessions/s1/terminals/t1", DRIVEN)
    const mod = await loadStore()
    expect(await stopAllSaid(mod)).toBe(
      `Stopped 1 agent. Left running: 2 terminals of agent agentx. ${DRIVEN}`,
    )
  })

  it("stopAllRunning words an idle agent's refused terminals as the agent's", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        { id: "s1", project_id: "p1", title: "agentx", status: "detached", tabs: [{ id: "a1" }] },
      ],
      terminals: [
        { id: "t1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
      ],
      sidebar: { groups: [] },
    }
    drivenUrls.set("/api/v1/sessions/s1/terminals/t1", DRIVEN)
    const mod = await loadStore()
    expect(await stopAllSaid(mod)).toBe(
      `Nothing was stopped. Left running: 1 terminal of agent agentx. ${DRIVEN}`,
    )
  })

  it("stopAllPlan words an idle agent another device drives by its running terminals", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: "agentx",
          status: "detached",
          tabs: [{ id: "a1", input_owner: "conn-other" }],
        },
      ],
      terminals: [
        { id: "t1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().spine).not.toBeNull())
    const { proseText } = await import("./prose")
    expect(proseText(mod.leftRunningProse(mod.stopAllPlan(mod.getSnapshot()).left))).toBe(
      "Left running, because another device drives it: 1 terminal of agent agentx. " +
        "Take it over on this device first to stop it.",
    )
  })

  it("stopAllRunning says a sentence the server gave several rows once, naming them all", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        { id: "s1", project_id: "p1", title: "agentx", status: "active", tabs: [{ id: "a1" }] },
        { id: "s2", project_id: "p1", title: "agenty", status: "active", tabs: [{ id: "a2" }] },
      ],
      terminals: [],
      sidebar: { groups: [] },
    }
    drivenUrls.set("/api/v1/sessions/s1/kill", DRIVEN)
    drivenUrls.set("/api/v1/sessions/s2/kill", DRIVEN)
    const mod = await loadStore()
    expect(await stopAllSaid(mod)).toBe(
      `Nothing was stopped. Left running: agent agentx and agent agenty. ${DRIVEN}`,
    )
  })

  it("names the device driving a row no pane here shows, from the workspace document", async () => {
    const ua = "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0"
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: "agentx",
          status: "active",
          tabs: [{ id: "a1", input_owner: "conn-other", input_owner_device: ua }],
        },
      ],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().spine).not.toBeNull())
    expect(mod.stopAllPlan(mod.getSnapshot()).left[0]?.device).toBe("Firefox on Linux")
  })

  it("never names this device as the driver of a row it is reconnecting to", async () => {
    const ua = "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0"
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: "agentx",
          status: "active",
          tabs: [{ id: "a1", input_owner: "conn-mine", input_owner_device: ua }],
        },
      ],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().spine).not.toBeNull())
    const { agentRowKey, registerPaneCover } = await import("./paneCover")
    registerPaneCover("a1", "spinner", { row: agentRowKey("s1"), driver: "reconnecting" })
    const { notifyPtyOwner } = await import("./ptyOwnership")
    notifyPtyOwner("a1", "conn-mine", 7, ua)
    const { proseText } = await import("./prose")
    expect(proseText(mod.leftRunningProse(mod.stopAllPlan(mod.getSnapshot()).left))).toBe(
      "Left running, because this device is reconnecting to it: agent agentx. " +
        "It can be stopped once this device hears who drives it.",
    )
  })

  // The panic button is the ONE caller that skips the shutdown grace. A polite
  // wait per agent is the opposite of what somebody pressing "Stop everything"
  // is asking for, so the flag has to be on this path and off every other one.
  it("stopAllRunning forces the agent stop rather than waiting out the grace", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status: "active" }],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.stopAllRunning()
    await tick()
    const kill = find(
      (u, init) => u === "/api/v1/sessions/s1/kill" && init?.method === "POST",
    )
    expect(kill).toBeDefined()
    expect(JSON.parse(String(kill?.[1]?.body))).toEqual({ force: true })
  })

  it("the per-agent detach asks for the grace instead of forcing", async () => {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status: "active" }],
      terminals: [],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.killSessionPty("s1")
    await tick()
    const kill = find(
      (u, init) => u === "/api/v1/sessions/s1/kill" && init?.method === "POST",
    )
    expect(JSON.parse(String(kill?.[1]?.body))).toEqual({ force: false })
  })
})

describe("store terminal reorder overlay", () => {
  it("reorderTerminals sets the optimistic overlay and POSTs the full id order", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t3", "t1", "t2"])
    // The overlay is applied synchronously so the UI reflects the drop instantly.
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t3", "t1", "t2"])
    await tick()
    const post = find(
      (u, init) => u === "/api/v1/terminals/reorder" && init?.method === "POST",
    )
    expect(post).toBeDefined()
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      terminal_ids: ["t3", "t1", "t2"],
    })
  })

  it("clears the overlay once a spine confirms the order (reconcile)", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
    await tick()
    // A spine whose terminals' global sort_order matches the dragged order: t2
    // before t1. The reconcile sorts every terminal by sort_order and clears the
    // overlay when that matches what we optimistically applied.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t2",
          label: "Terminal 2",
          sort_order: 0,
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "t1",
          label: "Terminal 1",
          sort_order: 1,
          owner: { kind: "session", session_id: "s1" },
        },
      ],
      sidebar: { groups: [] },
    }
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
    })
  })

  it("keeps the overlay when the spine order does not yet match", async () => {
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    await tick()
    // Spine still reflects the OLD order (t1 before t2): overlay must persist.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      terminals: [
        {
          id: "t1",
          label: "Terminal 1",
          sort_order: 0,
          owner: { kind: "session", session_id: "s1" },
        },
        {
          id: "t2",
          label: "Terminal 2",
          sort_order: 1,
          owner: { kind: "session", session_id: "s1" },
        },
      ],
      sidebar: { groups: [] },
    }
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine?.terminals).toHaveLength(2)
    })
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
  })

  it("clears the overlay and does not throw when the reorder POST fails", async () => {
    reorderFail = true
    const mod = await loadStore()
    mod.reorderTerminals(["t2", "t1"])
    expect(mod.getSnapshot().pendingTerminalOrder).toEqual(["t2", "t1"])
    // The rejected POST clears the overlay so the UI snaps back to server order.
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingTerminalOrder).toBeNull()
    })
  })
})

// An agent's stack: which layout it opens in when one of its terminals is
// brought to the front, how focus moves between its parts, and how its dock
// reorders.
describe("store agent stack", () => {
  function withSessionStatus(status: string, terminals: unknown[] = []) {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ id: "s1", project_id: "p1", status }],
      terminals,
      sidebar: { groups: [] },
    }
  }
  const companion = (id: string, sortOrder: number) => ({
    id,
    label: id,
    owner: { kind: "session", session_id: "s1" },
    sort_order: sortOrder,
  })

  it("opens a stopped agent's new terminal maximized, so nothing starts the agent", async () => {
    withSessionStatus("detached")
    const mod = await loadStore()
    mod.createTerminal("s1")
    await vi.waitFor(() => expect(mod.getSnapshot().stackLayouts.s1).toBe("terminal"))
  })

  it("opens at least the split when a terminal is brought to the front of a folded dock", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.setStackLayout("s1", "agent")
    mod.selectTerminal("t1", { kind: "session", sessionId: "s1" })
    expect(mod.getSnapshot().stackLayouts.s1).toBe("split")
    expect(mod.getSnapshot().stackTerminals.s1).toBe("t1")
  })

  it("judges every arrival at an agent afresh", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.setStackLayout("s1", "terminal")
    mod.selectSession(null)
    mod.selectSession("s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBeUndefined()
  })

  it("moves typing between the parts on screen without a new history entry", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const pushState = vi.fn()
    const replaceState = vi.fn()
    vi.stubGlobal("history", { go: () => {}, pushState, replaceState, state: null })
    const mod = await loadStore()
    mod.selectSession("s1")
    const pushesBefore = pushState.mock.calls.length
    mod.chooseStackPart("s1", "t1")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "session", sessionId: "s1" },
    })
    const slot = mod.getSnapshot().spine!.sessions[0].slot_tab_id
    mod.chooseStackPart("s1", slot)
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: slot,
    })
    expect(pushState.mock.calls.length).toBe(pushesBefore)
    expect(replaceState).toHaveBeenCalled()
  })

  it("pushes for a new terminal, which is a new position", async () => {
    withSessionStatus("active", [companion("t1", 1)])
    const pushState = vi.fn()
    vi.stubGlobal("history", { go: () => {}, pushState, replaceState: vi.fn(), state: null })
    const mod = await loadStore()
    mod.selectSession("s1")
    const before = pushState.mock.calls.length
    mod.createTerminal("s1")
    await vi.waitFor(() => expect(pushState.mock.calls.length).toBe(before + 1))
  })

  it("leaves the list's sort mode alone when the dock is reordered", async () => {
    withSessionStatus("active", [companion("t1", 1), companion("t2", 2)])
    const mod = await loadStore()
    const sortBefore = mod.agentSortValue(mod.getSnapshot())
    mod.reorderDock("s1", ["t2", "t1"])
    await tick()
    expect(mod.agentSortValue(mod.getSnapshot())).toBe(sortBefore)
    expect(calls.some(([u]) => u.includes("/config"))).toBe(false)
  })

  it("puts a started agent back in the layout this device last chose, or the split", async () => {
    withSessionStatus("detached", [companion("t1", 1)])
    const mem = new Map<string, string>([["dux:stack:s1", JSON.stringify({ layout: "agent" })]])
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: (k: string, v: string) => void mem.set(k, v),
      removeItem: (k: string) => void mem.delete(k),
    })
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.startStoppedAgent("s1", "s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBe("agent")
    mem.clear()
    mod.startStoppedAgent("s1", "s1")
    expect(mod.getSnapshot().stackLayouts.s1).toBe("split")
  })

  it("keeps the tab shown while typing moves to the dock", async () => {
    withSessionStatus("detached", [companion("t1", 1)])
    const mod = await loadStore()
    mod.selectSession("s1")
    const slot = mod.getSnapshot().spine!.sessions[0].slot_tab_id
    mod.chooseStackPart("s1", "t1")
    mod.selectTerminal("t1", { kind: "session", sessionId: "s1" })
    expect(mod.getSnapshot().stackTabs.s1).toBe(slot)
  })

  it("holds a dock's reorder for a take-over of its own row, never another row's", async () => {
    withSessionStatus("active", [companion("t1", 1), companion("t2", 2)])
    const mod = await loadStore()
    const flight = await import("./takeoverFlight")
    const sent = () => calls.filter(([u]) => u === "/api/v1/terminals/reorder").length
    const other = flight.beginTakeoverFlight("agent:s2")
    mod.reorderDock("s1", ["t2", "t1"])
    await tick()
    expect(sent()).toBe(1)
    other(false)

    const own = flight.beginTakeoverFlight("agent:s1")
    mod.reorderDock("s1", ["t1", "t2"])
    await tick()
    expect(sent()).toBe(1)
    own(true)
    await vi.waitFor(() => expect(sent()).toBe(2))
  })

  it("reorders one dock among the places its terminals hold, leaving every other terminal put", async () => {
    withSessionStatus("active", [
      companion("t1", 1),
      { id: "p-term", label: "p", owner: { kind: "project", project_id: "p1" }, sort_order: 2 },
      companion("t2", 3),
    ])
    const mod = await loadStore()
    mod.reorderDock("s1", ["t2", "t1"])
    await tick()
    const post = find((u, init) => u === "/api/v1/terminals/reorder" && init?.method === "POST")
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      terminal_ids: ["t2", "p-term", "t1"],
    })
  })
})
