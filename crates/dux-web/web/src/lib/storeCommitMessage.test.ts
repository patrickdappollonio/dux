import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs a `DuxSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly (mirrors storeWatchChangedFiles.test.ts's setup).

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

describe("commit-message routing", () => {
  it("fills the draft when the message matches the open dialog's session", async () => {
    const mod = await loadStore()
    mod.openCommit("s1")
    mod.socket.onCommitMessage("s1", "Generated for s1")
    expect(mod.getSnapshot().commitDraft).toBe("Generated for s1")
  })

  it("ignores a message for a different session (anti-misroute)", async () => {
    const mod = await loadStore()
    // The dialog is open for s1, but a message for s2 arrives (a second tab, or
    // the user switched). It must NOT clobber the s1 draft.
    mod.openCommit("s1")
    mod.setCommitDraft("hand-typed for s1")
    mod.socket.onCommitMessage("s2", "Generated for s2")
    expect(mod.getSnapshot().commitDraft).toBe("hand-typed for s1")
  })

  it("drops a message when no dialog is open", async () => {
    const mod = await loadStore()
    // commitTarget is null by default. A late message (dialog already closed)
    // must be dropped, leaving the empty draft untouched.
    mod.socket.onCommitMessage("s1", "Generated for s1")
    expect(mod.getSnapshot().commitTarget).toBeNull()
    expect(mod.getSnapshot().commitDraft).toBe("")
  })
})
