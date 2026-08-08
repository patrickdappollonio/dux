// @vitest-environment jsdom
// The web UI deliberately binds NO keyboard shortcut to open its app menu, and
// the Ctrl+K / Cmd+K command palette is gone. This pins both facts: the palette's
// window-level handler called preventDefault on the chord, so a stray
// reintroduction would silently steal Ctrl+K from the browser (and from a
// terminal-focused agent) again.
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render } from "@testing-library/react"

vi.mock("@/lib/store", () => ({
  openCustomizeWebapp: vi.fn(),
  openConfigEditor: vi.fn(),
  openMacrosDialog: vi.fn(),
  openGlobalEnv: vi.fn(),
  openTaskManager: vi.fn(),
  sortAgents: vi.fn(),
  openAddProject: vi.fn(),
  openAddProjectForInit: vi.fn(),
  openCreateAgentFromPr: vi.fn(),
  openNewAgentPicker: vi.fn(),
  useDux: () => ({ bootstrap: { gh_available: true } }),
}))
vi.mock("@/lib/configApi", () => ({
  configApi: { reload: () => Promise.resolve() },
}))

import { AppMenu } from "@/components/AppMenu"

afterEach(() => cleanup())

describe("app menu keyboard bindings", () => {
  it("binds no Ctrl+K or Cmd+K handler on window", () => {
    render(<AppMenu />)

    for (const init of [
      { key: "k", ctrlKey: true },
      { key: "k", metaKey: true },
      { key: "K", ctrlKey: true },
    ]) {
      const event = new KeyboardEvent("keydown", {
        ...init,
        bubbles: true,
        cancelable: true,
      })
      window.dispatchEvent(event)
      // Nothing consumed the chord: no menu opened, and no handler called
      // preventDefault, so the browser's own Ctrl+K still works.
      expect(document.querySelector('[role="menu"]')).toBeNull()
      expect(event.defaultPrevented).toBe(false)
    }
  })

  it("leaves the store with no palette state", async () => {
    // The store boots on import and reaches for browser globals; stub them so
    // this stays hermetic and off the network (mirrors InsetHeader.test.tsx).
    const mem = new Map<string, string>()
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: (k: string, v: string) => void mem.set(k, String(v)),
      removeItem: (k: string) => void mem.delete(k),
      clear: () => mem.clear(),
    })
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline test"))),
    )
    try {
      const store = await vi.importActual<Record<string, unknown>>("@/lib/store")
      expect("setPaletteOpen" in store).toBe(false)
      expect("toggleCopyOnSelect" in store).toBe(false)
      expect("toggleAlwaysShowTabs" in store).toBe(false)
      expect("togglePrBannerPosition" in store).toBe(false)
      expect("toggleRandomizedPetNameDefault" in store).toBe(false)
      // Still live: the Changes actions menu calls it.
      expect("toggleChangesPane" in store).toBe(true)
    } finally {
      vi.unstubAllGlobals()
    }
  })

  it("opens on ArrowDown but not on a bare letter key", async () => {
    const { getByRole, queryByRole } = render(<AppMenu />)
    const trigger = getByRole("button", { name: /^menu$/i })
    trigger.focus()

    fireEvent.keyDown(trigger, { key: "k" })
    await new Promise((r) => setTimeout(r, 30))
    expect(queryByRole("menu")).toBeNull()

    fireEvent.keyDown(trigger, { key: "ArrowDown" })
    await new Promise((r) => setTimeout(r, 30))
    expect(queryByRole("menu")).toBeTruthy()
  })
})
