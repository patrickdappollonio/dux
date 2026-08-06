import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The Changes pane's "Refresh changes" action. The important property is that it
// FORCES a recompute server-side before it re-reads: the plain `refreshChanges()`
// only re-GETs, and the server answers a GET from its cache, so a file the user
// changed from a terminal would not appear and the menu item would look like it
// worked while changing nothing.

interface Recorded {
  url: string
  method: string
}

let calls: Recorded[] = []

function okJson(body: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => null },
  }
}

const fetchMock = vi.fn(async (url: string, init?: { method?: string }) => {
  const u = String(url)
  calls.push({ url: u, method: init?.method ?? "GET" })
  if (u.includes("/git/refresh-changes")) {
    return { ok: true, status: 200, text: async () => "" } as unknown as Response
  }
  if (u.includes("/changes")) {
    return okJson({ rev: 7, staged: [], unstaged: [] }) as unknown as Response
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
    expect(mod.getSnapshot().booted).toBe(true)
  })
  return mod
}

describe("forceRefreshChanges", () => {
  it("POSTs the forcing route before it re-GETs the changed files", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().changes.phase).toBe("loaded")
    })
    calls = []

    await mod.forceRefreshChanges()
    await vi.waitFor(() => {
      expect(calls.length).toBeGreaterThanOrEqual(2)
    })

    expect(calls[0]).toEqual({
      url: "/api/v1/sessions/s1/git/refresh-changes",
      method: "POST",
    })
    expect(calls[1].url).toBe("/api/v1/sessions/s1/changes")
    expect(calls[1].method).toBe("GET")
  })

  it("does nothing when no session is selected", async () => {
    const mod = await loadStore()
    mod.selectSession(null)
    calls = []
    await mod.forceRefreshChanges()
    expect(calls).toEqual([])
  })

  it("still re-reads when the forcing POST fails, and reports the failure", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().changes.phase).toBe("loaded")
    })
    calls = []
    fetchMock.mockImplementationOnce(async (url: string) => {
      calls.push({ url: String(url), method: "POST" })
      return {
        ok: false,
        status: 500,
        text: async () => "git exploded",
      } as unknown as Response
    })

    await expect(mod.forceRefreshChanges()).rejects.toThrow(/git exploded/)
    await vi.waitFor(() => {
      expect(calls.some((c) => c.url.endsWith("/changes"))).toBe(true)
    })
  })
})
