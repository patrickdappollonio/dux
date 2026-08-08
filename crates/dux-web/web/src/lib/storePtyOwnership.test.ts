import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The per-PTY "another device owns input" ledger. TerminalPane is the only
// reporter: it knows a PTY's ownership from `pty.owner` handovers on its own
// socket, and the store keeps the verdicts so surfaces OUTSIDE the pane (the
// agent ⋯ menu) can gate mutating actions. Ownership is knowable ONLY for
// PTYs this client is attached to, so an unmounted pane clears its entry
// rather than leaving a stale claim behind.

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
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({}),
      text: async () => "",
      headers: { get: () => null },
    })),
  )
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("ptyOwnedElsewhere ledger", () => {
  it("noteAgentPtyOwnership records only owned-elsewhere ids and clears them again", async () => {
    const mod = await import("./store")
    expect(mod.getSnapshot().ptyOwnedElsewhere).toEqual({})
    mod.noteAgentPtyOwnership("s1", true)
    expect(mod.getSnapshot().ptyOwnedElsewhere).toEqual({ s1: true })
    // Idempotent: repeating the same verdict must not churn the state object.
    const before = mod.getSnapshot().ptyOwnedElsewhere
    mod.noteAgentPtyOwnership("s1", true)
    expect(mod.getSnapshot().ptyOwnedElsewhere).toBe(before)
    // Regaining ownership (or unmounting) removes the entry entirely.
    mod.noteAgentPtyOwnership("s1", false)
    expect(mod.getSnapshot().ptyOwnedElsewhere).toEqual({})
    // Clearing an id that was never set is a no-op, not a crash.
    mod.noteAgentPtyOwnership("ghost", false)
    expect(mod.getSnapshot().ptyOwnedElsewhere).toEqual({})
  })

  it("sessionActiveElsewhere matches the session slot and every extra tab id", async () => {
    const mod = await import("./store")
    type S = ReturnType<typeof mod.getSnapshot>
    const session = {
      id: "s1",
      tabs: [{ id: "s1" }, { id: "tab-2" }],
    } as unknown as Parameters<typeof mod.sessionActiveElsewhere>[1]
    const stateWith = (owned: Record<string, true>) =>
      ({ ptyOwnedElsewhere: owned }) as unknown as S
    expect(mod.sessionActiveElsewhere(stateWith({}), session)).toBe(false)
    expect(mod.sessionActiveElsewhere(stateWith({ s1: true }), session)).toBe(
      true,
    )
    expect(
      mod.sessionActiveElsewhere(stateWith({ "tab-2": true }), session),
    ).toBe(true)
    expect(
      mod.sessionActiveElsewhere(stateWith({ other: true }), session),
    ).toBe(false)
    // Mocked states elsewhere omit the field; the selector must not crash.
    expect(
      mod.sessionActiveElsewhere({} as unknown as S, session),
    ).toBe(false)
  })
})
