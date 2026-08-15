import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./workspaceApi"
import { partitionProjects } from "./projects"

// Exercises the spine slice end to end: the boot fetch populating the slice, a
// `projects.changed`/`sessions.changed` event triggering a refetch, the
// prune-on-gone and order-reconciliation behaviours that moved off the broadcast
// ViewModel onto the spine apply path, and a representative consumer
// (partitionProjects) reading from the slice. The `workspaceApi` wire behaviour (GET
// shape, error mapping) lives in `workspaceApi.test.ts`; here we drive the store's
// integration via a controllable fetch double.
//
// The store fires a `GET /api/v1/workspace` at import. We serve the spine body from
// `spineBody`, which the fetch double reads at call time so a test can mutate it
// before a refetch.

function makeSpine(overrides: Partial<Spine> = {}): Spine {
  return {
    projects: [],
    sessions: [],
    terminals: [],
    sidebar: { groups: [], agentless_start: null },
    ...overrides,
  }
}

function session(id: string, projectId: string): Spine["sessions"][number] {
  return {
    id,
    project_id: projectId,
  } as unknown as Spine["sessions"][number]
}

function project(id: string): Spine["projects"][number] {
  return {
    id,
    name: id,
  } as unknown as Spine["projects"][number]
}

// A project-owned terminal in the spine's flat, owner-tagged collection.
function projectTerm(id: string, projectId: string): Spine["terminals"][number] {
  return {
    id,
    owner: { kind: "project", project_id: projectId },
  } as unknown as Spine["terminals"][number]
}

let spineBody: Spine = makeSpine()
let spineFetches = 0
// When true, the spine GET rejects (simulated network failure) — used to
// exercise the failed-first-load → reconnect-retry recovery path.
let spineShouldFail = false

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/workspace")) {
    spineFetches++
    if (spineShouldFail) throw new Error("network down")
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/changes")) {
    // The selected session's changed-files fetch (selectSession kicks one off).
    return {
      ok: true,
      status: 200,
      json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // /api/v1/bootstrap and anything else: empty body.
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
  spineBody = makeSpine()
  spineFetches = 0
  spineShouldFail = false
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
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

// Set the spine body, fire the invalidation event, and wait for the refetch to
// land. The applied spine is always a fresh object reference, so this detects
// application even when the content is unchanged.
async function pushSpine(
  mod: Awaited<ReturnType<typeof loadStore>>,
  body: Spine,
  event: "sessions.changed" | "projects.changed" = "sessions.changed",
): Promise<void> {
  const prev = mod.getSnapshot().spine
  spineBody = body
  mod.eventsSocket.onEvent({ event })
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBe(prev)
  })
}

describe("spine slice", () => {
  it("the boot fetch populates the slice", async () => {
    spineBody = makeSpine({
      projects: [project("p1")],
      sessions: [session("s1", "p1")],
    })
    const mod = await loadStore()
    const s = mod.getSnapshot().spine
    expect(s?.projects.map((p) => p.id)).toEqual(["p1"])
    expect(s?.sessions.map((x) => x.id)).toEqual(["s1"])
    // Exactly one spine GET on boot.
    expect(spineFetches).toBe(1)
  })

  it("a sessions.changed event triggers a refetch that replaces the slice", async () => {
    const mod = await loadStore()
    const before = spineFetches
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
      "sessions.changed",
    )
    expect(mod.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual([
      "s1",
      "s2",
    ])
    expect(spineFetches).toBe(before + 1)
  })

  it("a sessions.changed refetch carries a tab's input_owner through ingestion", async () => {
    // The server publishes the input-owning PTY-socket connection id per tab
    // (an ownership flip fires `sessions.changed`); the ingestion path must
    // hand it through untouched so `sessionActiveElsewhere` can gate the row
    // menus of agents no pane here is attached to.
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({
        sessions: [
          {
            ...session("s1", "p1"),
            tabs: [
              { id: "s1", provider: "claude", order: 0, input_owner: "42" },
            ],
          } as unknown as Spine["sessions"][number],
        ],
      }),
      "sessions.changed",
    )
    expect(
      mod.getSnapshot().spine?.sessions[0]?.tabs?.[0]?.input_owner,
    ).toBe("42")
  })

  describe("a tab's launched drop-paste profile", () => {
    // WHY IT LIVES ON THE SPINE. The profile says how a file dropped onto a tab
    // has its path quoted, and which CLI's length limit applies. It changes when
    // a process LAUNCHES or TERMINATES, and `sessions.changed` is the event that
    // fires for both. It used to be published on the BOOTSTRAP document, which
    // the browser refetches only on `config.changed`, so a client's copy went
    // stale for the whole life of a process and nothing corrected it short of a
    // reconnect or a restart.
    //
    // Both tests below start from a copy that IS stale and drive the correction
    // through the real event path, rather than injecting an already-correct one.

    function tabbed(
      dropPaste: { form: string; command_name: string } | undefined,
      provider = "codex",
    ): Spine {
      return makeSpine({
        sessions: [
          {
            id: "s1",
            project_id: "p1",
            provider,
            tabs: [
              {
                id: "s1",
                provider,
                order: 0,
                has_live_process: dropPaste !== undefined,
                drop_paste: dropPaste,
              },
            ],
          } as unknown as Spine["sessions"][number],
        ],
      })
    }

    const tabProfile = (mod: Awaited<ReturnType<typeof loadStore>>) =>
      mod.getSnapshot().spine?.sessions[0]?.tabs?.[0]?.drop_paste

    it("is corrected by the relaunch, after a config edit made it stale", async () => {
      // SEQUENCE ONE. A live codex tab launched single-quoted. The user edits
      // `[providers.codex] web_dragdrop_paste` to bare; a `config.changed`
      // refetch lands, and the browser is still holding the tab's OLD profile
      // because a config refetch cannot change it. Then the tab is relaunched
      // and picks the new form up.
      spineBody = tabbed({ form: "single_quoted", command_name: "codex" })
      const mod = await loadStore()
      expect(tabProfile(mod)?.form).toBe("single_quoted")

      // The config edit. It refreshes the bootstrap document and nothing else:
      // the spine is not refetched at all, which is exactly why the profile
      // cannot live on a document that event refreshes. The browser is still
      // holding what the live process launched with, which is correct, because
      // that process has not been replaced yet.
      const spineFetchesBefore = spineFetches
      spineBody = tabbed({ form: "bare", command_name: "codex" })
      mod.eventsSocket.onEvent({ event: "config.changed" })
      await new Promise((r) => setTimeout(r, 0))
      expect(spineFetches).toBe(spineFetchesBefore)
      expect(tabProfile(mod)?.form).toBe("single_quoted")

      // The relaunch. This is the event the profile now travels on.
      await pushSpine(mod, tabbed({ form: "bare", command_name: "codex" }))
      expect(tabProfile(mod)?.form).toBe("bare")
    })

    it("is corrected when a dormant tab relaunches under a different provider", async () => {
      // SEQUENCE TWO. The tab goes dormant (its process exits), the user
      // retargets it to claude, and it relaunches. Every step is a
      // `sessions.changed`, and the profile follows all three: the stale codex
      // entry must not survive the tab going dormant, and it must not win over
      // the claude one that replaces it.
      spineBody = tabbed({ form: "single_quoted", command_name: "codex" })
      const mod = await loadStore()
      expect(tabProfile(mod)?.command_name).toBe("codex")

      // Dormant: no live process, so no profile at all. A pane now falls back
      // to what config says the tab WILL launch with.
      await pushSpine(mod, tabbed(undefined))
      expect(tabProfile(mod)).toBeUndefined()

      // Relaunched under claude, which wants a different form and has no
      // length limit.
      await pushSpine(
        mod,
        tabbed({ form: "bare", command_name: "claude" }, "claude"),
      )
      expect(tabProfile(mod)).toEqual({ form: "bare", command_name: "claude" })
    })
  })

  it("a projects.changed event triggers a refetch that replaces the slice", async () => {
    const mod = await loadStore()
    const before = spineFetches
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p1"), project("p2")] }),
      "projects.changed",
    )
    expect(mod.getSnapshot().spine?.projects.map((p) => p.id)).toEqual([
      "p1",
      "p2",
    ])
    expect(spineFetches).toBe(before + 1)
  })

  it("an unrelated event does not refetch the spine", async () => {
    const mod = await loadStore()
    const before = spineFetches
    // A session.changes event for an unselected session must not touch the spine.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s-x", rev: 1 })
    expect(spineFetches).toBe(before)
  })

  it("prunes the selection when its session vanishes from the spine", async () => {
    const mod = await loadStore()
    await pushSpine(mod, makeSpine({ sessions: [session("s1", "p1")] }))
    mod.selectSession("s1")
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    // The session is gone in the next spine — the selection must clear.
    await pushSpine(mod, makeSpine({ sessions: [] }))
    expect(mod.getSnapshot().selectedSessionId).toBeNull()
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("keeps a focused project terminal selected across a spine apply", async () => {
    // The trap this guards (T10): the prune resolved the terminal through a
    // (nonexistent) owning session, so a focused project terminal was ejected
    // to home on EVERY spine refresh.
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({
        projects: [project("p1")],
        terminals: [projectTerm("pt1", "p1")],
      }),
      "projects.changed",
    )
    mod.selectTerminal("pt1", { kind: "project", projectId: "p1" })
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "pt1",
      owner: { kind: "project", projectId: "p1" },
    })
    // A refresh that still carries the terminal must NOT eject the selection.
    await pushSpine(
      mod,
      makeSpine({
        projects: [project("p1")],
        terminals: [projectTerm("pt1", "p1")],
      }),
      "projects.changed",
    )
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "terminal",
      terminalId: "pt1",
      owner: { kind: "project", projectId: "p1" },
    })
  })

  it("clears the selection when the focused project terminal vanishes", async () => {
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({
        projects: [project("p1")],
        terminals: [projectTerm("pt1", "p1")],
      }),
      "projects.changed",
    )
    mod.selectTerminal("pt1", { kind: "project", projectId: "p1" })
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p1")] }),
      "projects.changed",
    )
    expect(mod.getSnapshot().selectedTarget).toBeNull()
  })

  it("retires an optimistic session-order overlay once the spine matches", async () => {
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
    )
    // Optimistically reorder p1's sessions; the overlay holds until confirmed.
    mod.reorderSessions("p1", ["s2", "s1"])
    expect(mod.getSnapshot().pendingSessionOrder).not.toBeNull()
    // A spine whose order does NOT match keeps the overlay.
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", "p1"), session("s2", "p1")] }),
    )
    expect(mod.getSnapshot().pendingSessionOrder).not.toBeNull()
    // A spine confirming the new order retires it.
    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s2", "p1"), session("s1", "p1")] }),
    )
    expect(mod.getSnapshot().pendingSessionOrder).toBeNull()
  })

  it("retires an optimistic project-order overlay once the spine matches", async () => {
    const mod = await loadStore()
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p1"), project("p2")] }),
      "projects.changed",
    )
    mod.reorderProjects(["p2", "p1"])
    expect(mod.getSnapshot().pendingProjectOrder).not.toBeNull()
    await pushSpine(
      mod,
      makeSpine({ projects: [project("p2"), project("p1")] }),
      "projects.changed",
    )
    expect(mod.getSnapshot().pendingProjectOrder).toBeNull()
  })

  it("retries a failed first spine load on a reconnect onOpen", async () => {
    // The very first load (driven by boot()) fails, so the slice stays null.
    spineShouldFail = true
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().booted).toBe(true)
    })
    await vi.waitFor(() => {
      expect(spineFetches).toBeGreaterThanOrEqual(1)
    })
    expect(mod.getSnapshot().spine).toBeNull()

    // The initial connect's open consumes the skip flag and does NOT refetch
    // (boot() already drove the first load -- even though it failed).
    const afterInitialOpen = spineFetches
    mod.eventsSocket.onOpen()
    expect(spineFetches).toBe(afterInitialOpen)
    expect(mod.getSnapshot().spine).toBeNull()

    // A later RE-connect retries even though the slice is still null (the old
    // `spine !== null` guard would have skipped this forever). It now succeeds.
    spineShouldFail = false
    mod.eventsSocket.onOpen()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
  })

  it("a representative consumer (partitionProjects) reads from the slice", async () => {
    spineBody = makeSpine({
      projects: [project("p1")],
      sessions: [session("s1", "p1")],
      sidebar: {
        groups: [
          {
            project_id: "p1",
            name: "Project One",
            orphaned: false,
            path_missing: false,
            session_ids: ["s1"],
          },
        ],
        agentless_start: null,
      },
    })
    const mod = await loadStore()
    const s = mod.getSnapshot().spine
    const partitioned = partitionProjects(s?.sidebar, s?.projects ?? [], s?.sessions ?? [])
    expect(partitioned.withAgents).toEqual(["p1"])
    expect(partitioned.projectName("p1")).toBe("Project One")
    expect(partitioned.grouped.get("p1")?.map((x) => x.id)).toEqual(["s1"])
  })
})
