import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"
import { ownerKey } from "./terminalOwner"

// Routing: the URL is the source of truth for where the app is. The screen is
// DERIVED from the hash (no target = home, a target = terminal, a `/changes`
// suffix = changes), moving in PUSHES a hash entry, and Back is the browser's
// own Back. Nothing counts entries and nothing steps history relatively.
//
// These tests drive a fake history that models the real thing: entry 0 is the
// page the browser showed BEFORE dux (a new-tab page), entry 1 is dux itself.
// Stepping below entry 1 means the user was thrown out of the app, which is the
// bug this suite exists to pin.

const OUTSIDE_ENTRY = "about:newtab"
const DUX_ENTRY_INDEX = 1

let entries: string[]
let index: number
let leftApp: boolean
// Armed to make the next history WRITE fail, the way Safari does when a page
// exceeds its rate limit on history calls.
let historyWriteError: Error | null = null
let popstateListeners: (() => void)[]
let loc: {
  protocol: string
  host: string
  hash: string
  pathname: string
  search: string
}

// Mirror the entry the fake history is parked on into the fake `location`, the
// way a browser does before it fires popstate.
function applyCurrentEntry(): void {
  const url = entries[index] ?? "/"
  loc.hash = url.startsWith("#") ? url : ""
}

const fakeHistory = {
  state: null as unknown,
  pushState(state: unknown, _title: string, url: string) {
    if (historyWriteError) throw historyWriteError
    entries = entries.slice(0, index + 1)
    entries.push(String(url))
    index = entries.length - 1
    fakeHistory.state = state
    applyCurrentEntry()
  },
  replaceState(state: unknown, _title: string, url: string) {
    if (historyWriteError) throw historyWriteError
    entries[index] = String(url)
    fakeHistory.state = state
    applyCurrentEntry()
  },
  go(delta: number) {
    const target = index + delta
    if (target < DUX_ENTRY_INDEX) {
      // The browser would happily leave dux for whatever preceded it.
      leftApp = true
      return
    }
    // A real browser ignores a Forward past the newest entry.
    if (target >= entries.length) return
    index = target
    applyCurrentEntry()
    for (const listener of popstateListeners) listener()
  },
  back() {
    fakeHistory.go(-1)
  },
  forward() {
    fakeHistory.go(1)
  },
}

// A session descriptor for `makeSpine`. `tabs` lists the EXTRA tabs; the
// session-slot tab (`tabId === sessionId`) is always present and is added here
// so a caller never has to remember it.
interface SessionSpec {
  id: string
  project_id: string
  status?: string
  tabs?: string[]
  terminals?: string[]
  // The default "active" sort floats a working agent above the rest, so this is
  // what lets a test tell the sort apart from the raw server order.
  working?: boolean
}

function makeSpine(
  sessions: SessionSpec[],
  projects: { id: string; terminals?: string[] }[] = [],
  // Terminals owned by nothing. They hang off no session and no project, which
  // is exactly why they need their own argument here.
  standaloneTerminals: string[] = [],
): Spine {
  return {
    projects: projects.map((p) => ({
      id: p.id,
      name: p.id,
      path: `/tmp/${p.id}`,
    })) as unknown as Spine["projects"],
    sessions: sessions.map((s) => ({
      id: s.id,
      project_id: s.project_id,
      status: s.status ?? "active",
      title: s.id,
      branch_name: s.id,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      working: s.working ?? false,
      needs_attention: false,
      tabs: [{ id: s.id }, ...(s.tabs ?? []).map((id) => ({ id }))],
    })) as unknown as Spine["sessions"],
    // Every terminal, of every owner, in one flat owner-tagged collection: the
    // sessions' terminals in session order, then the projects' in project order.
    terminals: [
      ...sessions.flatMap((s) =>
        (s.terminals ?? []).map((id) => ({
          id,
          label: id,
          owner: { kind: "session", session_id: s.id },
        })),
      ),
      ...projects.flatMap((p) =>
        (p.terminals ?? []).map((id) => ({
          id,
          label: id,
          owner: { kind: "project", project_id: p.id },
        })),
      ),
      ...standaloneTerminals.map((id) => ({
        id,
        label: id,
        owner: { kind: "standalone", cwd_label: "~/code" },
      })),
    ] as unknown as Spine["terminals"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])

// Holds the spine fetch open so a popstate can arrive while `state.spine` is
// still null: a slow spine fetch, or a session/bfcache restore that comes back
// with a back stack already. `releaseSpine` lets the response through.
let spineGate: Promise<void> | null = null
let releaseSpine: () => void = () => {}

function holdSpine(): void {
  spineGate = new Promise<void>((resolve) => {
    releaseSpine = () => {
      spineGate = null
      resolve()
    }
  })
}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/spine")) {
    if (spineGate) await spineGate
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
  spineGate = null
  releaseSpine = () => {}
  leftApp = false
  historyWriteError = null
  popstateListeners = []
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", {
    addEventListener: (type: string, handler: () => void) => {
      if (type === "popstate") popstateListeners.push(handler)
    },
  })
  vi.stubGlobal("history", fakeHistory)
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// Boot the store with `hash` as the URL the browser landed on and `sessions` as
// the first spine. The fake history starts parked on dux's own entry, one step
// above the page that preceded it.
async function loadStore(
  hash: string,
  sessions: SessionSpec[],
  projects: { id: string; terminals?: string[] }[] = [],
  standaloneTerminals: string[] = [],
) {
  entries = [OUTSIDE_ENTRY, hash === "" ? "/" : hash]
  index = DUX_ENTRY_INDEX
  loc = {
    protocol: "http:",
    host: "localhost:0",
    hash,
    pathname: "/",
    search: "",
  }
  vi.stubGlobal("location", loc)
  spineBody = makeSpine(sessions, projects, standaloneTerminals)
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Boot the store WITHOUT waiting for the spine: the fetch is held open by
// `holdSpine`, so the store comes up with `state.spine` still null and a
// popstate can land in that window.
async function loadStoreWithHeldSpine(hash: string, sessions: SessionSpec[]) {
  entries = [OUTSIDE_ENTRY, hash === "" ? "/" : hash]
  index = DUX_ENTRY_INDEX
  loc = {
    protocol: "http:",
    host: "localhost:0",
    hash,
    pathname: "/",
    search: "",
  }
  vi.stubGlobal("location", loc)
  spineBody = makeSpine(sessions)
  holdSpine()
  const mod = await import("./store")
  // The popstate listener registers at module scope, so it is live the moment
  // the import resolves, spine or no spine.
  await vi.waitFor(() => {
    expect(popstateListeners.length).toBeGreaterThan(0)
  })
  expect(mod.getSnapshot().spine).toBeNull()
  return mod
}

// Model a real popstate: the browser moves its own cursor onto another entry
// and THEN fires the event. Written directly rather than through
// `history.pushState`, because a push is the app's own write and fires nothing.
function popstateTo(url: string): void {
  entries = entries.slice(0, index + 1)
  entries.push(url)
  index = entries.length - 1
  applyCurrentEntry()
  for (const listener of popstateListeners) listener()
}

// Push a new spine through the events socket, as the server does when the
// session list changes (a delete, a create).
async function pushSpine(
  mod: Awaited<ReturnType<typeof loadStore>>,
  sessions: SessionSpec[],
  projects: { id: string; terminals?: string[] }[] = [],
  standaloneTerminals: string[] = [],
) {
  spineBody = makeSpine(sessions, projects, standaloneTerminals)
  mod.eventsSocket.onEvent({ event: "sessions.changed" })
  // Wait on the ids, not just the session count: a push that only closes a
  // project terminal or one tab leaves the count untouched, and a count-only
  // wait would return before the apply had run. Terminals are one flat,
  // owner-tagged collection now, so their identity here is id + owner.
  const shape = (spine: Spine | null) =>
    JSON.stringify({
      sessions: spine?.sessions.map((s) => [s.id, s.tabs.map((t) => t.id)]) ?? [],
      terminals: spine?.terminals.map((t) => [t.id, ownerKey(t.owner)]) ?? [],
    })
  const want = shape(spineBody)
  await vi.waitFor(() => {
    expect(shape(mod.getSnapshot().spine)).toBe(want)
  })
}

describe("a deleted agent never throws the user out of dux", () => {
  it("keeps the user inside the app after a deep-linked agent is deleted", async () => {
    // The reported bug: arriving on `#/agent/s1` sets the terminal screen
    // without pushing an entry, so collapsing the screen by stepping back
    // walked past dux onto the browser's new-tab page.
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    expect(leftApp).toBe(false)
    expect(index).toBe(DUX_ENTRY_INDEX)
  })

  it("lands on the next active agent when the focused one is deleted", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    expect(loc.hash).toBe("#/agent/s2")
  })

  it("skips an agent that detached in the same push that deleted the focused one", async () => {
    // The neighbour must be detached only in the NEW spine, never in the old
    // one. Starting it detached proved nothing: `nextActiveSessionId` walks the
    // PREVIOUS list, which is itself filtered to active agents, so an
    // already-detached neighbour never reaches the candidate check at all and
    // the test passed with that check removed. Here s2 is active while the walk
    // is built and dormant by the time it is consulted, which is the only shape
    // that exercises it.
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
      { id: "s3", project_id: "p1" },
    ])
    await pushSpine(mod, [
      { id: "s2", project_id: "p1", status: "detached" },
      { id: "s3", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s3")
  })

  it("picks the destination by the sort the user is looking at", async () => {
    // Every other store-level vanish test boots with the default sort and no
    // agent working, where every sort key produces the same order, so replacing
    // the sort lookup with a hardcoded "manual" left them all green. s2 is
    // working, so the default "active" sort floats it above s1 and the next row
    // after s1 on screen is s3, not the s2 that raw server order would give.
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1", working: true },
      { id: "s3", project_id: "p1" },
    ])
    await pushSpine(mod, [
      { id: "s2", project_id: "p1", working: true },
      { id: "s3", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s3")
  })

  it("follows an in-flight drag order, not the order the server still reports", async () => {
    // A drag is applied optimistically and only retired once the server echoes
    // it back. Until then the list on screen is the overlay's order, so the
    // destination has to be read off the overlay too, or the user lands on a row
    // that is not the one below the agent that vanished.
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
      { id: "s3", project_id: "p1" },
    ])
    mod.setAgentSort("manual")
    mod.reorderAgents(["s1", "s3", "s2"])
    // The server has not confirmed the drag yet: its order is still s2 then s3.
    await pushSpine(mod, [
      { id: "s2", project_id: "p1" },
      { id: "s3", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s3")
  })

  it("falls back to the owning agent when a companion terminal closes", async () => {
    // A session-owned terminal's PTY exits while the user is watching it. Its
    // agent is alive one level up, so that is the destination: ejecting all the
    // way to home threw away a position that still exists. The deep-link path
    // already did this; the prune path used to disagree with it.
    const mod = await loadStore("#/agent/s1/terminal/t1", [
      { id: "s1", project_id: "p1", terminals: ["t1"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "session", sessionId: "s1" },
    })
    const depth = index
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    // Out loud: the address bar must stop naming the dead terminal.
    expect(loc.hash).toBe("#/agent/s1")
    expect(index).toBe(depth)
  })

  it("goes home when a PROJECT terminal closes under the user", async () => {
    // A project terminal has no agent above it, so home is the only honest
    // destination.
    const mod = await loadStore(
      "#/project/p1/terminal/t1",
      [{ id: "s1", project_id: "p1" }],
      [{ id: "p1", terminals: ["t1"] }],
    )
    expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }], [{ id: "p1", terminals: [] }])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
  })

  it("goes home when a STANDALONE terminal closes under the user", async () => {
    // It has neither an agent nor a project above it, so home is the only
    // honest destination, and the address bar must say so rather than going on
    // naming a terminal that is gone.
    const mod = await loadStore("#/terminal/solo-1", [], [], ["solo-1"])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "solo-1",
      owner: { kind: "standalone" },
    })
    await pushSpine(mod, [], [], [])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
  })

  it("goes home when no active agent is left", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1", status: "detached" },
    ])
    await pushSpine(mod, [{ id: "s2", project_id: "p1", status: "detached" }])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(leftApp).toBe(false)
    expect(loc.hash).toBe("")
    // The world changed under the user, so this is a REWRITE of the entry they
    // were parked on, never a new position they can Back out of.
    expect(index).toBe(DUX_ENTRY_INDEX)
  })

  it("rewrites the entry rather than burying it under a new one", async () => {
    // The position assertion the address-only test could not make: leaving the
    // changes screen for another agent's terminal screen is a SCREEN change, so
    // a destination that forgot to replace would push here.
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s1")
    mod.openChangesScreen()
    const depth = index
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    expect(loc.hash).toBe("#/agent/s2")
    expect(index).toBe(depth)
    expect(entries).not.toContain("#/agent/s1/changes")
  })

  it("rewrites the entry when the focused TAB vanishes under the user", async () => {
    // Another device closes the tab you are on while you are reading its
    // changes. The fall back to the parent agent is not a move the user made,
    // so it must not leave an entry behind for them to Back into.
    const mod = await loadStore("#/agent/s1/tab/t2/changes", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    const depth = index
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    // Only the invalid part of the route is corrected: the tab is gone, but
    // changed files are session-scoped, so the screen being read survives.
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/changes")
    expect(index).toBe(depth)
    expect(entries).not.toContain("#/agent/s1/tab/t2/changes")
  })
})

// A popstate can arrive before the first spine lands. The handler used to
// return early in that window, on the claim that the boot deep-link restore
// would resolve the hash later. It does not: the boot restore resolves the BOOT
// hash, not the address the browser has since moved to.
describe("a popstate that beats the first spine is not dropped", () => {
  it("adopts the agent the browser moved to while the spine was still loading", async () => {
    const mod = await loadStoreWithHeldSpine("", [{ id: "s1", project_id: "p1" }])
    popstateTo("#/agent/s1")
    releaseSpine()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    // The address bar named s1 the whole time; the app has to be showing it.
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    expect(loc.hash).toBe("#/agent/s1")
  })

  it("does not undo a Back to home that arrived before the spine", async () => {
    const mod = await loadStoreWithHeldSpine("#/agent/s1", [
      { id: "s1", project_id: "p1" },
    ])
    popstateTo("/")
    releaseSpine()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    // The boot link named s1, but the user has since left it. Restoring the
    // boot hash here would both yank them back and overwrite the entry they
    // landed on.
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
    expect(entries[index]).toBe("/")
  })

  it("keeps the changes screen a pre-spine popstate named", async () => {
    const mod = await loadStoreWithHeldSpine("", [{ id: "s1", project_id: "p1" }])
    popstateTo("#/agent/s1/changes")
    releaseSpine()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/changes")
  })
})

describe("moving in pushes, and Back unwinds exactly", () => {
  it("returns to the starting history position after going in and back out", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    mod.selectSession("s1")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    mod.openChangesScreen()
    expect(mod.getSnapshot().mobileScreen).toBe("changes")

    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(index).toBe(DUX_ENTRY_INDEX)
    expect(leftApp).toBe(false)
  })

  it("does not accumulate entries when switching between agents", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s1")
    const afterFirst = index
    mod.selectSession("s2")
    mod.selectSession("s1")
    expect(index).toBe(afterFirst)
    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(index).toBe(DUX_ENTRY_INDEX)
  })

  it("leaves a Back that actually moves after ten trips in and out", async () => {
    // Up is ordinary navigation to a different position, so it PUSHES like any
    // other move. When it replaced instead, every trip in pushed an entry and
    // every trip out overwrote the top one with home, so ten cycles silted up
    // ten identical home entries and the browser's Back did nothing visible ten
    // times before it did anything at all.
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    for (let i = 0; i < 10; i++) {
      mod.selectSession("s1")
      mod.navigateUp()
    }
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    // No two neighbouring entries name the same position, so no Back is inert.
    const inertSteps = entries.filter(
      (entry, at) => at > DUX_ENTRY_INDEX && entry === entries[at - 1],
    )
    expect(inertSteps).toEqual([])
    // One Back from home lands back on the agent, not on another home entry.
    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    expect(loc.hash).toBe("#/agent/s1")
  })

  it("restores the selection named by the entry Back lands on", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s1")
    mod.openChangesScreen()
    history.back()
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
  })
})

// The changes screen is a position like any other, so the URL has to keep
// naming it. Resolving a route used to commit the target first and set the
// screen second, so the URL was written from a half-applied route and the
// `/changes` segment was stripped off the address the user was looking at.
describe("the changes screen keeps its place in the URL", () => {
  it("keeps the address it booted on", async () => {
    const mod = await loadStore("#/agent/s1/changes", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    // Reload, bookmark and share all read this back, so it has to survive.
    expect(loc.hash).toBe("#/agent/s1/changes")
    expect(entries[index]).toBe("#/agent/s1/changes")
  })

  it("keeps the address it booted on for an extra tab", async () => {
    const mod = await loadStore("#/agent/s1/tab/t2/changes", [
      { id: "s1", project_id: "p1", tabs: ["t2"] },
    ])
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/tab/t2/changes")
  })

  it("survives being visited by Back and Forward, repeatedly", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.openChangesScreen()
    expect(loc.hash).toBe("#/agent/s1/changes")

    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    history.forward()
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/changes")

    // The second round trip is the one that used to fail: the first Forward
    // rewrote the entry down to the bare agent, so the screen became
    // permanently unreachable by Forward.
    history.back()
    history.forward()
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/changes")
  })

  it("keeps its segment when the agent it names comes back", async () => {
    // The not-found retry resolves the route the URL still names, changes
    // segment included.
    const mod = await loadStore("#/agent/s1/changes", [])
    expect(mod.getSnapshot().routeNotFound).not.toBeNull()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
    expect(loc.hash).toBe("#/agent/s1/changes")
  })
})

describe("a URL naming a missing agent renders not-found", () => {
  it("reports not-found for a deep link to an agent that does not exist", async () => {
    const mod = await loadStore("#/agent/missing", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "missing",
    })
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("reports not-found when Back lands on a deleted agent", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.openChangesScreen()
    // The changes entry is rewritten to the destination when s1 vanishes, but
    // the terminal entry underneath still names s1.
    await pushSpine(mod, [])
    history.back()
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "s1",
    })
  })

  it("stops saying not-found once the agent is back in the spine", async () => {
    // The agent is missing from the first spine (this client connected while it
    // was being created, or a delete was rolled back) and arrives in a later
    // one. Nothing else clears the flag: the user is looking at a dead-end
    // screen whose single button is the only way out, so the spine that proves
    // the URL right again has to retire it.
    const mod = await loadStore("#/agent/s1", [])
    expect(mod.getSnapshot().routeNotFound).not.toBeNull()
    await pushSpine(mod, [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().routeNotFound).toBeNull()
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
  })

  it("keeps not-found when a later spine still lacks the agent", async () => {
    const mod = await loadStore("#/agent/s1", [])
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "s1",
    })
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("clears not-found once a real destination is selected", async () => {
    const mod = await loadStore("#/agent/missing", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().routeNotFound).not.toBeNull()
    mod.selectSession("s1")
    expect(mod.getSnapshot().routeNotFound).toBeNull()
  })

  it("leaves no not-found entry behind when the user takes its way out", async () => {
    // "Back to agents" is a correction of a bad URL, not a place worth keeping:
    // if it pushed home, the browser's Back would put the user straight back on
    // the dead end they just left.
    const mod = await loadStore("#/agent/missing", [
      { id: "s1", project_id: "p1" },
    ])
    mod.navigateUp()
    expect(mod.getSnapshot().routeNotFound).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
    expect(index).toBe(DUX_ENTRY_INDEX)
    expect(entries[index]).toBe("/")
  })
})

describe("a failing history write is reported as what it is", () => {
  it("names the history write, not the fetch, when the browser refuses one", async () => {
    // Safari rate-limits history calls, so a write CAN throw. Reporting that as
    // "spine fetch failed" for a fetch that worked perfectly would send whoever
    // reads the console at the network.
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    historyWriteError = new Error("SecurityError: too many history writes")
    // Deleting the focused agent makes the apply navigate, which writes the URL.
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    const messages = warn.mock.calls.map((call) => String(call[0]))
    warn.mockRestore()
    expect(messages).toContain("[dux] history write refused")
    expect(messages).not.toContain(
      "[dux] spine fetch failed; will retry on reconnect",
    )
  })

  it("does not throw out of a user's click", async () => {
    // The guard used to sit only around the spine apply, so every user-driven
    // navigation was unprotected: a refused write threw into the click handler
    // AFTER the screen had already moved.
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    historyWriteError = new Error("SecurityError: too many history writes")
    expect(() => mod.selectSession("s1")).not.toThrow()
    expect(() => mod.openChangesScreen()).not.toThrow()
    expect(() => mod.navigateUp()).not.toThrow()
    expect(() => mod.selectSession("s2")).not.toThrow()
    expect(() =>
      mod.selectTerminal("t1", { kind: "project", projectId: "p1" }),
    ).not.toThrow()
    expect(() => mod.selectTab("s1", "s1")).not.toThrow()
    const messages = warn.mock.calls.map((call) => String(call[0]))
    warn.mockRestore()
    // Refused, but reported: the address bar lags until the next write lands.
    expect(messages).toContain("[dux] history write refused")
  })
})

describe("the in-app Up control never steps out of the app", () => {
  it("would leave the app entirely if Up stepped history after a deep-link boot", async () => {
    // The pin for the whole model: a deep-link boot pushes NOTHING, so the
    // screen the user is looking at IS dux's first entry. This is what any
    // relative Back does from there, and it is why Up must name a destination.
    await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    history.back()
    expect(leftApp).toBe(true)
  })

  it("goes home from a deep-linked terminal screen without leaving the app", async () => {
    const mod = await loadStore("#/agent/s1", [{ id: "s1", project_id: "p1" }])
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    const depth = index
    mod.navigateUp()
    expect(leftApp).toBe(false)
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(loc.hash).toBe("")
    // Home is a different position, so it is PUSHED like any other move, and
    // the deep-linked agent stays underneath it for Back to return to. What Up
    // must never do is STEP, which from this very entry walks out of dux.
    expect(index).toBe(depth + 1)
    history.back()
    expect(leftApp).toBe(false)
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
  })

  it("goes from the changes screen up to the agent it belongs to", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.openChangesScreen()
    const depth = index
    mod.navigateUp()
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(loc.hash).toBe("#/agent/s1")
    // Pushed, not rewritten: the agent screen is a position of its own, so Back
    // returns to the changes screen the user just left rather than skipping it.
    expect(index).toBe(depth + 1)
    history.back()
    expect(mod.getSnapshot().mobileScreen).toBe("changes")
  })

  it("goes home from the changes screen when nothing is focused", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.navigateUp()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(leftApp).toBe(false)
    expect(index).toBe(DUX_ENTRY_INDEX)
  })
})

describe("a URL naming a project terminal that is gone", () => {
  it("lands home and rewrites the URL rather than leaving it lying", async () => {
    // The URL says a project terminal; the workspace has no such terminal. The
    // silent return used to leave the address bar naming it while the app
    // showed something else, which is the exact disagreement this model exists
    // to remove.
    const mod = await loadStore(
      "#/project/p1/terminal/gone",
      [{ id: "s1", project_id: "p1" }],
      [{ id: "p1", terminals: [] }],
    )
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(loc.hash).toBe("")
  })

  it("lands home when the project itself is gone", async () => {
    const mod = await loadStore(
      "#/project/nope/terminal/t1",
      [{ id: "s1", project_id: "p1" }],
      [{ id: "p1", terminals: ["t1"] }],
    )
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(loc.hash).toBe("")
  })

  it("rewrites the entry when Forward lands on a terminal that has since closed", async () => {
    const mod = await loadStore("", [], [{ id: "p1", terminals: ["t1"] }])
    mod.selectTerminal("t1", { kind: "project", projectId: "p1" })
    expect(loc.hash).toBe("#/project/p1/terminal/t1")
    history.back()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    await pushSpine(mod, [], [{ id: "p1", terminals: [] }])
    const entryCount = entries.length
    history.go(1)
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(loc.hash).toBe("")
    // The position assertions are the point: asserting the target and the
    // address alone left this green when the fallback PUSHED instead of
    // replacing, which traps the user. Every Back would land on the dead
    // terminal's entry again, resolve it again, and push another home entry on
    // top, so Back would never get anywhere. Forward must land on the entry
    // that already existed and REWRITE it.
    expect(entries.length).toBe(entryCount)
    expect(index).toBe(entryCount - 1)
    expect(entries[index]).toBe("/")
    expect(entries).not.toContain("#/project/p1/terminal/t1")
  })
})

describe("the changes screen needs something to show changes for", () => {
  it("stays on home when opened with nothing focused", async () => {
    // Without the guard the screen would read "changes" while the URL, which
    // has no target to hang the suffix on, collapses to home: exactly the
    // address-versus-state disagreement the router exists to remove.
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.openChangesScreen()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
  })
})

// A terminal owned by nothing lives at an address that names no owner. It must
// be reachable there, land on a real screen rather than falling through to the
// hub, and write that same address back out.
describe("a standalone terminal is addressable in its own right", () => {
  it("boots onto the un-nested address and lands on a real screen", async () => {
    const mod = await loadStore("#/terminal/solo-1", [], [], ["solo-1"])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "solo-1",
      owner: { kind: "standalone" },
    })
    // The phone journey: NOT the home screen. Before the owner was a tagged
    // value, a kind with no screen of its own fell through to the hub, which
    // looks exactly like the app ignoring the tap.
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    // It came in on this address, so it stays on it rather than being rewritten.
    expect(loc.hash).toBe("#/terminal/solo-1")
    expect(leftApp).toBe(false)
  })

  it("writes the un-nested address when one is selected from elsewhere", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }], [], [
      "solo-1",
    ])
    mod.selectTerminal("solo-1", { kind: "standalone" })
    expect(loc.hash).toBe("#/terminal/solo-1")
    expect(mod.getSnapshot().mobileScreen).toBe("terminal")
  })

  it("goes up to home, by naming it, never by stepping history", async () => {
    const mod = await loadStore("#/terminal/solo-1", [], [], ["solo-1"])
    const depth = index
    mod.navigateUp()
    expect(leftApp).toBe(false)
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
    expect(index).toBe(depth + 1)
  })
})

// (c) the editor rides the URL. The route grammar grows an editor suffix
// (`#/agent/<sid>/editor[/<mode>/<encoded-path>]`), mutually exclusive with
// the `/changes` suffix; opening the editor PUSHES one entry, in-editor file
// switches REPLACE it, one Back closes the editor and keeps every draft, and
// a hard refresh restores the editor and its open file from the address.
describe("the editor rides the URL", () => {
  function editorBuffer(path: string, draft: string) {
    return {
      path,
      loadedPath: path,
      loading: false,
      loaded: "on disk",
      draft,
      binary: false,
      readOnly: false,
      diff: null,
      diffLoadedPath: null,
      diffLoadedSignal: "",
      fileError: null,
      diffError: null,
      errorPath: null,
    }
  }

  it("parses and serializes as exact inverses over the suffix cross-product", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    const target = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }
    const routes = [
      { target, changes: false, editor: null, standalone: false },
      { target, changes: true, editor: null, standalone: false },
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: null },
        standalone: false,
      },
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: "src/a b.ts" },
        standalone: false,
      },
      {
        target,
        changes: false,
        editor: { mode: "diff" as const, path: "src/a b.ts" },
        standalone: false,
      },
      // Files whose NAMES collide with the grammar's own keywords: a path
      // literally called "editor" or "changes" must round-trip exactly (the
      // parser picks the split point whose prefix is a real target, not the
      // rightmost "/editor" in the string).
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: "editor" },
        standalone: false,
      },
      {
        target,
        changes: false,
        editor: { mode: "diff" as const, path: "docs/editor" },
        standalone: false,
      },
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: "changes" },
        standalone: false,
      },
      // The standalone surface: the same editor positions at their un-nested
      // whole-tab addresses.
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: null },
        standalone: true,
      },
      {
        target,
        changes: false,
        editor: { mode: "diff" as const, path: "src/a b.ts" },
        standalone: true,
      },
      {
        target,
        changes: false,
        editor: { mode: "file" as const, path: "editor" },
        standalone: true,
      },
    ]
    for (const route of routes) {
      expect(mod.parseRoute(mod.routeHash(route))).toEqual(route)
    }
  })

  it("normalizes the shapes the standalone grammar cannot carry", async () => {
    // Standalone routes are session-slot only BY DEFINITION (the surface is
    // the editor, not a tab strip), and they carry no changes screen. The
    // serializer already drops both; the parser can only ever produce the
    // normalized form, so serialize-then-parse lands on it.
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    const editor = { mode: "file" as const, path: "a.ts" }
    const normalized = {
      target: { kind: "agent" as const, sessionId: "s1", tabId: "s1" },
      changes: false,
      editor,
      standalone: true,
    }
    // An extra-tab target normalizes to the session-slot tab.
    expect(
      mod.parseRoute(
        mod.routeHash({
          target: { kind: "agent", sessionId: "s1", tabId: "t2" },
          changes: false,
          editor,
          standalone: true,
        }),
      ),
    ).toEqual(normalized)
    // A changes flag is dropped: the standalone form has nowhere to say it.
    expect(
      mod.parseRoute(
        mod.routeHash({
          target: { kind: "agent", sessionId: "s1", tabId: "s1" },
          changes: true,
          editor,
          standalone: true,
        }),
      ),
    ).toEqual(normalized)
  })

  it("the serializer emits at most one suffix, and the parser tries editor first", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    const target = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }
    // A route claiming both suffixes serializes to the editor form only.
    expect(
      mod.routeHash({
        target,
        changes: true,
        editor: { mode: "file", path: null },
      }),
    ).toBe("#/agent/s1/editor")
    // And an editor path that literally ends in "changes" stays an editor
    // route: the parser tries the editor suffix first.
    const tricky = mod.routeHash({
      target,
      changes: false,
      editor: { mode: "file", path: "changes" },
    })
    expect(mod.parseRoute(tricky).editor).toEqual({
      mode: "file",
      path: "changes",
    })
  })

  it("opening the editor pushes exactly one entry", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    const depth = index
    mod.openEditor("s1")
    expect(loc.hash).toBe("#/agent/s1/editor")
    expect(index).toBe(depth + 1)
    history.back()
    expect(loc.hash).toBe("#/agent/s1")
    expect(mod.getSnapshot().editorTarget).toBeNull()
  })

  it("an in-editor file switch replaces the entry rather than piling up", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    mod.selectSession("s1")
    mod.openEditor("s1", "a.ts")
    expect(loc.hash).toBe("#/agent/s1/editor/file/a.ts")
    const depth = index
    mod.editorSyncActiveTab("s1", "file", "b.ts")
    expect(loc.hash).toBe("#/agent/s1/editor/file/b.ts")
    expect(index).toBe(depth)
    mod.editorSyncActiveTab("s1", "diff", "b.ts")
    expect(loc.hash).toBe("#/agent/s1/editor/diff/b.ts")
    expect(index).toBe(depth)
  })

  it("one Back closes the editor and keeps tabs, dirty flags, and drafts", async () => {
    const mod = await loadStore("", [{ id: "s1", project_id: "p1" }])
    const drafts = await import("./editorDrafts")
    mod.selectSession("s1")
    mod.openEditor("s1", "a.ts")
    const tabId = mod.getSnapshot().editorTabs.s1.tabs[0].id
    // The user typed: the store flag flips and the draft cache holds the text
    // (in the app EditorBody does both; the store test does them directly).
    mod.editorSetTabDirty("s1", tabId, true)
    drafts.storeSessionDrafts(
      "s1",
      new Map([[tabId, editorBuffer("a.ts", "typed and unsaved")]]),
    )

    history.back()
    // Closed, and the dirty state did NOT gate the Back: there is no cancel,
    // because nothing is lost.
    expect(mod.getSnapshot().editorTarget).toBeNull()
    expect(mod.getSnapshot().editorRoute).toBeNull()
    expect(loc.hash).toBe("#/agent/s1")
    expect(mod.getSnapshot().editorTabs.s1.tabs).toHaveLength(1)
    expect(mod.getSnapshot().editorTabs.s1.tabs[0].dirty).toBe(true)
    expect(drafts.loadSessionDrafts("s1").get(tabId)?.draft).toBe(
      "typed and unsaved",
    )

    // And Forward reopens it, tab intact, from the address alone.
    history.forward()
    expect(mod.getSnapshot().editorTarget).not.toBeNull()
    expect(mod.getSnapshot().editorRoute).toEqual({
      sessionId: "s1",
      mode: "file",
      path: "a.ts",
    })
    drafts.clearSessionDrafts("s1")
  })

  it("a hard refresh restores the editor and its open file from the address", async () => {
    const mod = await loadStore("#/agent/s1/editor/file/src%2Fa.ts", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(mod.getSnapshot().editorTarget).toEqual({
      sessionId: "s1",
      initialPath: "src/a.ts",
      initialMode: "file",
    })
    expect(mod.getSnapshot().editorRoute).toEqual({
      sessionId: "s1",
      mode: "file",
      path: "src/a.ts",
    })
    expect(mod.getSnapshot().editorTabs.s1.tabs.map((t) => t.path)).toEqual([
      "src/a.ts",
    ])
    // A restore, not a move: the browser is already parked on this entry.
    expect(loc.hash).toBe("#/agent/s1/editor/file/src%2Fa.ts")
    expect(index).toBe(DUX_ENTRY_INDEX)
  })

  it("clears the editor with a single URL write when its session vanishes", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    mod.selectSession("s1")
    mod.openEditor("s1", "a.ts")
    const depth = index
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    // The selection prune is the single URL writer in that pass; the editor
    // prune only clears state.
    expect(mod.getSnapshot().editorTarget).toBeNull()
    expect(mod.getSnapshot().editorRoute).toBeNull()
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    expect(loc.hash).toBe("#/agent/s2")
    expect(index).toBe(depth)
    expect(entries).not.toContain("#/agent/s1/editor/file/a.ts")
  })

  it("boots the standalone editor surface from its own address", async () => {
    const mod = await loadStore("#/editor/agent/s1/file/src%2Fa.ts", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().standaloneEditor).toBe(true)
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(mod.getSnapshot().editorRoute).toEqual({
      sessionId: "s1",
      mode: "file",
      path: "src/a.ts",
    })
    expect(mod.getSnapshot().editorTabs.s1.tabs.map((t) => t.path)).toEqual([
      "src/a.ts",
    ])
    // A restore of the address the tab opened on: nothing pushed, nothing
    // rewritten.
    expect(loc.hash).toBe("#/editor/agent/s1/file/src%2Fa.ts")
    expect(index).toBe(DUX_ENTRY_INDEX)
  })

  it("keeps the standalone grammar when switching files in the standalone tab", async () => {
    const mod = await loadStore("#/editor/agent/s1", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().standaloneEditor).toBe(true)
    const depth = index
    mod.editorSyncActiveTab("s1", "file", "b.ts")
    expect(loc.hash).toBe("#/editor/agent/s1/file/b.ts")
    expect(index).toBe(depth)
  })

  it("leaves the standalone surface when the address stops naming it", async () => {
    // The standalone header's open-in-dux link is a plain hash anchor: the
    // browser pushes the entry and fires popstate, and the URL, as always,
    // is what decides which surface renders.
    const mod = await loadStore("#/editor/agent/s1", [
      { id: "s1", project_id: "p1" },
    ])
    popstateTo("#/agent/s1")
    expect(mod.getSnapshot().standaloneEditor).toBe(false)
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    // The in-app address names no editor, so the editor state closed with it.
    expect(mod.getSnapshot().editorRoute).toBeNull()
  })

  it("escapes a dead standalone link onto a real surface, never the boot spinner", async () => {
    // The blocker: a standalone tab whose agent is gone used to keep
    // `standaloneEditor` true with `editorTarget` null, and the not-found
    // screen's only button then landed the shell on its boot spinner forever.
    const mod = await loadStore("#/editor/agent/missing", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "missing",
    })
    // Clearing the editor state outside popstate also clears the surface
    // flag, so not-found already renders in the ordinary shell.
    expect(mod.getSnapshot().standaloneEditor).toBe(false)
    mod.navigateUp()
    expect(mod.getSnapshot().standaloneEditor).toBe(false)
    expect(mod.getSnapshot().routeNotFound).toBeNull()
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(loc.hash).toBe("")
  })

  it("swaps the standalone tab to the ordinary shell when its session vanishes", async () => {
    const mod = await loadStore("#/editor/agent/s1/file/a.ts", [
      { id: "s1", project_id: "p1" },
      { id: "s2", project_id: "p1" },
    ])
    expect(mod.getSnapshot().standaloneEditor).toBe(true)
    const depth = index
    await pushSpine(mod, [{ id: "s2", project_id: "p1" }])
    // The selection prune stays the single URL writer and writes the in-app
    // grammar; the editor prune clears the state AND the surface flag, so
    // the tab renders the workspace rather than a spinner.
    expect(mod.getSnapshot().standaloneEditor).toBe(false)
    expect(mod.getSnapshot().editorTarget).toBeNull()
    expect(mod.getSnapshot().editorRoute).toBeNull()
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
    expect(loc.hash).toBe("#/agent/s2")
    expect(index).toBe(depth)
  })

  it("boots the NORMAL shell on a standalone address with a malformed tail", async () => {
    // Strict parse at boot: the surface flag comes from the same grammar the
    // router uses, so a mangled link cannot marooon the tab on a standalone
    // shell whose route resolves to nothing.
    const mod = await loadStore("#/editor/agent/s1/file/%ZZ", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().standaloneEditor).toBe(false)
    expect(mod.getSnapshot().mobileScreen).toBe("home")
    expect(mod.getSnapshot().editorTarget).toBeNull()
  })

  it("renders not-found for an editor link to an agent that does not exist", async () => {
    const mod = await loadStore("#/agent/missing/editor/file/a.ts", [
      { id: "s1", project_id: "p1" },
    ])
    expect(mod.getSnapshot().routeNotFound).toEqual({
      kind: "agent",
      sessionId: "missing",
    })
    expect(mod.getSnapshot().editorTarget).toBeNull()
    expect(mod.getSnapshot().editorRoute).toBeNull()
  })
})
