import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Asserts that the store boots directly into the workspace with no /api/me
// round-trip and no login step. The server is trusted-local (no auth gate), so
// the boot function connects the events socket and fetches bootstrap+spine
// immediately, synchronously, at module load.

const requestedUrls: string[] = []

const fetchMock = vi.fn(async (url: string) => {
  requestedUrls.push(String(url))
  return {
    ok: true,
    status: 200,
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
  requestedUrls.length = 0
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

describe("no-auth boot", () => {
  it("settles booted=true without any /api/me request", async () => {
    const mod = await import("./store")
    // booted is set synchronously at module load, so no vi.waitFor needed.
    expect(mod.getSnapshot().booted).toBe(true)
    // No /api/me call should have been made.
    expect(requestedUrls.some((u) => u.includes("/api/me"))).toBe(false)
  })

  it("connects the events socket at boot (no login step)", async () => {
    await import("./store")
    // The boot() function calls eventsSocket.connect() synchronously,
    // which constructs a WebSocket.
    expect(FakeWebSocket.instances.length).toBeGreaterThan(0)
  })

  it("initiates bootstrap and spine fetches at boot", async () => {
    await import("./store")
    await vi.waitFor(() => {
      expect(requestedUrls.some((u) => u.includes("/api/v1/bootstrap"))).toBe(true)
      expect(requestedUrls.some((u) => u.includes("/api/v1/spine"))).toBe(true)
    })
  })
})
