import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// THE PTY RETRY GATE IS "VALIDATED", NOT "OPEN".
//
// `eventsSocket.onOpen` fires the run-identity probe as
// `void reloadIfServerChanged()`, an async fetch that resolves later, so
// `conn === "open"` is true well before the check has answered. A PTY socket
// allowed to retry in that window can force-launch a provider on a RESTARTED
// server, which is exactly what the check exists to prevent. This drives the
// real store's own socket callbacks and asserts the signal the gate reads.

// The store fires fetches at import and from `onOpen`; serve them all with an
// empty-but-valid body, and the build endpoint with a stable identity so the run
// never looks changed.
const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/build")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({ version: "development", process: "run-1" }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({
        projects: [],
        sessions: [],
        terminals: [],
        sidebar: { groups: [], agentless_start: null },
      }),
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

vi.stubGlobal("fetch", fetchMock)

let store: typeof import("./store")
let serverValidated: () => boolean

beforeEach(async () => {
  vi.resetModules()
  fetchMock.mockClear()
  // Both imports must come from the SAME module graph as the reset store, or the
  // test would read a signal nothing writes.
  store = await import("./store")
  serverValidated = (await import("./serverValidated")).serverValidated
})

afterEach(() => {
  vi.unstubAllGlobals()
})

/// Let every promise the callback kicked off settle.
const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

describe("the PTY retry gate", () => {
  it("is HELD while the events socket is open but the run check has not answered", () => {
    // The open callback fires the probe and returns; the probe has not resolved.
    store.eventsSocket.onOpen()
    expect(serverValidated()).toBe(false)
  })

  it("is RELEASED once the probe resolves against the same run", async () => {
    store.eventsSocket.onOpen()
    await settle()
    expect(serverValidated()).toBe(true)
  })

  it("is RE-HELD when the events socket closes, because the next open owes a fresh check", async () => {
    store.eventsSocket.onOpen()
    await settle()
    expect(serverValidated()).toBe(true)
    store.eventsSocket.onConn("closed")
    expect(serverValidated()).toBe(false)
  })
})
