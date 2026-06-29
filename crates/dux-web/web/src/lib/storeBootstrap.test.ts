import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Exercises the bootstrap slice end to end: the boot fetch populating the slice,
// a `config.changed` event triggering a refetch, and a representative consumer
// (changesPaneVisible) reading from the slice. The `bootstrapApi` wire behaviour
// (GET shape, error mapping) lives in `bootstrapApi.test.ts`; here we drive the
// store's integration via a controllable fetch double.
//
// The store fires a `GET /api/v1/bootstrap` at import. We serve the bootstrap
// body from `bootstrapBody`, which the fetch double reads at call time so a test
// can mutate it before a refetch.

function makeBootstrap(overrides: Partial<Bootstrap> = {}): Bootstrap {
  return {
    available_providers: ["claude", "codex"],
    macros: [],
    palette_commands: [],
    welcome_tips: ["tip one"],
    dux_version: "v1.2.3",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    global_env: {},
    status_clear_seconds: 6,
    ...overrides,
  }
}

let bootstrapBody: Bootstrap = makeBootstrap()
let bootstrapFetches = 0
// When true, the bootstrap GET rejects (simulated network failure) — used to
// exercise the failed-first-load → reconnect-retry recovery path.
let bootstrapShouldFail = false

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    bootstrapFetches++
    if (bootstrapShouldFail) throw new Error("network down")
    return {
      ok: true,
      status: 200,
      json: async () => bootstrapBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // Anything else: return empty 200.
  return {
    status: 200,
    json: async () => ({}),
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
  bootstrapBody = makeBootstrap()
  bootstrapFetches = 0
  bootstrapShouldFail = false
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

describe("bootstrap slice", () => {
  it("the boot fetch populates the slice", async () => {
    bootstrapBody = makeBootstrap({
      dux_version: "v9.9.9",
      available_providers: ["claude", "opencode", "copilot"],
      welcome_tips: ["surprise!"],
    })
    const mod = await loadStore()
    const b = mod.getSnapshot().bootstrap
    expect(b?.dux_version).toBe("v9.9.9")
    expect(b?.available_providers).toEqual(["claude", "opencode", "copilot"])
    expect(b?.welcome_tips).toEqual(["surprise!"])
    // Exactly one bootstrap GET on boot.
    expect(bootstrapFetches).toBe(1)
  })

  it("a config.changed event triggers a refetch that replaces the slice", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().bootstrap?.welcome_tips).toEqual(["tip one"])
    const before = bootstrapFetches

    // The server's config was edited/reloaded: the new body has fresh values.
    bootstrapBody = makeBootstrap({
      welcome_tips: ["edited tip"],
      gh_available: true,
    })
    mod.eventsSocket.onEvent({ event: "config.changed" })

    await vi.waitFor(() => {
      expect(mod.getSnapshot().bootstrap?.welcome_tips).toEqual(["edited tip"])
    })
    expect(mod.getSnapshot().bootstrap?.gh_available).toBe(true)
    // The event drove exactly one additional GET.
    expect(bootstrapFetches).toBe(before + 1)
  })

  it("an unrelated event does not refetch bootstrap", async () => {
    const mod = await loadStore()
    const before = bootstrapFetches
    // A session.changes event for an unselected session must not touch bootstrap.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s-unknown", rev: 1 })
    expect(bootstrapFetches).toBe(before)
  })

  it("retries a failed first bootstrap load on a reconnect onOpen", async () => {
    // The very first load (driven by boot()) fails, so the slice stays null.
    bootstrapShouldFail = true
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().booted).toBe(true)
    })
    await vi.waitFor(() => {
      expect(bootstrapFetches).toBeGreaterThanOrEqual(1)
    })
    expect(mod.getSnapshot().bootstrap).toBeNull()

    // The initial connect's open consumes the skip flag and does NOT refetch
    // (boot() already drove the first load -- even though it failed).
    const afterInitialOpen = bootstrapFetches
    mod.eventsSocket.onOpen()
    expect(bootstrapFetches).toBe(afterInitialOpen)
    expect(mod.getSnapshot().bootstrap).toBeNull()

    // A later RE-connect retries even though the slice is still null (the old
    // `bootstrap !== null` guard would have skipped this forever). It now succeeds.
    bootstrapShouldFail = false
    mod.eventsSocket.onOpen()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().bootstrap).not.toBeNull()
    })
  })

  it("a representative consumer (changesPaneVisible) reads from the slice", async () => {
    bootstrapBody = makeBootstrap({ show_changes_pane: false })
    const mod = await loadStore()
    // No optimistic override → the value comes straight from the bootstrap slice.
    expect(mod.getSnapshot().changesPaneOverride).toBe(null)
    expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(false)

    // A config.changed flip to visible is reflected through the same consumer.
    bootstrapBody = makeBootstrap({ show_changes_pane: true })
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(mod.changesPaneVisible(mod.getSnapshot())).toBe(true)
    })
  })

  // Scoped `document` stub: the file-wide beforeEach stays document-free so the
  // `typeof document` guard in applyBootstrap is still exercised as "absent" for
  // every other test. The sentinel title (not "dux") proves applyBootstrap wrote
  // the value rather than coincidentally matching the stub's initial state.
  describe("instance title → document.title", () => {
    beforeEach(() => {
      vi.stubGlobal("document", { title: "pending" })
    })

    it("sets document.title from the configured instance title", async () => {
      bootstrapBody = makeBootstrap({ title: "dux #1" })
      await loadStore()
      expect(document.title).toBe("dux #1")
    })

    it("resolves a blank instance title to the product name", async () => {
      bootstrapBody = makeBootstrap({ title: "   " })
      await loadStore()
      expect(document.title).toBe("dux")
    })

    it("updates the tab title live on a config.changed rename", async () => {
      bootstrapBody = makeBootstrap({ title: "first" })
      const mod = await loadStore()
      expect(document.title).toBe("first")

      // The server's config was edited and reloaded with a new instance name.
      bootstrapBody = makeBootstrap({ title: "renamed" })
      mod.eventsSocket.onEvent({ event: "config.changed" })
      await vi.waitFor(() => {
        expect(document.title).toBe("renamed")
      })
    })
  })
})
