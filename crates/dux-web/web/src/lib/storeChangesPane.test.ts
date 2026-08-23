import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Mirror the store test harness: the module reads location/localStorage,
// registers listeners, and fires a bootstrap fetch on import.
// Stub the minimum so the store settles.
//
// `show_changes_pane` moved off the broadcast ViewModel onto the
// `GET /api/v1/bootstrap` document. The optimistic Changes-pane
// override is reconciled when a `config.changed` event refetches bootstrap, not
// on a ViewModel push. Tests drive the bootstrap body via `bootstrapBody`.

function makeBootstrap(showChangesPane: boolean): Bootstrap {
  return {
    available_providers: [],
    macros: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: showChangesPane,
    always_show_tab_strip: false,
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
  // The Changes-pane toggle persists via a REST PUT. Acknowledge it so the
  // optimistic override is not rolled back.
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

  // The customize-webapp dialog gates its close on these resolved booleans, so
  // the real implementations (not just component-level mocks) must resolve
  // true on success and false (never reject) on failure.
  it("setChangesPaneVisibility resolves true on success and false on failure, rolling back", async () => {
    const mod = await loadStore()
    await expect(mod.setChangesPaneVisibility(false)).resolves.toBe(true)
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)

    // The PUT fails: the promise resolves false and the optimistic override
    // rolls back so the pane doesn't strand in the toggled state.
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
    await expect(mod.setChangesPaneVisibility(true)).resolves.toBe(false)
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
  })

  it("setInstanceIdentity resolves true on success and false on failure", async () => {
    const mod = await loadStore()
    await expect(
      mod.setInstanceIdentity({ title: "prod dux", favicon: "blue" }),
    ).resolves.toBe(true)

    fetchMock.mockImplementationOnce(
      async () =>
        ({
          ok: false,
          status: 400,
          json: async () => null,
          text: async () => "bad favicon",
          headers: { get: () => null },
        }) as unknown as Response,
    )
    await expect(
      mod.setInstanceIdentity({ title: "prod dux", favicon: "nope" }),
    ).resolves.toBe(false)
  })

  // `toggleChangesPane` used to be reachable from the web command palette's
  // "toggle-remove-git-pane" entry. That surface is gone (the Changes pane is a
  // Preferences row now), but the action itself is still live: the Changes
  // actions menu calls it. Drive it directly.
  it("toggleChangesPane flips the pane's visibility", async () => {
    const mod = await loadStore()
    mod.toggleChangesPane()
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/ui/changes-pane",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ visible: false }),
      }),
    )
  })
})

// The pane could be dragged to zero width, and the zero was nobody's business:
// the preference still said "visible", so the header's reopen button (gated on
// the preference alone) stayed away, and the pane's own hide item was inside the
// zero-width pane. The pane was unreachable until a reload. These are the pure
// halves of the fix; the panel wiring is asserted in App.test.tsx.
describe("Changes-pane width", () => {
  it("setChangesPanePercent clamps to 0..100 and drops a no-op write", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().changesPanePercent).toBe(
      mod.CHANGES_PANE_DEFAULT_PERCENT,
    )

    mod.setChangesPanePercent(42.5)
    expect(mod.getSnapshot().changesPanePercent).toBe(42.5)

    // Writing the same value again must not produce a new snapshot: this
    // setter runs on every pointer move of a divider drag.
    const before = mod.getSnapshot()
    mod.setChangesPanePercent(42.5)
    expect(mod.getSnapshot()).toBe(before)

    mod.setChangesPanePercent(-10)
    expect(mod.getSnapshot().changesPanePercent).toBe(0)
    mod.setChangesPanePercent(1000)
    expect(mod.getSnapshot().changesPanePercent).toBe(100)
  })

  it("isChangesPaneDragCollapse fires only on a measured expanded-to-zero step", async () => {
    const mod = await loadStore()
    // A real drag-collapse: it was open, now it is at the collapsedSize (0%,
    // plus an epsilon for float slop, the same threshold isExplorerCollapsed uses).
    expect(mod.isChangesPaneDragCollapse(0, 26)).toBe(true)
    expect(mod.isChangesPaneDragCollapse(0.4, 14)).toBe(true)
    // Not a collapse: still open, or already collapsed (so nothing changed).
    expect(mod.isChangesPaneDragCollapse(14, 26)).toBe(false)
    expect(mod.isChangesPaneDragCollapse(0, 0)).toBe(false)
    // The FIRST report of a panel's life has no previous size. Treating that as
    // a collapse would hide the pane during mount, before anything is measured.
    expect(mod.isChangesPaneDragCollapse(0, undefined)).toBe(false)
  })

  it("changesPaneCollapseStep latches a pointer collapse and commits only at release", async () => {
    const mod = await loadStore()
    const step = (
      percent: number,
      prevPercent: number | undefined,
      pointerDown: boolean,
      armed: boolean,
    ) =>
      mod.changesPaneCollapseStep({
        percent,
        prevPercent,
        pointerDown,
        armed,
        reshowPending: false,
      })

    // Mid-drag the write waits: flipping the preference here unmounts the panel
    // under the library while the pointer is still down.
    expect(step(0, 26, true, false)).toBe("arm")
    // The same collapse with nothing held down (a keyboard resize of the
    // separator) has no gesture to wait for.
    expect(step(0, 26, false, false)).toBe("commit")
    // Dragged back out before release: the escape hatch. Releasing now keeps
    // the pane.
    expect(step(20, 0, true, true)).toBe("disarm")
    // Still collapsed and still held: nothing to do, and re-arming would be a
    // second write's worth of noise.
    expect(step(0, 0, true, true)).toBe("none")
    // An ordinary resize, armed or not, says nothing.
    expect(step(18, 26, true, false)).toBe("none")
    // A panel's first report has no previous size; it is not a collapse.
    expect(step(0, undefined, true, false)).toBe("none")
  })

  // A pane coming back from hidden re-mounts into whatever layout the library
  // cached for the two-panel group, and for a pane that LEFT at zero that is a
  // zero. The panel reports that mount width like any other resize, with no
  // pointer down, which is the exact shape of a keyboard collapse. Believing it
  // would hide the pane during the act of showing it, so the user's click on
  // "Show Changes pane" would look like it did nothing.
  it("changesPaneCollapseStep believes nothing a re-showing pane reports", async () => {
    const mod = await loadStore()
    const step = (
      percent: number,
      prevPercent: number | undefined,
      pointerDown: boolean,
      armed: boolean,
    ) =>
      mod.changesPaneCollapseStep({
        percent,
        prevPercent,
        pointerDown,
        armed,
        reshowPending: true,
      })

    // The dangerous one: it would commit immediately without this window.
    expect(step(0, 26, false, false)).toBe("none")
    // And nothing else in the window is acted on either, in either direction.
    expect(step(0, 26, true, false)).toBe("none")
    expect(step(20, 0, true, true)).toBe("none")
    expect(step(26, undefined, false, false)).toBe("none")
  })

  it("changesPaneEffectivelyHidden: off by preference, or on but dragged to nothing", async () => {
    const mod = await loadStore()
    type S = ReturnType<typeof mod.getSnapshot>
    const hidden = (override: boolean | null, percent: number) =>
      mod.changesPaneEffectivelyHidden({
        changesPaneOverride: override,
        bootstrap: { show_changes_pane: true },
        changesPanePercent: percent,
      } as unknown as S)

    expect(hidden(true, 26)).toBe(false)
    // Hidden by preference. The percent is stale and irrelevant (the spacer
    // reads zero here), so it must not be what decides.
    expect(hidden(false, 26)).toBe(true)
    expect(hidden(false, 0)).toBe(true)
    // The state this whole fix exists for: the preference says visible and the
    // pane is nonetheless zero-width.
    expect(hidden(true, 0)).toBe(true)
    expect(hidden(true, 0.5)).toBe(true)
  })

  it("collapseChangesPaneFromDrag hides the pane through the same preference the menu writes", async () => {
    const mod = await loadStore()
    mod.collapseChangesPaneFromDrag()
    expect(mod.getSnapshot().changesPaneOverride).toBe(false)
    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/ui/changes-pane",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ visible: false }),
      }),
    )

    // Already hidden: no second write. The panel can report zero more than once
    // (an unmount measures too), and each write is a config PUT.
    const calls = fetchMock.mock.calls.length
    mod.collapseChangesPaneFromDrag()
    expect(fetchMock.mock.calls.length).toBe(calls)
  })

  it("showChangesPane restores the default width when the pane was dragged to nothing", async () => {
    const mod = await loadStore()
    mod.setChangesPanePercent(0)
    mod.showChangesPane()
    expect(mod.getSnapshot().changesPanePercent).toBe(
      mod.CHANGES_PANE_DEFAULT_PERCENT,
    )
    expect(mod.getSnapshot().changesPaneOverride).toBe(true)
  })

  it("showChangesPane keeps a width the user chose", async () => {
    const mod = await loadStore()
    mod.setChangesPanePercent(44)
    mod.showChangesPane()
    expect(mod.getSnapshot().changesPanePercent).toBe(44)
  })
})
