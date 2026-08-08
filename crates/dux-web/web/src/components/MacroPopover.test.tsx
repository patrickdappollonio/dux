// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { setComposeInsertSink } from "@/lib/composeInsert"
import type { SelectedTarget } from "@/lib/store"

// Where focus lands after PICKING a macro is a contract, not a nicety. On the
// direct-to-PTY path Base UI's default (or the caller's finalFocus) applies as
// it always has; but when the pick landed in the mobile compose DRAFT, focus
// must follow the macro into the compose textarea — Base UI owns focus during
// a popover close, so without the resolveFinalFocus routing it would hand
// focus back to the trigger and yank the keyboard away from the text the user
// is about to edit. runMacro's returned destination is what steers this, so
// the store is mocked to answer either way per test.

const runMacro = vi.fn<(name: string) => "compose" | "pty" | "none">()
const openMacrosDialog = vi.fn()
vi.mock("@/lib/store", () => ({
  useDux: () => ({
    bootstrap: {
      macros: [{ name: "Greet", text: "hello", surface: "both" }],
    },
  }),
  runMacro: (name: string) => runMacro(name),
  openMacrosDialog: () => openMacrosDialog(),
}))

const { MacroPopover } = await import("./MacroPopover")

const target: SelectedTarget = {
  kind: "agent",
  sessionId: "s1",
  tabId: "s1",
} as SelectedTarget

// Base UI runs its open/close transitions and focus moves on timers; a short
// settle lets them land (the idiom the dropdown-menu tests use).
const settle = () => new Promise((r) => setTimeout(r, 40))

function stubMatchMedia() {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
  // Base UI's positioner measures the popup with a ResizeObserver jsdom lacks.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
}

beforeEach(() => {
  runMacro.mockReset()
  openMacrosDialog.mockReset()
  setComposeInsertSink(null)
  stubMatchMedia()
  // cmdk scrolls the selected item into view on mount; jsdom has no layout.
  Element.prototype.scrollIntoView = () => {}
})

afterEach(() => {
  cleanup()
  setComposeInsertSink(null)
  vi.unstubAllGlobals()
})

async function openAndPick() {
  fireEvent.click(screen.getByRole("button", { name: "Run a macro" }))
  await settle()
  fireEvent.click(await screen.findByText("Greet"))
  await settle()
}

describe("MacroPopover pick focus routing", () => {
  it("a pick that landed in the compose draft focuses the compose textarea", async () => {
    // The pane-side sink is registered (the compose bar is the typing
    // surface); its target is the compose textarea the draft lives in.
    const compose = document.createElement("textarea")
    document.body.appendChild(compose)
    setComposeInsertSink({ insert: () => {}, target: () => compose })
    runMacro.mockReturnValue("compose")

    render(<MacroPopover variant="icon" target={target} />)
    await openAndPick()

    expect(runMacro).toHaveBeenCalledWith("Greet")
    expect(document.activeElement).toBe(compose)
    compose.remove()
  })

  it("a direct-to-PTY pick keeps the caller's finalFocus target", async () => {
    // The desktop trigger points finalFocus at xterm's hidden textarea so the
    // user can review the pasted macro and press Enter — unchanged.
    const xtermTextarea = document.createElement("textarea")
    document.body.appendChild(xtermTextarea)
    runMacro.mockReturnValue("pty")

    render(
      <MacroPopover target={target} finalFocus={() => xtermTextarea} />,
    )
    await openAndPick()

    expect(runMacro).toHaveBeenCalledWith("Greet")
    expect(document.activeElement).toBe(xtermTextarea)
    xtermTextarea.remove()
  })

  it("a direct-to-PTY pick without finalFocus returns focus to the trigger", async () => {
    // The mobile icon variant with the compose bar OFF: Base UI's default
    // return-to-trigger behavior, exactly as before this routing existed.
    runMacro.mockReturnValue("pty")

    render(<MacroPopover variant="icon" target={target} />)
    await openAndPick()

    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Run a macro" }),
    )
  })

  it("a compose-destination pick with a vanished sink falls back to the default", async () => {
    // The sink can retire between the pick and the close-focus pass (an
    // ownership handover racing the tap); a null target must fall back rather
    // than focus nothing.
    runMacro.mockReturnValue("compose")

    render(<MacroPopover variant="icon" target={target} />)
    await openAndPick()

    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Run a macro" }),
    )
  })
})
