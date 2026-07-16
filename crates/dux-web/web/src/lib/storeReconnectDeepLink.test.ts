import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"

// Regression coverage for the reconnect deep-link loss: when the connection
// drops while the user is deep-linked to a running agent, clicking "Reconnect
// now" used to land them on the HOME screen. The center pane transiently ejects
// to the welcome screen while the agent is momentarily `detached` (it exited
// during the outage and has not finished resuming), which wipes the URL hash to
// home; nothing restored the route because `restoreDeepLink` is a spent boot
// one-shot. The store now re-arms the deep-link from `location.hash` on every
// events reconnect (BEFORE any spine apply can wipe it) and re-restores it once
// the agent is present AND back to `active`.

type Sess = {
  id: string
  project_id: string
  status?: "active" | "detached" | "exited"
  terminals?: string[]
  tabs?: string[]
}

function makeSpine(
  sessions: Sess[],
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
      status: s.status ?? "active",
      terminals: (s.terminals ?? []).map((id) => ({ id })),
      tabs: [{ id: s.id }, ...(s.tabs ?? []).map((id) => ({ id }))],
    })) as unknown as Spine["sessions"],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])
// A mutable backing store for `location.hash` so the store's `history.replaceState`
// mirror actually round-trips (the reconnect re-arm reads `location.hash`).
const hashRef = { value: "" }

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
  hashRef.value = ""
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    go: () => {},
    // Mirror what the real hash router does: an "#..." target lands in the hash;
    // the bare home path clears it.
    replaceState: (_s: unknown, _t: string, url: string) => {
      hashRef.value = url.startsWith("#") ? url : ""
    },
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
  sessions: Sess[],
  projects: { id: string; terminals?: string[] }[] = [],
) {
  hashRef.value = hash
  vi.stubGlobal("location", {
    protocol: "http:",
    host: "localhost:0",
    get hash() {
      return hashRef.value
    },
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

// Advance past the microtask queue so an async `loadSpine` fetch applies.
async function settle() {
  await new Promise((r) => setTimeout(r, 0))
}

type StoreMod = Awaited<ReturnType<typeof loadStore>>

// The first socket open after boot only consumes the `skipNextEventsOnOpenLoad`
// latch (the boot driver already did the initial load); it does not re-fetch. A
// real drop-and-reopen is the SECOND (and later) open. Fire the first here so the
// tests below drive genuine reconnects.
async function consumeBootOpen(mod: StoreMod) {
  mod.eventsSocket.onOpen()
  await settle()
}

describe("reconnect preserves a deep-linked agent route", () => {
  it("restores the agent after a transient detached-eject during reconnect", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1", status: "active" },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    await consumeBootOpen(mod)

    // --- The drop: the socket closed; selection/hash are untouched while offline.
    // --- The reconnect: the events socket reopens and re-fetches the spine, which
    // now reports s1 as `detached` (it exited during the outage, resume pending).
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "detached" }])
    mod.eventsSocket.onOpen()
    await settle()

    // The center pane (TerminalPane) mirrors the TUI exit behavior and ejects to
    // the welcome screen for a non-active session-slot agent, wiping the hash to
    // home. Model that eject here via the marker function TerminalPane calls
    // instead of a bare `selectSession(null)` (it fires from a React effect
    // after the apply).
    mod.ejectSelectionForReconnect()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(hashRef.value).toBe("")

    // --- The resume completes: s1 is `active` again and a fresh spine arrives.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "active" }])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    })

    // The route is back on the agent (bug: it stayed on home).
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    expect(hashRef.value).toBe("#/agent/s1")
  })

  it("does not undo a deliberate home navigation made during the armed window", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1", status: "active" },
    ])
    await consumeBootOpen(mod)

    // Reconnect: s1 detached (resume pending). Intent armed for s1.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "detached" }])
    mod.eventsSocket.onOpen()
    await settle()

    // The user deliberately navigates home themselves (NOT the TerminalPane
    // reconnect-eject) while s1 is still resuming, e.g. clicking a "back to
    // home" control. This must disarm the intent, not merely clear the target.
    mod.selectSession(null)
    expect(mod.getSnapshot().selectedTarget).toBeNull()

    // s1 finishes resuming; the deliberate home nav must be respected, not
    // yanked back onto the agent.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "active" }])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await settle()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(hashRef.value).toBe("")
  })

  it("restores the agent after a second reconnect wipes the hash again before resume completes", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1", status: "active" },
    ])
    await consumeBootOpen(mod)

    // First reconnect: s1 detached (resume pending). Intent armed for s1, then
    // the transient eject wipes the hash back to home.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "detached" }])
    mod.eventsSocket.onOpen()
    await settle()
    mod.ejectSelectionForReconnect()
    expect(hashRef.value).toBe("")

    // Second reconnect (e.g. a flaky connection drops again) fires BEFORE s1
    // has finished resuming. `armReconnectDeepLink` now reads the hash as home
    // (our own eject wiped it); it must NOT discard the still-valid armed
    // intent for s1.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "detached" }])
    mod.eventsSocket.onOpen()
    await settle()

    // The resume completes on this later reconnect.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "active" }])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    })
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    expect(hashRef.value).toBe("#/agent/s1")
  })
})

describe("reconnect deep-link guard rails", () => {
  it("does NOT resurrect a genuinely deleted agent", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1", status: "active" },
    ])
    await consumeBootOpen(mod)

    // Reconnect while s1 has been deleted (gone from the spine entirely). The
    // prune ejects to home; the re-armed intent must drop, not force it back.
    spineBody = makeSpine([{ id: "s2", project_id: "p1", status: "active" }])
    mod.eventsSocket.onOpen()
    await settle()
    expect(mod.getSnapshot().selectedTarget).toBeNull()

    // A later spine (still no s1) must not conjure it back either.
    spineBody = makeSpine([{ id: "s2", project_id: "p1", status: "active" }])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await settle()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("stays on home when the user was on home before the drop", async () => {
    const mod = await loadStore("", [
      { id: "s1", project_id: "p1", status: "active" },
    ])
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    await consumeBootOpen(mod)

    // Reconnect: nothing was deep-linked, so nothing is armed or restored.
    spineBody = makeSpine([{ id: "s1", project_id: "p1", status: "active" }])
    mod.eventsSocket.onOpen()
    await settle()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    expect(hashRef.value).toBe("")
  })

  it("does not yank the user back when they navigate to a different agent mid-resume", async () => {
    const mod = await loadStore("#/agent/s1", [
      { id: "s1", project_id: "p1", status: "active" },
      { id: "s2", project_id: "p1", status: "active" },
    ])
    await consumeBootOpen(mod)

    // Reconnect: s1 detached (resume pending). Intent armed for s1.
    spineBody = makeSpine([
      { id: "s1", project_id: "p1", status: "detached" },
      { id: "s2", project_id: "p1", status: "active" },
    ])
    mod.eventsSocket.onOpen()
    await settle()

    // The user deliberately opens a DIFFERENT agent while s1 is still resuming.
    mod.selectSession("s2")
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")

    // s1 finishes resuming; the intent must have been dropped, leaving the user
    // on their chosen agent rather than snapping back to s1.
    spineBody = makeSpine([
      { id: "s1", project_id: "p1", status: "active" },
      { id: "s2", project_id: "p1", status: "active" },
    ])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await settle()
    expect(mod.getSnapshot().selectedSessionId).toBe("s2")
  })

  it("a project-terminal deep link survives an events-socket reconnect", async () => {
    // A project terminal has no resume phase and never receives the agent
    // pane's transient eject, so the reconnect must leave its selection (and
    // the armed intent must disarm as a no-op, never yank the user anywhere).
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
    await consumeBootOpen(mod)

    // A drop-and-reopen arms the intent from the hash, then reloads the spine.
    spineBody = makeSpine([], [{ id: "p1", terminals: ["pt1"] }])
    mod.eventsSocket.onOpen()
    await settle()

    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "pt1",
      owner: { kind: "project", projectId: "p1" },
    })
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
  })

  it("restores a project terminal cleared by our own eject while armed", async () => {
    // The restorable gap: the selection was cleared by the store's OWN
    // reconnect eject while the intent was armed; once the spine carries the
    // terminal again the route comes back.
    const mod = await loadStore(
      "#/project/p1/terminal/pt1",
      [],
      [{ id: "p1", terminals: ["pt1"] }],
    )
    await consumeBootOpen(mod)

    // Reopen arms from the hash synchronously; our own eject then clears the
    // selection BEFORE the reconnect's spine apply lands (the race the armed
    // intent exists to heal).
    spineBody = makeSpine([], [{ id: "p1", terminals: ["pt1"] }])
    mod.eventsSocket.onOpen()
    mod.ejectSelectionForReconnect()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
    await settle()

    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    })
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "pt1",
      owner: { kind: "project", projectId: "p1" },
    })
  })

  it("restores an extra-tab deep link across a reconnect", async () => {
    const mod = await loadStore("#/agent/s1/tab/t2", [
      { id: "s1", project_id: "p1", status: "active", tabs: ["t2"] },
    ])
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t2",
    })
    await consumeBootOpen(mod)

    spineBody = makeSpine([
      { id: "s1", project_id: "p1", status: "detached", tabs: ["t2"] },
    ])
    mod.eventsSocket.onOpen()
    await settle()
    mod.ejectSelectionForReconnect()

    spineBody = makeSpine([
      { id: "s1", project_id: "p1", status: "active", tabs: ["t2"] },
    ])
    mod.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).not.toBeNull()
    })
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t2",
    })
  })
})
