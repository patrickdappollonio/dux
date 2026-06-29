// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Drives the real store's boot/config.changed path under jsdom (so a real
// document exists) and asserts applyBootstrap swaps the <link rel="icon">. The
// pure resolution + DOM mechanics live in favicon.test.ts / favicon.dom.test.tsx;
// this proves the store actually wires config.server.favicon → applyFavicon.

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
  // jsdom provides window/document/location; it lacks localStorage and a usable
  // WebSocket, and we don't want real network, so stub those.
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  document.head.innerHTML = ""
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
  document.head.innerHTML = ""
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

function iconHref(): string | null {
  return document.querySelector("link[rel='icon']")?.getAttribute("href") ?? null
}

describe("store applies the configured favicon", () => {
  it("renders a recoloured dux-logo outline for a colour", async () => {
    bootstrapBody = makeBootstrap({ favicon: "violet" })
    await loadStore()
    const href = iconHref() ?? ""
    expect(href.startsWith("data:image/svg+xml,")).toBe(true)
    // Verify the colour survives the whole store→applyFavicon path, not just the
    // data-URI format.
    const decoded = decodeURIComponent(href.replace("data:image/svg+xml,", ""))
    expect(decoded).toContain('stroke="#863bff"')
  })

  it("keeps the bundled favicon when none is configured", async () => {
    bootstrapBody = makeBootstrap({ favicon: "" })
    await loadStore()
    expect(iconHref()).toBe("/favicon.svg")
  })

  it("reads the favicon field, not the title", async () => {
    // favicon empty keeps the bundled logo even though the title on its own would
    // resolve to a colour. Guards against applyFavicon being wired to b.title.
    bootstrapBody = makeBootstrap({ favicon: "", title: "violet" })
    await loadStore()
    expect(iconHref()).toBe("/favicon.svg")
  })

  it("updates the favicon live on a config.changed", async () => {
    bootstrapBody = makeBootstrap({ favicon: "" })
    const mod = await loadStore()
    expect(iconHref()).toBe("/favicon.svg")

    bootstrapBody = makeBootstrap({ favicon: "https://x.test/a.png" })
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => {
      expect(iconHref()).toBe("https://x.test/a.png")
    })
  })
})
