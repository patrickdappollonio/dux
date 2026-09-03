import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Sonner is mocked so the toast policy is assertable: a successful bar write
// must raise NO toast of any kind (the bar visibly moving is the feedback,
// and the PATCH asks the server for quiet so no status toast arrives either),
// while a failed write must still toast the error (a silent rollback would be
// a silent failure).
const toastMock = Object.assign(vi.fn(), {
  error: vi.fn(),
  success: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
  loading: vi.fn(),
  dismiss: vi.fn(),
  custom: vi.fn(),
})
vi.mock("sonner", () => ({ toast: toastMock }))

// Mirror the storeChangesPane test harness: the store module reads
// location/localStorage, registers listeners, and fires a bootstrap fetch on
// import, so stub the minimum for it to settle.
//
// The terminal-keys bar preference (`ui.mobile_accessory_bar`) rides the
// GENERIC settings PATCH (`PATCH /api/v1/config/settings`) rather than a
// bespoke endpoint: it is a pure render gate with no server-side side effect.
// What the quick toggle adds is purely client-side: an optimistic override
// reconciled against the next bootstrap exactly like `changesPaneOverride`.
//
// It is the ONLY hideable bar. A `ui.mobile_top_bar` sibling hid the phone's
// top bar and is gone: theater mode hides that chrome and carries its own way
// back, and two flows for hiding one header could disagree about what was on
// screen.

function makeBootstrap(accessoryBar: boolean): Bootstrap {
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
    mobile_accessory_bar: accessoryBar,
  } as Bootstrap
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

describe("terminal-keys bar visibility", () => {
  it("the store publishes no top-bar preference at all", async () => {
    const mod = await loadStore()
    // The retired preference must not come back as a half-wired selector: it
    // is not exported, and the snapshot carries no override slot for it.
    expect("mobileTopBarVisible" in mod).toBe(false)
    expect("mobileTopBarOverride" in mod.getSnapshot()).toBe(false)
  })

  it("selector: override wins, else bootstrap default, else visible", async () => {
    const mod = await loadStore()
    type S = ReturnType<typeof mod.getSnapshot>
    const keys = (override: boolean | null, configValue?: boolean) =>
      mod.mobileAccessoryBarVisible({
        mobileAccessoryBarOverride: override,
        bootstrap:
          configValue === undefined
            ? null
            : { mobile_accessory_bar: configValue },
      } as unknown as S)

    // No override and no bootstrap yet (pre-load window) → visible.
    expect(keys(null, undefined)).toBe(true)
    // No override → follows the bootstrap value.
    expect(keys(null, false)).toBe(false)
    // An explicit override beats the bootstrap value either way.
    expect(keys(false, true)).toBe(false)
    expect(keys(true, false)).toBe(true)
  })

  it("setAccessoryBarVisibility sets an optimistic override and PATCHes only that field", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(null)
    await expect(mod.setAccessoryBarVisibility(false)).resolves.toBe(true)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
    expect(mod.mobileAccessoryBarVisible(mod.getSnapshot())).toBe(false)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_accessory_bar: false },
          quiet: true,
        }),
      }),
    )
  })

  // Restoring is the same write as hiding: the input ⋯ menu's item names the
  // direction it will move the bar in and writes the same field either way.
  it("restores the bar through the same write that hid it", async () => {
    const mod = await loadStore()
    await mod.setAccessoryBarVisibility(false)
    await expect(mod.setAccessoryBarVisibility(true)).resolves.toBe(true)
    expect(mod.mobileAccessoryBarVisible(mod.getSnapshot())).toBe(true)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_accessory_bar: true },
          quiet: true,
        }),
      }),
    )
  })

  it("a config.changed refetch clears an override once the server confirms it", async () => {
    const mod = await loadStore()
    await mod.setAccessoryBarVisibility(false)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
    bootstrapBody = makeBootstrap(false)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(null)
    })
  })

  it("a config.changed refetch keeps an override until the server value matches", async () => {
    const mod = await loadStore()
    await mod.setAccessoryBarVisibility(false)
    bootstrapBody = makeBootstrap(true)
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.getSnapshot().bootstrap?.mobile_accessory_bar).toBe(true)
    })
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
  })

  it("a successful bar write raises no client toast (success is silence)", async () => {
    const mod = await loadStore()
    toastMock.mockClear()
    toastMock.error.mockClear()
    toastMock.success.mockClear()
    toastMock.info.mockClear()
    toastMock.custom.mockClear()
    await expect(mod.setAccessoryBarVisibility(false)).resolves.toBe(true)
    await expect(mod.setAccessoryBarVisibility(true)).resolves.toBe(true)
    expect(toastMock).not.toHaveBeenCalled()
    expect(toastMock.error).not.toHaveBeenCalled()
    expect(toastMock.success).not.toHaveBeenCalled()
    expect(toastMock.info).not.toHaveBeenCalled()
    expect(toastMock.custom).not.toHaveBeenCalled()
  })

  it("a failed bar write still toasts the error (a silent rollback is a silent failure)", async () => {
    const mod = await loadStore()
    toastMock.error.mockClear()
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
    await expect(mod.setAccessoryBarVisibility(false)).resolves.toBe(false)
    // With the duration dux's policy gives an error: four times the 6s
    // default. Before every raise went through `lib/notify.ts` this was
    // sonner's own bare 4000ms.
    expect(toastMock.error).toHaveBeenCalledWith("disk full", {
      duration: 24000,
    })
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
    await expect(mod.setAccessoryBarVisibility(false)).resolves.toBe(false)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(null)
    expect(mod.mobileAccessoryBarVisible(mod.getSnapshot())).toBe(true)
  })

  it("a late failure of an overtaken write does not clobber a newer override", async () => {
    // Tap-tap where the FIRST write fails after the SECOND already landed:
    // hide (slow, will fail), then show (fast, succeeds). The first call's
    // rollback must notice its value was overtaken and leave the newer
    // override alone, otherwise the bar visibly snaps back to a state the
    // user already corrected.
    const mod = await loadStore()
    let rejectFirst!: (e: Error) => void
    fetchMock.mockImplementationOnce(
      () =>
        new Promise((_, reject) => {
          rejectFirst = reject
        }) as unknown as Promise<Response>,
    )
    const first = mod.setAccessoryBarVisibility(false)
    // The second tap goes through the default (succeeding) fetch mock.
    await expect(mod.setAccessoryBarVisibility(true)).resolves.toBe(true)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(true)
    rejectFirst(new Error("boom"))
    await expect(first).resolves.toBe(false)
    // The overtaken failure must NOT roll the override back to the first
    // call's captured previous value (null).
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(true)
  })

  it("rolls a failed restore back to hidden", async () => {
    const mod = await loadStore()
    await mod.setAccessoryBarVisibility(false)
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
    await expect(mod.setAccessoryBarVisibility(true)).resolves.toBe(false)
    expect(mod.getSnapshot().mobileAccessoryBarOverride).toBe(false)
  })
})
