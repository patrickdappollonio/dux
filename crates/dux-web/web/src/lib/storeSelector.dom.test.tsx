// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

// `useDux` re-renders its consumer on every `setState` anywhere in the app.
// `useDuxSelector` re-renders only when the value it selects moves, which is
// what keeps a pane with a very long list from re-rendering on every unrelated
// store change. Drives the real store under jsdom.

const okJson = (body: unknown) =>
  ({
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => "",
    headers: { get: () => null },
  }) as unknown as Response

let spineBody: unknown = { projects: [], sessions: [], sidebar: { groups: [] } }

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/workspace")) return okJson(spineBody)
  if (u.includes("/changes")) return new Promise<Response>(() => {})
  return okJson({})
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
  vi.spyOn(console, "warn").mockImplementation(() => {})
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  window.location.hash = ""
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

describe("useDuxSelector", () => {
  it("skips a store change that leaves the selected slice alone, and follows one that moves it", async () => {
    const mod = await loadStore()
    let renders = 0
    const { result } = renderHook(() => {
      renders += 1
      return mod.useDuxSelector((state) => state.changes)
    })
    const before = renders
    const firstChanges = result.current

    // An unrelated change: a fresh workspace spine.
    const prevSpine = mod.getSnapshot().spine
    spineBody = { projects: [], sessions: [], sidebar: { groups: [] } }
    await act(async () => {
      mod.eventsSocket.onEvent({ event: "sessions.changed" })
      await vi.waitFor(() => {
        expect(mod.getSnapshot().spine).not.toBe(prevSpine)
      })
    })
    expect(renders).toBe(before)

    // A change to the slice itself.
    act(() => mod.selectSession("s1"))
    expect(renders).toBeGreaterThan(before)
    expect(result.current).not.toBe(firstChanges)
    expect(result.current.sessionId).toBe("s1")
  })
})
