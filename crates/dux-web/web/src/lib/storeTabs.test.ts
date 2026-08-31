import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"

// Exercises the store's agent-tab lifecycle wiring (mirrors storeTerminals.test.ts):
// addTab POSTs the nested endpoint and focuses the returned tab; closeTab retargets
// a focused extra tab back to the session-slot tab and DELETEs; retargetTab validates + PATCHes.
// The tabsApi wire behaviour itself is in tabsApi.test.ts.

function makeBootstrap(): Bootstrap {
  return {
    available_providers: ["claude", "codex", "opencode"],
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
    agent_tabs_max: 20,
  }
}

// A session with a session-slot tab (id === s1) and one extra tab (b2).
function makeSpine() {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "",
          initial_branch: "",
          branch_provenance: "created",
          source_branch: "",
          worktree_path: "",
        },
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
  if (u.includes("/api/v1/workspace")) {
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
  if (u.includes("/tabs/") && init?.method === "DELETE") {
    // Mirror the real server: 200 + the authoritative `{ detached }` outcome,
    // computed from whether any OTHER tab in the closed tab's session is still
    // live (the closed tab itself never counts).
    const match = u.match(/\/sessions\/([^/]+)\/tabs\/([^/]+)$/)
    const sid = match?.[1]
    const tid = match?.[2]
    const session = (
      spineBody as {
        sessions: {
          id: string
          slot_tab_id?: string
          tabs: { id: string; has_live_process?: boolean }[]
        }[]
      }
    ).sessions.find((s) => s.id === sid)
    const detached = session
      ? !session.tabs.some((t) => t.id !== tid && t.has_live_process)
      : true
    // Mirror the real route's promotion: closing the tab the session's pointer
    // names hands the slot to the next tab in strip order, and the body says
    // which one took it.
    const promoted =
      session && (session.slot_tab_id ?? session.id) === tid
        ? session.tabs.find((t) => t.id !== tid)?.id
        : undefined
    const body = promoted === undefined ? { detached } : { detached, promoted }
    return {
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => JSON.stringify(body),
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/tabs/")) {
    return {
      ok: true,
      status: 200,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.endsWith("/reconnect") && init?.method === "POST") {
    return {
      ok: true,
      status: 200,
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
// re-fetch the current `spineBody` and re-run applyWorkspace.
function fireSessionsChanged() {
  sockets.at(-1)?.onmessage?.({
    data: JSON.stringify({ event: "sessions.changed" }),
  })
}

// A spine with a session-slot tab (s1) and one extra tab (b2) whose liveness varies.
function spineWithExtraTab(b2Live: boolean) {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "",
          initial_branch: "",
          branch_provenance: "created",
          source_branch: "",
          worktree_path: "",
        },
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

// A spine whose SESSION-SLOT tab (s1) has no live process: the shape a create or
// a reconnect leaves behind while its launch is still in flight, and the shape a
// launch that FAILED leaves behind forever.
function spineWithDormantSlot() {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [
      {
        id: "s1",
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "",
          initial_branch: "",
          branch_provenance: "created",
          source_branch: "",
          worktree_path: "",
        },
        terminals: [],
        tabs: [{ id: "s1", has_live_process: false }],
      },
    ],
    sidebar: { groups: [] },
  }
}

// The address bar these tests read back. `syncUrl` is the ONE place the store
// writes history, so mirroring its writes into `hash` is what lets a test assert
// the URL actually moved rather than that `routeHash` would have computed the
// right string.
let fakeLocation = {
  host: "localhost:0",
  protocol: "http:",
  pathname: "/",
  search: "",
  hash: "",
}

function writeUrl(url: string) {
  fakeLocation.hash = url.startsWith("#") ? url : ""
}

beforeEach(() => {
  calls = []
  sockets = []
  spineBody = makeSpine()
  fakeLocation = {
    host: "localhost:0",
    protocol: "http:",
    pathname: "/",
    search: "",
    hash: "",
  }
  vi.stubGlobal("location", fakeLocation)
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    go: () => {},
    pushState: (_state: unknown, _title: string, url: string) => writeUrl(url),
    replaceState: (_state: unknown, _title: string, url: string) => writeUrl(url),
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

  it("addTab called twice synchronously (a double-click) fires only ONE POST", async () => {
    const mod = await loadStore()
    // Both calls happen before either has a chance to await/resolve — the
    // in-flight guard (`createTabInFlight`) must block the second one
    // synchronously, not just race it.
    mod.addTab("s1")
    mod.addTab("s1")
    expect(mod.getSnapshot().createTabInFlight).toEqual(["s1"])
    await vi.waitFor(() => {
      expect(mod.getSnapshot().createTabInFlight).not.toContain("s1")
    })
    const posts = calls.filter(
      ([u, init]) => u === "/api/v1/sessions/s1/tabs" && init?.method === "POST",
    )
    expect(posts).toHaveLength(1)
  })

  it("closeTab on the focused extra tab retargets to the session-slot tab and DELETEs", async () => {
    const mod = await loadStore()
    mod.selectTab("s1", "b2")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "b2",
    })
    mod.closeTab("s1", "b2")
    await tick()
    // Selection snapped back to the session-slot tab immediately (not left on the dead id).
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

  it("closeTab on the focused extra tab never re-selects the dead tab via a stale remembered value", async () => {
    // The spine still remembers the just-closed tab as `last_focused_tab`
    // (the DELETE resolved, but no `sessions.changed` refetch has pruned the
    // spine yet), AND the tab is still listed in `session.tabs` (also stale).
    // `closeTab`'s fallback must land on the session-slot tab directly,
    // without consulting that stale memory, or it would try to re-select the
    // tab it just deleted.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          terminals: [],
          tabs: [{ id: "s1" }, { id: "b2" }],
          last_focused_tab: "b2",
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.selectTab("s1", "b2")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "b2",
    })
    mod.closeTab("s1", "b2")
    await tick()
    // Selection landed on the session-slot tab, never bounced back to "b2".
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("closeTab on the SLOT tab lands on the promoted tab at the bare agent address", async () => {
    // The slot tab is closable, and the close promotes its neighbour. The
    // spine is still stale when the DELETE resolves (no `sessions.changed`
    // refetch has run), so the answer's `promoted` is the only thing that
    // knows which tab is the agent's first now: the selection must follow it,
    // and the address must be the bare agent form, because the promoted tab IS
    // the slot tab.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          slot_tab_id: "t1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          terminals: [],
          tabs: [
            { id: "t1", has_live_process: true },
            { id: "t2", has_live_process: true },
          ],
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.selectTab("s1", "t1")
    mod.closeTab("s1", "t1")
    await tick()

    const target = mod.getSnapshot().selectedTarget
    expect(target).toEqual({ kind: "agent", sessionId: "s1", tabId: "t2" })
    expect(
      mod.routeHash({
        target,
        changes: false,
        editor: null,
        standalone: false,
      }),
    ).toBe("#/agent/s1")
    // And the address bar actually moved there: `routeHash` alone would agree
    // even if nothing had written the URL.
    expect(fakeLocation.hash).toBe("#/agent/s1")
    const del = find(
      (u, init) =>
        u === "/api/v1/sessions/s1/tabs/t1" && init?.method === "DELETE",
    )
    expect(del).toBeDefined()
  })

  // ── The promotion overlay's lifecycle ────────────────────────────────────
  //
  // A close of the slot tab leaves an overlay behind, because the spine is
  // still the pre-close one when the DELETE resolves. The overlay is keyed on
  // the CLOSED tab, so a spine retires it by no longer listing that tab, and a
  // further promotion somebody else made cannot pin a dead answer here.

  // A three-tab agent whose slot is `slot` and whose tabs are `ids`, in strip
  // order. Written straight into `spineBody`, which is what the next workspace
  // fetch answers with.
  function seedTabs(slot: string, ids: string[]) {
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          slot_tab_id: slot,
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          terminals: [],
          tabs: ids.map((id) => ({ id, has_live_process: true })),
        },
      ],
      sidebar: { groups: [] },
    }
  }

  it("retires the promotion overlay on the spine that confirms the close", async () => {
    seedTabs("t1", ["t1", "t2", "t3"])
    const mod = await loadStore()
    mod.closeTab("s1", "t1")
    await tick()
    expect(mod.getSnapshot().pendingSlotTab).toEqual({
      s1: { closedTabId: "t1", promotedTabId: "t2" },
    })

    seedTabs("t2", ["t2", "t3"])
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingSlotTab).toEqual({})
    })
  })

  it("retires it on a spine where somebody else has already promoted again", async () => {
    // We moved the slot t1 -> t2; another surface then moved it t2 -> t3 before
    // our spine caught up. Keyed on the promoted id, the overlay would go on
    // insisting t2 is the slot forever, because no spine will ever say so
    // again.
    seedTabs("t1", ["t1", "t2", "t3"])
    const mod = await loadStore()
    mod.closeTab("s1", "t1")
    await tick()
    expect(mod.getSnapshot().pendingSlotTab.s1?.promotedTabId).toBe("t2")

    seedTabs("t3", ["t3"])
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().pendingSlotTab).toEqual({})
    })
    // And the slot the client now reports is the spine's, not the dead t2: a
    // fresh selection of the agent lands on t3.
    mod.selectSession("s1")
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "t3",
    })
  })

  it("keeps the overlay across a spine that predates the close", async () => {
    seedTabs("t1", ["t1", "t2", "t3"])
    const mod = await loadStore()
    mod.closeTab("s1", "t1")
    await tick()

    // A spine snapshotted before the DELETE landed: it still lists the closed
    // tab, so it says nothing about this close and the overlay stands.
    fireSessionsChanged()
    await tick()
    expect(mod.getSnapshot().pendingSlotTab).toEqual({
      s1: { closedTabId: "t1", promotedTabId: "t2" },
    })
  })

  it("re-keys the overlay onto the second close when two land before any spine", async () => {
    seedTabs("t1", ["t1", "t2", "t3"])
    const mod = await loadStore()
    mod.closeTab("s1", "t1")
    await tick()

    // Server-side the slot is t2 now; the client has not refetched, so its
    // spine still says t1. Closing t2 promotes t3, and the overlay must follow
    // the second close rather than keep answering for the first.
    seedTabs("t2", ["t2", "t3"])
    mod.closeTab("s1", "t2")
    await tick()
    expect(mod.getSnapshot().pendingSlotTab).toEqual({
      s1: { closedTabId: "t2", promotedTabId: "t3" },
    })
  })

  it("falls back out loud from a link naming the tab a promotion closed", async () => {
    // A bookmark or a second browser tab can still be pointing at the tab that
    // held the slot when a promotion closed it. It is a vanished target like
    // any other: the selection is rewritten to the agent's current first tab
    // rather than left on an id nothing answers for.
    spineBody = {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          slot_tab_id: "t1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          terminals: [],
          tabs: [
            { id: "t1", has_live_process: true },
            { id: "t2", has_live_process: true },
          ],
        },
      ],
      sidebar: { groups: [] },
    }
    const mod = await loadStore()
    mod.selectTab("s1", "t1")

    // Somebody else's promotion: t2 holds the slot and t1 is gone.
    spineBody = {
      ...(spineBody as { projects: unknown; sidebar: unknown }),
      sessions: [
        {
          ...(spineBody as { sessions: Record<string, unknown>[] }).sessions[0],
          slot_tab_id: "t2",
          tabs: [{ id: "t2", has_live_process: true }],
        },
      ],
    }
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().selectedTarget).toEqual({
        kind: "agent",
        sessionId: "s1",
        tabId: "t2",
      })
    })
    expect(
      mod.routeHash({
        target: mod.getSnapshot().selectedTarget,
        changes: false,
        editor: null,
        standalone: false,
      }),
    ).toBe("#/agent/s1")
    // The fallback is out loud: the address bar was rewritten, not just the
    // selection.
    await vi.waitFor(() => {
      expect(fakeLocation.hash).toBe("#/agent/s1")
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

  it("focuses a started tab immediately and latches it only once the server accepts", async () => {
    spineBody = spineWithExtraTab(false)
    const mod = await loadStore()
    mod.startDormantTab("s1", "b2")
    // Selection is what the press MEANT, so it lands on the press itself.
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "b2",
    })
    // The LAUNCH is the server's, and the latch waits for it: until then the
    // card stays up, which is what keeps the pane from opening the socket path,
    // which refuses a tab whose last run failed.
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
    await vi.waitFor(() => {
      expect(
        find(
          (u, init) =>
            u === "/api/v1/sessions/s1/tabs/b2/start" && init?.method === "POST",
        ),
      ).toBeDefined()
    })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
    })
  })

  // The start request must never move the user: navigating away while it is in
  // flight has to stick, which it cannot if the answer re-selects the tab.
  it("does not yank the selection back when the start answers after a move away", async () => {
    spineBody = spineWithExtraTab(false)
    const mod = await loadStore()
    mod.startDormantTab("s1", "b2")
    mod.selectTab("s1", "s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
    })
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
  })

  it("clears the started-dormant latch once a tab is live, so a later exit re-shows the card", async () => {
    spineBody = spineWithExtraTab(false)
    const mod = await loadStore()
    // User clicks "Start session" on the dormant extra tab.
    mod.startDormantTab("s1", "b2")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
    })

    // The provider comes up: the latch is no longer needed and gets cleared.
    spineBody = spineWithExtraTab(true)
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
    })

    // The provider later exits: the tab is dormant again with no lingering latch,
    // so a plain refocus would show the card (not force-launch).
    spineBody = spineWithExtraTab(false)
    fireSessionsChanged()
    await vi.waitFor(() => {
      const tab = mod
        .getSnapshot()
        .spine?.sessions[0].tabs?.find((t) => t.id === "b2")
      expect(tab?.has_live_process).toBe(false)
    })
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
  })

  // The latch bridges the press until the launch answers, EITHER WAY. A retry
  // that fails again must put the diagnosis card back rather than leave the
  // latch pinning it out of sight forever.
  it("drops the started-dormant latch when the spine reports the run failed", async () => {
    spineBody = spineWithExtraTab(false)
    const mod = await loadStore()
    mod.startDormantTab("s1", "b2")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
    })

    const failed = spineWithExtraTab(false)
    failed.sessions[0].tabs[1].last_run_failed = true
    spineBody = failed
    fireSessionsChanged()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
    })
  })

  it("handleTabGone clears the started-dormant latch for the gone tab id", async () => {
    spineBody = spineWithExtraTab(false)
    const mod = await loadStore()
    mod.startDormantTab("s1", "b2")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().startedDormantTabs).toContain("b2")
    })
    mod.handleTabGone("b2")
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("b2")
  })

  // A reconnect needs no latch of its own: dispatching a launch is what clears
  // the tab's recorded failure server-side, so the card does not show over a
  // reconnect in the first place.
  it("reconnectSession focuses the session-slot tab without latching it", async () => {
    spineBody = spineWithDormantSlot()
    const mod = await loadStore()
    mod.reconnectSession("s1", true)
    expect(mod.getSnapshot().selectedTarget).toEqual({
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    })
    expect(mod.getSnapshot().startedDormantTabs).not.toContain("s1")
    await vi.waitFor(() => {
      expect(
        find((u, init) => u.endsWith("/reconnect") && init?.method === "POST"),
      ).toBeDefined()
    })
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
