import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Since Phase 6 the events socket (`/ws/events`) is the only JSON socket, so it
// owns the connection-state UX the retired `DuxSocket` used to: when its reconnect
// loop exhausts its budget it signals "failed", and the store rechecks auth — a
// 401 means the session is gone (drop to the login screen), a still-authed
// `/api/me` means a genuine network blip (restart the socket, stay authed).
//
// The boot probe and the recheck both GET `/api/me`, so the mock serves it from a
// mutable `meResponse` that the test flips after the authed boot settles.

let meResponse: { status: number; body?: unknown } = {
  status: 200,
  body: { username: "alice" },
}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/me")) {
    return {
      status: meResponse.status,
      ok: meResponse.status >= 200 && meResponse.status < 300,
      json: async () => meResponse.body ?? null,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // bootstrap / spine / anything else: succeed with an empty-ish body.
  return {
    status: 200,
    ok: true,
    json: async () => ({}),
    text: async () => "{}",
    headers: { get: () => null },
  } as unknown as Response
})

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 0
  constructor() {
    FakeWebSocket.instances.push(this)
  }
  close() {}
  send() {}
}

beforeEach(() => {
  meResponse = { status: 200, body: { username: "alice" } }
  FakeWebSocket.instances = []
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:0", hash: "" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {}, replaceState: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadAuthed() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).toBe("authed")
  })
  return mod
}

describe("events-socket-driven auth recovery", () => {
  it("a 'failed' socket while authed + a 401 recheck drops to the login screen", async () => {
    const mod = await loadAuthed()
    // The session expired/was revoked: the recheck `/api/me` now 401s.
    meResponse = { status: 401 }
    mod.eventsSocket.onConn("failed")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().auth.phase).toBe("anonymous")
    })
  })

  it("a 'failed' socket while authed but still-authed recheck restarts the socket", async () => {
    const mod = await loadAuthed()
    const before = FakeWebSocket.instances.length
    // Still authed: a genuine network blip, not a logout.
    meResponse = { status: 200, body: { username: "alice" } }
    mod.eventsSocket.onConn("failed")
    await vi.waitFor(() => {
      // The recheck restarts the events socket (a new WebSocket is constructed).
      expect(FakeWebSocket.instances.length).toBeGreaterThan(before)
    })
    expect(mod.getSnapshot().auth.phase).toBe("authed")
  })

  it("caps the authed reconnect storm after repeated still-authed rechecks", async () => {
    // A non-auth upgrade failure (proxy not forwarding Upgrade, TLS) keeps the
    // socket from ever opening, but `/api/me` stays authed. Without a cap the loop
    // is unbounded (failed → recheck → connect → reset → repeat). The cap (3) lets
    // a few auto-restarts through, then stops auto-connecting (FakeWebSocket here
    // never opens, so the success-reset never fires and cycles only accumulate).
    const mod = await loadAuthed()
    meResponse = { status: 200, body: { username: "alice" } }
    const baseline = FakeWebSocket.instances.length
    // The first three 'failed' edges each auto-restart the socket (+1 each).
    for (let i = 0; i < 3; i++) {
      mod.eventsSocket.onConn("failed")
      await vi.waitFor(() => {
        expect(FakeWebSocket.instances.length).toBe(baseline + i + 1)
      })
    }
    // The fourth exceeds the cap: the recheck runs but must NOT connect() again.
    mod.eventsSocket.onConn("failed")
    await new Promise((r) => setTimeout(r, 0))
    await new Promise((r) => setTimeout(r, 0))
    expect(FakeWebSocket.instances.length).toBe(baseline + 3)
    // The app stays authed with the manual Reconnect affordance available.
    expect(mod.getSnapshot().auth.phase).toBe("authed")
  })

  it("manual reconnect() still works after the cap is hit", async () => {
    const mod = await loadAuthed()
    meResponse = { status: 200, body: { username: "alice" } }
    const baseline = FakeWebSocket.instances.length
    for (let i = 0; i < 4; i++) {
      mod.eventsSocket.onConn("failed")
      await new Promise((r) => setTimeout(r, 0))
      await new Promise((r) => setTimeout(r, 0))
    }
    const capped = FakeWebSocket.instances.length
    expect(capped).toBe(baseline + 3) // auto-restarts stopped at the cap
    // The user hits Reconnect: connect() is called directly, never gated by the
    // cap, so a fresh socket is always constructed.
    mod.reconnect()
    expect(FakeWebSocket.instances.length).toBe(capped + 1)
  })
})
