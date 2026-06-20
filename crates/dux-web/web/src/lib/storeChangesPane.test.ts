import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { ViewModel } from "@/lib/types"

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

// A minimal ViewModel for the reconcile path; onViewModel reads only
// sessions/projects (empty here) plus show_changes_pane. `undefined` models a
// pre-feature server that never sends the field.
function vmWith(showChangesPane: boolean | undefined): ViewModel {
  return {
    sessions: [],
    projects: [],
    ...(showChangesPane === undefined
      ? {}
      : { show_changes_pane: showChangesPane }),
  } as unknown as ViewModel
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

  it("toggleChangesPane sets an optimistic override and persists via the socket", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    // Default: no override, no config default seeded → visible.
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(true)
    // First toggle hides it (optimistic override = false) and tells the server.
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(false)
    expect(spy).toHaveBeenLastCalledWith("set_changes_pane_visible", {
      visible: false,
    })
    // Second toggle shows it again, persisting the new value.
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(true)
    expect(spy).toHaveBeenLastCalledWith("set_changes_pane_visible", {
      visible: true,
    })
  })

  it("onViewModel clears the override once the server confirms the value", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane() // optimistic hide → override = false
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    mod.socket.onViewModel(vmWith(false)) // server confirms hidden
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
  })

  it("onViewModel keeps the override until the server value matches", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane() // override = false
    mod.socket.onViewModel(vmWith(true)) // server still reports visible
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
  })

  it("onViewModel clears the override against a server that omits the field", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane() // override = false
    mod.socket.onViewModel(vmWith(undefined)) // pre-feature server
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
  })

  it("the toggle-remove-git-pane palette command runs the toggle", async () => {
    const mod = await loadStore()
    const { PALETTE_HANDLERS } = await import("./paletteRegistry")
    const spy = vi.spyOn(mod.socket, "sendCommand")
    PALETTE_HANDLERS["toggle-remove-git-pane"]()
    expect(spy).toHaveBeenCalledWith("set_changes_pane_visible", {
      visible: false,
    })
  })
})
