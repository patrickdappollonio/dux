// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "./bootstrapApi"
import type { Spine } from "./workspaceApi"

// Drives the real store's spine-apply path under jsdom and asserts the browser-tab
// "needs attention" chrome: a `(N) ` count prefix on the document title. The pure
// count/format helpers are unit-tested in attention.test.ts; this proves the store
// actually wires the live spine → refreshAttentionChrome → document.title. The
// canvas-composited favicon dot is not asserted (jsdom has no 2D context); the
// clean-favicon restore path is covered in favicon.dom.test.tsx.

const BASE_TITLE = "workbench"

function makeBootstrap(overrides: Partial<Bootstrap> = {}): Bootstrap {
  return {
    available_providers: ["claude", "codex"],
    macros: [],
    welcome_tips: ["tip one"],
    dux_version: "v1.2.3",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    always_show_tab_strip: false,
    global_env: {},
    status_clear_seconds: 6,
    title: BASE_TITLE,
    ...overrides,
  }
}

function makeSpine(overrides: Partial<Spine> = {}): Spine {
  return {
    projects: [],
    sessions: [],
    sidebar: { groups: [], agentless_start: null },
    ...overrides,
  }
}

function session(id: string, needsAttention: boolean): Spine["sessions"][number] {
  return {
    id,
    project_id: "p1",
    terminals: [],
    tabs: [],
    needs_attention: needsAttention,
  } as unknown as Spine["sessions"][number]
}

let bootstrapBody: Bootstrap = makeBootstrap()
let spineBody: Spine = makeSpine()

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => bootstrapBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
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
  bootstrapBody = makeBootstrap()
  spineBody = makeSpine()
  vi.spyOn(console, "warn").mockImplementation(() => {})
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  document.head.innerHTML = ""
  // The surface is read from the hash at store boot, so every test starts on
  // the ordinary workspace address unless it says otherwise.
  window.location.hash = ""
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  document.head.innerHTML = ""
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

async function pushSpine(
  mod: Awaited<ReturnType<typeof loadStore>>,
  body: Spine,
): Promise<void> {
  const prev = mod.getSnapshot().spine
  spineBody = body
  mod.eventsSocket.onEvent({ event: "sessions.changed" })
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBe(prev)
  })
}

describe("store attention chrome (browser-tab count)", () => {
  it("leaves the title bare when no agent needs attention", async () => {
    spineBody = makeSpine({ sessions: [session("s1", false)] })
    await loadStore()
    expect(document.title).toBe(BASE_TITLE)
  })

  it("prefixes the count when an agent needs attention, and clears it again", async () => {
    spineBody = makeSpine({ sessions: [session("s1", false)] })
    const mod = await loadStore()
    expect(document.title).toBe(BASE_TITLE)

    // One flagged agent → "(1) " prefix.
    await pushSpine(mod, makeSpine({ sessions: [session("s1", true)] }))
    expect(document.title).toBe(`(1) ${BASE_TITLE}`)

    // Flag cleared → prefix gone.
    await pushSpine(mod, makeSpine({ sessions: [session("s1", false)] }))
    expect(document.title).toBe(BASE_TITLE)
  })

  it("never counts in the standalone editor tab, however many agents are flagged", async () => {
    // The editor tab is not the thing needing attention, so its title keeps the
    // "Editor" prefix and never grows a "(N) " one.
    window.location.hash = "#/editor/agent/s1"
    spineBody = makeSpine({ sessions: [session("s1", true), session("s2", true)] })
    const mod = await loadStore()
    expect(mod.getSnapshot().standaloneEditor).toBe(true)
    expect(document.title).toBe(`Editor — ${BASE_TITLE}`)

    await pushSpine(
      mod,
      makeSpine({ sessions: [session("s1", true), session("s2", true)] }),
    )
    expect(document.title).toBe(`Editor — ${BASE_TITLE}`)
  })
})
