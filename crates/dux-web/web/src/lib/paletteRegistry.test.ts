import { beforeAll, describe, expect, it, vi } from "vitest"

// `paletteRegistry` imports `store`, which at module load reads `location` and
// `localStorage` and registers a `popstate` listener (the test env is node, not
// a DOM). Stub the minimum surface so the import succeeds; the pin test only
// inspects the handler map's keys, it never opens a socket or runs a handler.
beforeAll(() => {
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
})

// TWO-SIDED PIN (TS half): the exact set of web-surfaced palette command ids.
// The Rust counterpart pins the `Web`/`Both` core registry to this same list —
// see `web_surface_ids_match_expected_pin` in
// `crates/dux-core/src/palette.rs`. Changing one surface without the other
// fails a gate. Keep this list alphabetized.
const EXPECTED_WEB_COMMANDS = [
  "add-project",
  "configure-global-env",
  "reload-config",
  "sort-agents-by-created",
  "sort-agents-by-name",
  "sort-agents-by-updated",
  "toggle-diff-line-numbers",
]

describe("paletteRegistry", () => {
  it("handler map keys match the pinned web-command set", async () => {
    const { PALETTE_HANDLERS } = await import("./paletteRegistry")
    const keys = Object.keys(PALETTE_HANDLERS).sort()
    expect(keys).toEqual(EXPECTED_WEB_COMMANDS)
  })

  it("every handler is a function", async () => {
    const { PALETTE_HANDLERS } = await import("./paletteRegistry")
    for (const [id, handler] of Object.entries(PALETTE_HANDLERS)) {
      expect(typeof handler, `handler for ${id}`).toBe("function")
    }
  })
})
