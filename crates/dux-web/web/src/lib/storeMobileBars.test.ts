import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Mirror the storeChangesPane test harness: the store module reads
// location/localStorage, registers listeners, and fires a bootstrap fetch on
// import, so stub the minimum for it to settle.
//
// The two mobile-bar preferences (`ui.mobile_top_bar`,
// `ui.mobile_accessory_bar`) ride the GENERIC settings PATCH
// (`PATCH /api/v1/config/settings`) rather than a bespoke endpoint: they are
// pure render gates with no server-side side effect. What the quick toggles
// add is purely client-side: an optimistic override reconciled against the
// next bootstrap exactly like `changesPaneOverride`.

function makeBootstrap(topBar: boolean, accessoryBar: boolean): Bootstrap {
  return {
    available_providers: [],
    macros: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    always_show_tab_strip: false,
    global_env: {},
    status_clear_seconds: 6,
    mobile_top_bar: topBar,
    mobile_accessory_bar: accessoryBar,
  } as Bootstrap
}

let bootstrapBody: Bootstrap = makeBootstrap(true, true)

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
  if (u.includes("/api/v1/config/settings")) {
    return {
      ok: true,
      status: 200,
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
  bootstrapBody = makeBootstrap(true, true)
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

describe("mobile bar visibility", () => {
  it("selectors: override wins, else bootstrap default, else visible", async () => {
    const mod = await loadStore()
    type S = ReturnType<typeof mod.getSnapshot>
    const top = (override: boolean | null, configValue?: boolean) =>
      mod.mobileTopBarVisible({
        mobileTopBarOverride: override,
        bootstrap:
          configValue === undefined ? null : { mobile_top_bar: configValue },
      } as unknown as S)
    const keys = (override: boolean | null, configValue?: boolean) =>
      mod.mobileAccessoryBarVisible({
        mobileAccessoryBarOverride: override,
        bootstrap:
          configValue === undefined
            ? null
            : { mobile_accessory_bar: configValue },
      } as unknown as S)

    // No override and no bootstrap yet (pre-load window) → visible.
    expect(top(null, undefined)).toBe(true)
    expect(keys(null, undefined)).toBe(true)
    // No override → follows the bootstrap value.
    expect(top(null, false)).toBe(false)
    expect(keys(null, false)).toBe(false)
    // An explicit override beats the bootstrap value either way.
    expect(top(false, true)).toBe(false)
    expect(top(true, false)).toBe(true)
    expect(keys(false, true)).toBe(false)
    expect(keys(true, false)).toBe(true)
  })

  it("setMobileBarVisibility(top) sets an optimistic override and PATCHes only that field", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(null)
    await expect(mod.setMobileBarVisibility("top", false)).resolves.toBe(true)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(false)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(null)
    expect(mod.mobileTopBarVisible(mod.getSnapshot())).toBe(false)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({ ui: { mobile_top_bar: false } }),
      }),
    )
  })

  it("setMobileBarVisibility(accessory) sets an optimistic override and PATCHes only that field", async () => {
    const mod = await loadStore()
    await expect(mod.setMobileBarVisibility("accessory", false)).resolves.toBe(
      true,
    )
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(null)
    expect(mod.mobileAccessoryBarVisible(mod.getSnapshot())).toBe(false)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({ ui: { mobile_accessory_bar: false } }),
      }),
    )
  })

  it("restoreMobileBars restores BOTH bars in one PATCH", async () => {
    const mod = await loadStore()
    await mod.setMobileBarVisibility("top", false)
    await mod.setMobileBarVisibility("accessory", false)
    const callsBefore = fetchMock.mock.calls.length
    await expect(mod.restoreMobileBars()).resolves.toBe(true)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(true)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(true)
    expect(mod.mobileTopBarVisible(mod.getSnapshot())).toBe(true)
    expect(mod.mobileAccessoryBarVisible(mod.getSnapshot())).toBe(true)
    // One request carrying both fields, not two requests.
    expect(fetchMock.mock.calls.length).toBe(callsBefore + 1)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_top_bar: true, mobile_accessory_bar: true },
        }),
      }),
    )
  })

  it("a config.changed refetch clears an override once the server confirms it", async () => {
    const mod = await loadStore()
    await mod.setMobileBarVisibility("top", false)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(false)
    bootstrapBody = makeBootstrap(false, true)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().mobileTopBarOverride).toBe(null)
    })
    // The untouched accessory override stays untouched (null).
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(null)
  })

  it("a config.changed refetch keeps an override until the server value matches", async () => {
    const mod = await loadStore()
    await mod.setMobileBarVisibility("accessory", false)
    bootstrapBody = makeBootstrap(true, true)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().bootstrap?.mobile_accessory_bar).toBe(true)
    })
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
  })

  it("rolls the optimistic override back and resolves false when the PATCH fails", async () => {
    const mod = await loadStore()
    fetchMock.mockImplementationOnce(
      async () =>
        ({
          ok: false,
          status: 500,
          json: async () => null,
          text: async () => "disk full",
          headers: { get: () => null },
        }) as unknown as Response,
    )
    await expect(mod.setMobileBarVisibility("top", false)).resolves.toBe(false)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(null)
    expect(mod.mobileTopBarVisible(mod.getSnapshot())).toBe(true)
  })

  it("rolls BOTH overrides back when a restore fails", async () => {
    const mod = await loadStore()
    await mod.setMobileBarVisibility("top", false)
    await mod.setMobileBarVisibility("accessory", false)
    fetchMock.mockImplementationOnce(
      async () =>
        ({
          ok: false,
          status: 500,
          json: async () => null,
          text: async () => "disk full",
          headers: { get: () => null },
        }) as unknown as Response,
    )
    await expect(mod.restoreMobileBars()).resolves.toBe(false)
    expect(mod.getSnapshot().mobileTopBarOverride).toBe(false)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
  })
})
