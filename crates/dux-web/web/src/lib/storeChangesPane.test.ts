import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Mirror the store test harness (see storeWatchChangedFiles.test.ts): the module
// reads location/localStorage, registers listeners, and fires a boot probe on
// import. Stub the minimum and steer the probe to auth-off so the store settles
// before each test.

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

describe("Changes-pane visibility", () => {
  it("changesPaneVisible: override wins, else config default, else visible", async () => {
    const mod = await loadStore()
    type S = ReturnType<typeof mod.getSnapshot>
    const v = (override: boolean | null, configValue?: boolean) =>
      mod.changesPaneVisible({
        changesPaneOverride: override,
        viewModel:
          configValue === undefined ? null : { show_changes_pane: configValue },
      } as unknown as S)

    // No override and no config value (older servers omit it) → visible.
    expect(v(null, undefined)).toBe(true)
    // No override → follows the config default the ViewModel carries.
    expect(v(null, false)).toBe(false)
    expect(v(null, true)).toBe(true)
    // An explicit per-session override beats the config default either way.
    expect(v(true, false)).toBe(true)
    expect(v(false, true)).toBe(false)
  })

  it("toggleChangesPane flips an explicit override off the effective value", async () => {
    const mod = await loadStore()
    // Default: no override, no config default seeded → visible.
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(true)
    // First toggle hides it (explicit override = false).
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(false)
    // Second toggle shows it again.
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(true)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(true)
  })
})
