import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Mirror the store test harness: the module reads location/localStorage,
// registers listeners, and fires a bootstrap fetch on import.
// Stub the minimum so the store settles.
//
// `show_changes_pane` moved off the broadcast ViewModel onto the
// `GET /api/v1/bootstrap` document (Phase 2). The optimistic Changes-pane
// override is reconciled when a `config.changed` event refetches bootstrap, not
// on a ViewModel push. Tests drive the bootstrap body via `bootstrapBody`.

function makeBootstrap(showChangesPane: boolean): Bootstrap {
  return {
    available_providers: [],
    macros: [],
    palette_commands: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: showChangesPane,
    global_env: {},
    status_clear_seconds: 6,
  }
}

let bootstrapBody: Bootstrap = makeBootstrap(true)

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => bootstrapBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // The Changes-pane toggle now persists via a REST PUT (Phase 6, was a `/ws`
  // command). Acknowledge it so the optimistic override is not rolled back.
  if (u.includes("/api/v1/ui/changes-pane")) {
    return {
      ok: true,
      status: 204,
      json: async () => null,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    ok: true,
    status: 200,
    json: async () => ({ auth: "disabled" }),
    text: async () => "",
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
  bootstrapBody = makeBootstrap(true)
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
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

describe("Changes-pane visibility", () => {
  it("changesPaneVisible: override wins, else bootstrap default, else visible", async () => {
    const mod = await loadStore()
    type S = ReturnType<typeof mod.getSnapshot>
    const v = (override: boolean | null, configValue?: boolean) =>
      mod.changesPaneVisible({
        changesPaneOverride: override,
        bootstrap:
          configValue === undefined ? null : { show_changes_pane: configValue },
      } as unknown as S)

    // No override and no bootstrap yet (pre-load window) → visible.
    expect(v(null, undefined)).toBe(true)
    // No override → follows the bootstrap default.
    expect(v(null, false)).toBe(false)
    expect(v(null, true)).toBe(true)
    // An explicit per-session override beats the bootstrap default either way.
    expect(v(true, false)).toBe(true)
    expect(v(false, true)).toBe(false)
  })

  it("toggleChangesPane sets an optimistic override and persists via REST", async () => {
    const mod = await loadStore()
    // Bootstrap default is visible; no override → visible.
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(true)
    // First toggle hides it (optimistic override = false) and PUTs the new value.
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(false)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/ui/changes-pane",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ visible: false }),
      }),
    )
    // Second toggle shows it again, persisting the new value.
    mod.toggleChangesPane()
    expect(mod.getSnapshot().changesPaneOverride).toBe(true)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/ui/changes-pane",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ visible: true }),
      }),
    )
  })

  it("a config.changed refetch clears the override once the server confirms", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane() // optimistic hide → override = false
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    // The server persisted the hide and emits config.changed; the refetched
    // bootstrap now reports it hidden, so the override retires.
    bootstrapBody = makeBootstrap(false)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().changesPaneOverride).toBe(null)
    })
  })

  it("a config.changed refetch keeps the override until the server value matches", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane() // override = false
    // Server still reports the pane visible (the persist hasn't taken effect or
    // another client re-showed it): the override must stand.
    bootstrapBody = makeBootstrap(true)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().bootstrap?.show_changes_pane).toBe(true)
    })
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
  })

  it("the toggle-remove-git-pane palette command runs the toggle", async () => {
    await loadStore() // boot the store so the palette handler module resolves
    const { PALETTE_HANDLERS } = await import("./paletteRegistry")
    PALETTE_HANDLERS["toggle-remove-git-pane"]()
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/ui/changes-pane",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ visible: false }),
      }),
    )
  })
})
