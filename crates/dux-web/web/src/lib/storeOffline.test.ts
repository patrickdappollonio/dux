import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Drives the store's `eventsSocket.onConn` handler (the same one production wires)
// and asserts the sticky `offline` flag that gates the full-screen OfflineOverlay.
// Mirrors the storeChangesPane harness: the store reads location/localStorage,
// registers listeners, and fires a bootstrap fetch on import, so stub the minimum
// to let it settle.

function makeBootstrap(): Bootstrap {
  return {
    available_providers: [],
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
  }
}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => makeBootstrap(),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    ok: true,
    status: 200,
    json: async () => ({ auth: "disabled" }),
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
  vi.stubGlobal("location", { host: "localhost:0", protocol: "http:" })
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
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

describe("offline flag (full-screen offline modal signal)", () => {
  it("stays false through the initial boot connect (connecting, never opened)", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().conn).toBe("connecting")
    expect(mod.getSnapshot().offline).toBe(false)
  })

  it("latches true on a drop and clears only when open again", async () => {
    const mod = await loadStore()
    mod.eventsSocket.onConn("open")
    expect(mod.getSnapshot().offline).toBe(false)

    mod.eventsSocket.onConn("closed")
    expect(mod.getSnapshot().offline).toBe(true)

    mod.eventsSocket.onConn("open")
    expect(mod.getSnapshot().offline).toBe(false)
  })

  it("a reconnect attempt's 'connecting' does NOT clear offline (no flicker)", async () => {
    const mod = await loadStore()
    mod.eventsSocket.onConn("open")
    mod.eventsSocket.onConn("closed")
    expect(mod.getSnapshot().offline).toBe(true)
    // open() re-emits "connecting" on every retry; the modal must stay up.
    mod.eventsSocket.onConn("connecting")
    expect(mod.getSnapshot().offline).toBe(true)
    // ...and only the eventual reopen clears it.
    mod.eventsSocket.onConn("open")
    expect(mod.getSnapshot().offline).toBe(false)
  })

  it("'failed' (auto-retry gave up) keeps offline latched", async () => {
    const mod = await loadStore()
    mod.eventsSocket.onConn("open")
    mod.eventsSocket.onConn("failed")
    expect(mod.getSnapshot().offline).toBe(true)
  })

  it("a connect failure with no prior open still marks offline", async () => {
    const mod = await loadStore()
    // Server went down between serving the page and the socket opening.
    mod.eventsSocket.onConn("closed")
    expect(mod.getSnapshot().offline).toBe(true)
  })
})
