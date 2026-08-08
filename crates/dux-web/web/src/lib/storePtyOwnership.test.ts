import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The per-PTY input-ownership ledger and the server-published spine field it
// combines with. Two sources answer "is another connection driving this
// agent": a MOUNTED TerminalPane's live verdict ("mine"/"elsewhere", the fast
// path, ahead of the next spine refetch), and the spine's per-tab
// `input_owner` (the owning PTY-socket connection id), compared against this
// client's own registered PTY-socket ids. The server field is what lets a hub
// or sidebar row menu gate an agent no pane on this device is attached to.

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

describe("ptyOwnership ledger", () => {
  it("noteAgentPtyOwnership records both verdicts and retires them on unknown", async () => {
    const mod = await import("./store")
    expect(mod.getSnapshot().ptyOwnership).toEqual({})
    mod.noteAgentPtyOwnership("s1", "elsewhere")
    expect(mod.getSnapshot().ptyOwnership).toEqual({ s1: "elsewhere" })
    // Idempotent: repeating the same verdict must not churn the state object.
    const before = mod.getSnapshot().ptyOwnership
    mod.noteAgentPtyOwnership("s1", "elsewhere")
    expect(mod.getSnapshot().ptyOwnership).toBe(before)
    // Taking over flips the verdict in place.
    mod.noteAgentPtyOwnership("s1", "mine")
    expect(mod.getSnapshot().ptyOwnership).toEqual({ s1: "mine" })
    // Unmounting retires the verdict entirely.
    mod.noteAgentPtyOwnership("s1", "unknown")
    expect(mod.getSnapshot().ptyOwnership).toEqual({})
    // Retiring an id that was never set is a no-op, not a crash.
    mod.noteAgentPtyOwnership("ghost", "unknown")
    expect(mod.getSnapshot().ptyOwnership).toEqual({})
  })

  it("noteOwnPtyConnection registers and retires this client's socket ids", async () => {
    const mod = await import("./store")
    expect(mod.getSnapshot().ownPtyConnIds).toEqual({})
    mod.noteOwnPtyConnection("7", true)
    expect(mod.getSnapshot().ownPtyConnIds).toEqual({ "7": true })
    const before = mod.getSnapshot().ownPtyConnIds
    mod.noteOwnPtyConnection("7", true)
    expect(mod.getSnapshot().ownPtyConnIds).toBe(before)
    mod.noteOwnPtyConnection("7", false)
    expect(mod.getSnapshot().ownPtyConnIds).toEqual({})
    mod.noteOwnPtyConnection("ghost", false)
    expect(mod.getSnapshot().ownPtyConnIds).toEqual({})
  })
})

describe("sessionActiveElsewhere", () => {
  type Mod = typeof import("./store")
  type S = ReturnType<Mod["getSnapshot"]>
  const session = (tabs: Array<{ id: string; input_owner?: string }>) =>
    ({ id: "s1", tabs }) as unknown as Parameters<
      Mod["sessionActiveElsewhere"]
    >[1]
  const stateWith = (partial: object) => partial as unknown as S

  it("the local ledger alone disables, on the session slot or any extra tab", async () => {
    const mod = await import("./store")
    const s = session([{ id: "s1" }, { id: "tab-2" }])
    expect(mod.sessionActiveElsewhere(stateWith({}), s)).toBe(false)
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ptyOwnership: { s1: "elsewhere" } }),
        s,
      ),
    ).toBe(true)
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ptyOwnership: { "tab-2": "elsewhere" } }),
        s,
      ),
    ).toBe(true)
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ptyOwnership: { other: "elsewhere" } }),
        s,
      ),
    ).toBe(false)
    // Mocked states elsewhere omit the fields; the selector must not crash.
    expect(mod.sessionActiveElsewhere(stateWith({}), s)).toBe(false)
  })

  it("the server field alone disables, with no pane mounted anywhere", async () => {
    const mod = await import("./store")
    const s = session([
      { id: "s1", input_owner: "42" },
      { id: "tab-2" },
    ])
    // No local verdict, no own socket ids: owned means owned by someone else.
    expect(mod.sessionActiveElsewhere(stateWith({}), s)).toBe(true)
    // An extra tab's owner gates too.
    expect(
      mod.sessionActiveElsewhere(
        stateWith({}),
        session([{ id: "s1" }, { id: "tab-2", input_owner: "9" }]),
      ),
    ).toBe(true)
    // An unowned session gates nothing.
    expect(
      mod.sessionActiveElsewhere(stateWith({}), session([{ id: "s1" }])),
    ).toBe(false)
  })

  it("ownership by one of this client's own connections does not disable", async () => {
    const mod = await import("./store")
    const s = session([{ id: "s1", input_owner: "42" }])
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ownPtyConnIds: { "42": true } }),
        s,
      ),
    ).toBe(false)
    // A different connection of this client does not vouch for this one.
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ownPtyConnIds: { "43": true } }),
        s,
      ),
    ).toBe(true)
  })

  it("a mounted pane's live verdict beats a stale server field, both ways", async () => {
    const mod = await import("./store")
    // Right after a take-over: the spine still names the previous owner, but
    // the pane already knows the PTY is mine. The menu must not stay disabled
    // for the device that just took over.
    expect(
      mod.sessionActiveElsewhere(
        stateWith({ ptyOwnership: { s1: "mine" } }),
        session([{ id: "s1", input_owner: "42" }]),
      ),
    ).toBe(false)
    // Right after losing input: the spine may still name this client, but the
    // pane already saw the handover.
    expect(
      mod.sessionActiveElsewhere(
        stateWith({
          ptyOwnership: { s1: "elsewhere" },
          ownPtyConnIds: { "42": true },
        }),
        session([{ id: "s1", input_owner: "42" }]),
      ),
    ).toBe(true)
  })
})
