// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import {
  registerPaneMacroTarget,
  resetPaneMacroTargets,
  type PaneMacroTarget,
} from "@/lib/paneMacroTarget"
import type { SelectedTarget } from "@/lib/store"

// Where focus lands after PICKING a macro is a contract, not a nicety. On the
// direct-to-PTY path Base UI's default (or the caller's finalFocus) applies as
// it always has; but when the pick landed in the mobile compose DRAFT, focus
// must follow the macro into the compose textarea; Base UI owns focus during
// a popover close, so without the resolveFinalFocus routing it would hand
// focus back to the trigger and yank the keyboard away from the text the user
// is about to edit. runMacro's returned destination is what steers this, so
// the store is mocked to answer either way per test.

const runMacro = vi.fn<
  (name: string, target: SelectedTarget) => "compose" | "pty" | "none"
>()
const openMacrosDialog = vi.fn()
// The workspace the control reads its tab's state from; a case that needs a
// dormant tab seeds one.
let mockSpine: unknown = null
const mockState = () => ({
  bootstrap: {
    macros: [{ name: "Greet", text: "hello", surface: "both" }],
  },
  spine: mockSpine,
  startedDormantTabs: [],
  pendingSlotTab: {},
  endedInViewTab: null,
})
vi.mock("@/lib/store", () => ({
  useDux: () => mockState(),
  useDuxSelector: (select: (s: unknown) => unknown) => select(mockState()),
  runMacro: (name: string, target: SelectedTarget) => runMacro(name, target),
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
function ownerPane(over: Partial<PaneMacroTarget> = {}): PaneMacroTarget {
  return {
    takeoverCard: false,
    send: () => true,
    compose: null,
    typingSurface: () => null,
    ...over,
  }
}

const settle = () => new Promise((r) => setTimeout(r, 40))

function stubMatchMedia(coarse = false) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: coarse && query === "(pointer: coarse)",
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
  mockSpine = null
  runMacro.mockReset()
  openMacrosDialog.mockReset()
  resetPaneMacroTargets()
  // The pane under the control, published as its owner: an ordinary pane.
  registerPaneMacroTarget("s1", ownerPane())
  stubMatchMedia()
  // cmdk scrolls the selected item into view on mount; jsdom has no layout.
  Element.prototype.scrollIntoView = () => {}
})

afterEach(() => {
  cleanup()
  resetPaneMacroTargets()
  vi.unstubAllGlobals()
})

async function openAndPick() {
  fireEvent.click(screen.getByRole("button", { name: "Run a macro" }))
  await settle()
  fireEvent.click(await screen.findByText("Greet"))
  await settle()
}

// On a phone the picker presents as the SAME bottom sheet every dropdown
// menu uses (ui/popupSheet.ts): full width, backdrop, slide-from-bottom. The
// search field loses its autofocus there, because menus never pop the soft
// keyboard on open. The split is useIsMobile's innerWidth snapshot, the same
// idiom dropdown-menu.test.tsx flips.
describe("MacroPopover phone sheet presentation", () => {
  function setViewportWidth(width: number) {
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      writable: true,
      value: width,
    })
  }

  it("renders as the shared bottom sheet, search not autofocused", async () => {
    const originalWidth = window.innerWidth
    setViewportWidth(500)
    try {
      render(<MacroPopover target={target} />)
      fireEvent.click(screen.getByRole("button", { name: "Run a macro" }))
      await settle()
      const popup = document.querySelector('[data-slot="popover-content"]')
      expect(popup?.className).toContain("rounded-t-2xl")
      expect(popup?.className).toContain("slide-in-from-bottom")
      expect(popup?.className).not.toContain("w-72")
      expect(document.querySelector('[data-slot="popover-backdrop"]')).toBeTruthy()
      // The sheet's home-indicator clearance must survive the p-0 override
      // (tailwind-merge folds the whole padding group, so it is restated).
      expect(popup?.className).toContain(
        "pb-[max(env(safe-area-inset-bottom),0.25rem)]",
      )
      const input = document.querySelector('[data-slot="command-input"]')
      expect(input).toBeTruthy()
      expect(document.activeElement).not.toBe(input)
    } finally {
      setViewportWidth(originalWidth)
    }
  })

  it("keeps the anchored panel and the autofocused search on desktop", async () => {
    render(<MacroPopover target={target} />)
    fireEvent.click(screen.getByRole("button", { name: "Run a macro" }))
    await settle()
    const popup = document.querySelector('[data-slot="popover-content"]')
    expect(popup?.className).toContain("w-72")
    expect(popup?.className).not.toContain("rounded-t-2xl")
    expect(document.querySelector('[data-slot="popover-backdrop"]')).toBeNull()
    const input = document.querySelector('[data-slot="command-input"]')
    expect(document.activeElement).toBe(input)
  })
})

// The list grows like every other menu: the popup is capped at the
// positioner's --available-height (the dropdown menus' rule) and the command
// list drops the primitive's fixed 288px cap, so with a tall window the whole
// macro set is visible and only a genuinely over-tall list scrolls. jsdom has
// no layout, so the pin is the class contract on both halves.
describe("MacroPopover list height", () => {
  it("caps the popup at the available height and uncaps the list", async () => {
    render(<MacroPopover target={target} />)
    fireEvent.click(screen.getByRole("button", { name: "Run a macro" }))
    await settle()
    const popup = document.querySelector('[data-slot="popover-content"]')
    expect(popup?.className).toContain("max-h-(--available-height)")
    const list = document.querySelector('[data-slot="command-list"]')
    expect(list?.className).toContain("max-h-[none]")
    expect(list?.className).toContain("min-h-0")
    expect(list?.className).not.toContain("max-h-72")
  })
})

describe("MacroPopover pick focus routing", () => {
  it("a pick that landed in the compose draft focuses the compose textarea", async () => {
    // The pane-side sink is registered (the compose bar is the typing
    // surface); its target is the compose textarea the draft lives in.
    const compose = document.createElement("textarea")
    document.body.appendChild(compose)
    registerPaneMacroTarget(
      "s1",
      ownerPane({ compose: { insert: () => {}, target: () => compose } }),
    )
    runMacro.mockReturnValue("compose")

    render(<MacroPopover variant="icon" target={target} />)
    await openAndPick()

    expect(runMacro).toHaveBeenCalledWith("Greet", target)
    expect(document.activeElement).toBe(compose)
    compose.remove()
  })

  it("a direct-to-PTY pick keeps the caller's finalFocus target", async () => {
    // The desktop trigger points finalFocus at xterm's hidden textarea so the
    // user can review the pasted macro and press Enter, unchanged.
    const xtermTextarea = document.createElement("textarea")
    document.body.appendChild(xtermTextarea)
    runMacro.mockReturnValue("pty")

    render(
      <MacroPopover target={target} finalFocus={() => xtermTextarea} />,
    )
    await openAndPick()

    expect(runMacro).toHaveBeenCalledWith("Greet", target)
    expect(document.activeElement).toBe(xtermTextarea)
    xtermTextarea.remove()
  })

  it("a direct-to-PTY pick without finalFocus returns focus to the trigger", async () => {
    // The pane published no typing surface (its terminal is not mounted
    // yet): Base UI's default return-to-trigger behavior.
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

// With two panes mounted (an agent and its companion terminal), the desktop
// trigger in the header must hand focus back to the pane the macro went to,
// not to whichever pane mounted last, or the Enter meant to submit the macro
// types into the other terminal.
describe("MacroPopover returns focus to the pane the macro was sent to", () => {
  function textarea(): HTMLTextAreaElement {
    const el = document.createElement("textarea")
    document.body.appendChild(el)
    return el
  }

  it("a pick sent to the first pane's terminal focuses that terminal", async () => {
    const first = textarea()
    const second = textarea()
    registerPaneMacroTarget("s1", ownerPane({ typingSurface: () => first }))
    registerPaneMacroTarget("t9", ownerPane({ typingSurface: () => second }))
    runMacro.mockReturnValue("pty")

    render(<MacroPopover target={target} />)
    await openAndPick()

    expect(document.activeElement).toBe(first)
    first.remove()
    second.remove()
  })

  it("a pick sent to the first pane's compose box focuses that box", async () => {
    const first = textarea()
    const second = textarea()
    registerPaneMacroTarget(
      "s1",
      ownerPane({
        compose: { insert: () => {}, target: () => first },
        typingSurface: () => first,
      }),
    )
    registerPaneMacroTarget(
      "t9",
      ownerPane({
        compose: { insert: () => {}, target: () => second },
        typingSurface: () => second,
      }),
    )
    runMacro.mockReturnValue("compose")

    render(<MacroPopover target={target} />)
    await openAndPick()

    expect(document.activeElement).toBe(first)
    first.remove()
    second.remove()
  })
})

// The theater pill and the phone controls sit outside the pane just as the
// desktop header's trigger does, so a pick written straight to the pty returns
// focus to that pane's typing surface from every variant: the Enter that
// submits the macro must reach the terminal, not re-press the trigger. On a
// coarse pointer that would pop the soft keyboard the user never asked for, so
// there the trigger keeps focus, as the phone flap always has.
describe("MacroPopover returns focus to the pane from every variant", () => {
  function typingSurface(): HTMLTextAreaElement {
    const el = document.createElement("textarea")
    document.body.appendChild(el)
    registerPaneMacroTarget("s1", ownerPane({ typingSurface: () => el }))
    return el
  }

  it.each(["labeled", "icon", "pill"] as const)(
    "a %s pick sent to the pty focuses the pane's typing surface",
    async (variant) => {
      const surface = typingSurface()
      runMacro.mockReturnValue("pty")

      render(<MacroPopover variant={variant} target={target} />)
      await openAndPick()

      expect(document.activeElement).toBe(surface)
      surface.remove()
    },
  )

  it.each(["icon", "pill"] as const)(
    "a %s pick on a coarse pointer leaves focus on the trigger",
    async (variant) => {
      stubMatchMedia(true)
      const surface = typingSurface()
      runMacro.mockReturnValue("pty")

      render(<MacroPopover variant={variant} target={target} />)
      await openAndPick()

      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Run a macro" }),
      )
      surface.remove()
    },
  )

  it("a pill pick that sent nothing leaves focus on the trigger", async () => {
    const surface = typingSurface()
    runMacro.mockReturnValue("none")

    render(<MacroPopover variant="pill" target={target} />)
    await openAndPick()

    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Run a macro" }),
    )
    surface.remove()
  })
})

// While the take-over card covers the pane a Macros control belongs to, this tab
// is not driving the pty: the control stays on screen at its full size, cannot
// be pressed, and says why through the shared tooltip. Both card states, another
// device driving and nobody driving, publish the same `takeoverCard` verdict.
describe("MacroPopover under the take-over card", () => {
  const isDisabled = (el: HTMLElement) => (el as HTMLButtonElement).disabled
  const covered = () =>
    registerPaneMacroTarget("s1", { ...ownerPane(), takeoverCard: true, send: null })

  it.each(["labeled", "icon", "pill"] as const)(
    "the %s trigger is disabled and opens nothing",
    async (variant) => {
      covered()
      render(<MacroPopover variant={variant} target={target} />)
      const trigger = screen.getByRole("button", { name: "Run a macro" })
      expect(isDisabled(trigger)).toBe(true)
      fireEvent.click(trigger)
      await settle()
      expect(document.querySelector('[data-slot="popover-content"]')).toBeNull()
      expect(runMacro).not.toHaveBeenCalled()
    },
  )

  it("keeps the trigger's own size classes while disabled", () => {
    covered()
    render(<MacroPopover variant="pill" target={target} />)
    expect(screen.getByRole("button", { name: "Run a macro" }).className).toContain(
      "size-10",
    )
  })

  it("explains itself through the shared tooltip, never a title", async () => {
    covered()
    render(<MacroPopover target={target} />)
    const wrapper = screen.getByTestId("macros-disabled")
    expect(wrapper.getAttribute("title")).toBeNull()
    expect(
      screen.getByRole("button", { name: "Run a macro" }).getAttribute("title"),
    ).toBeNull()
    act(() => wrapper.focus())
    expect(await screen.findByText("Take over to send macros")).toBeTruthy()
  })

  it("re-enables the moment this tab becomes the owner", async () => {
    const retire = covered()
    render(<MacroPopover target={target} />)
    expect(isDisabled(screen.getByRole("button", { name: "Run a macro" }))).toBe(true)
    act(() => {
      retire()
      registerPaneMacroTarget("s1", ownerPane())
    })
    const trigger = screen.getByRole("button", { name: "Run a macro" })
    expect(isDisabled(trigger)).toBe(false)
    expect(screen.queryByTestId("macros-disabled")).toBeNull()
    await openAndPick()
    expect(runMacro).toHaveBeenCalledWith("Greet", target)
  })

  it("reads the card of its own pane, not another mounted one", () => {
    registerPaneMacroTarget("t9", { ...ownerPane(), takeoverCard: true, send: null })
    render(<MacroPopover target={target} />)
    expect(isDisabled(screen.getByRole("button", { name: "Run a macro" }))).toBe(false)
  })
})

// A tab that is not running has no pane, so nothing can carry a macro to it: the
// control is disabled and says how to get one.
describe("MacroPopover over a dormant tab", () => {
  const dormantTarget: SelectedTarget = {
    kind: "agent",
    sessionId: "s1",
    tabId: "t2",
  } as SelectedTarget

  beforeEach(() => {
    mockSpine = {
      projects: [],
      sessions: [
        {
          id: "s1",
          slot_tab_id: "s1",
          tabs: [
            { id: "s1", has_live_process: true },
            { id: "t2", has_live_process: false },
          ],
        },
      ],
    }
  })

  it.each(["labeled", "icon", "pill"] as const)(
    "the %s trigger is disabled and opens nothing",
    async (variant) => {
      render(<MacroPopover variant={variant} target={dormantTarget} />)
      const trigger = screen.getByRole("button", { name: "Run a macro" })
      expect((trigger as HTMLButtonElement).disabled).toBe(true)
      fireEvent.click(trigger)
      await settle()
      expect(document.querySelector('[data-slot="popover-content"]')).toBeNull()
      expect(runMacro).not.toHaveBeenCalled()
    },
  )

  it("says to start the tab, through the shared tooltip", async () => {
    render(<MacroPopover target={dormantTarget} />)
    const wrapper = screen.getByTestId("macros-disabled")
    expect(wrapper.getAttribute("title")).toBeNull()
    act(() => wrapper.focus())
    expect(await screen.findByText("Start the tab to send macros")).toBeTruthy()
  })

  it("stays live for the running tab beside it", () => {
    registerPaneMacroTarget("s1", ownerPane())
    render(<MacroPopover target={target} />)
    const trigger = screen.getByRole("button", { name: "Run a macro" })
    expect((trigger as HTMLButtonElement).disabled).toBe(false)
  })
})
