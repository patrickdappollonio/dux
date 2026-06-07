import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs a `DuxSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly (mirrors storeMacros.test.ts's setup).

const fetchMock = vi.fn(async () => {
  return {
    status: 200,
    json: async () => ({ auth: "disabled" }),
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
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  return mod
}

describe("changed-files watch wiring", () => {
  it("selectSession watches the selected session's worktree", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.selectSession("s1")
    expect(spy).toHaveBeenCalledWith("watch_changed_files", { session_id: "s1" })
  })

  it("selectSession(null) clears the watch", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.selectSession(null)
    expect(spy).toHaveBeenCalledWith("watch_changed_files", {
      session_id: null,
    })
  })

  it("selectTerminal watches the PARENT session's worktree", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.selectTerminal("term-9", "s2")
    expect(spy).toHaveBeenCalledWith("watch_changed_files", { session_id: "s2" })
  })

  it("re-watches the selected session when the socket (re)connects", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const spy = vi.spyOn(mod.socket, "sendCommand")
    // Simulate a reconnect: the socket reports it is open again.
    mod.socket.onConn("open")
    expect(spy).toHaveBeenCalledWith("watch_changed_files", { session_id: "s1" })
  })

  it("does not re-watch on connect when nothing is selected", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.socket.onConn("open")
    expect(spy).not.toHaveBeenCalledWith(
      "watch_changed_files",
      expect.anything(),
    )
  })
})
