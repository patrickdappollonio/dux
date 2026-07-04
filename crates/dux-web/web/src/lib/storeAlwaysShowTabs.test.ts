import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Mirrors `storeChangesPane.test.ts`'s harness: the store module reads
// location/localStorage, registers listeners, and fires a bootstrap fetch on
// import. `toggleAlwaysShowTabs` is a parameterless server-side flip (like
// `toggleGithubIntegration`/`togglePrBannerPosition`), so there is no
// optimistic local state to assert on — only that it POSTs the right path.

function makeBootstrap(): Bootstrap {
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
    show_changes_pane: true,
    always_show_tab_strip: false,
    global_env: {},
    status_clear_seconds: 6,
  }
}

let bootstrapBody: Bootstrap = makeBootstrap()

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
  if (u.includes("/api/v1/ui/toggle-always-show-tab-strip")) {
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
  bootstrapBody = makeBootstrap()
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

describe("toggleAlwaysShowTabs", () => {
  it("POSTs the toggle-always-show-tab-strip endpoint with no body", async () => {
    const mod = await loadStore()
    mod.toggleAlwaysShowTabs()
    await vi.waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/ui/toggle-always-show-tab-strip",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({}),
        }),
      )
    })
  })

  it("the toggle-always-show-tabs palette command runs the toggle", async () => {
    await loadStore() // boot the store so the palette handler module resolves
    const { PALETTE_HANDLERS } = await import("./paletteRegistry")
    PALETTE_HANDLERS["toggle-always-show-tabs"]()
    await vi.waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/ui/toggle-always-show-tab-strip",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({}),
        }),
      )
    })
  })
})
