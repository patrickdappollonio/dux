// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { HOVER_REVEAL, REVEAL_ROW, REVEALED_ON_ANY_FOCUS } from "@/lib/hoverReveal"
import type { SessionView } from "@/lib/types"
import { ONLY_TAB_CLOSE_REFUSAL } from "@/lib/agentTabs"
import {
  fireAnimationIteration,
  fireAnimationStart,
  stubGlyphEase,
} from "@/test/animationEvents"

let mockState: DuxState
const addTabMock = vi.fn()
const openCloseTabMock = vi.fn()
const openStopTabMock = vi.fn()
const clickTabMock = vi.fn()
const reorderTabsMock = vi.fn()
// The real strip, with its props captured. A drag runs under jsdom, but every
// rect is empty, so where it would drop cannot be computed (the pure part is in
// SortableStrip.test.tsx); the tests hand the strip the drop it would produce.
let sortableProps: {
  ids: string[]
  onReorder: (next: string[]) => void
  disabled?: boolean
} | null = null
vi.mock("@/components/SortableStrip", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/SortableStrip")>()
  return {
    ...actual,
    SortableStrip: (props: Parameters<typeof actual.SortableStrip>[0]) => {
      sortableProps = props
      return actual.SortableStrip(props)
    },
  }
})
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: (...args: unknown[]) => addTabMock(...args),
    openCloseTab: (...args: unknown[]) => openCloseTabMock(...args),
    openStopTab: (...args: unknown[]) => openStopTabMock(...args),
    clickTab: (...args: unknown[]) => clickTabMock(...args),
    reorderTabs: (...args: unknown[]) => reorderTabsMock(...args),
  }
})

function installBootStubs() {
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
}
installBootStubs()
const { AgentTabsStrip } = await import("./AgentTabsStrip")
const { DockStrip } = await import("./DockStrip")
const { agentRowKey, registerPaneCover, resetPaneCovers } = await import("@/lib/paneCover")

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "",
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: "",
    },
    provider: "claude",
    tabs: [
      { id: "s1", provider: "claude", order: 0, working: false, has_output: false, has_live_process: true },
      { id: "b2", provider: "codex", order: 1, working: false, has_output: false, has_live_process: true },
    ],
  } as unknown as SessionView
}

beforeEach(() => {
  installBootStubs()
  addTabMock.mockClear()
  openCloseTabMock.mockClear()
  openStopTabMock.mockClear()
  clickTabMock.mockClear()
  reorderTabsMock.mockClear()
  mockState = {
    bootstrap: { available_providers: ["claude", "codex", "opencode"] },
    spine: {
      projects: [{ id: "p1", default_provider: "codex" }],
    },
    createTabInFlight: [],
    pendingTabOrder: {},
    pendingSlotTab: {},
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  resetPaneCovers()
  vi.unstubAllGlobals()
})

describe("AgentTabsStrip", () => {
  it("exposes exactly one close affordance per pill (the ⋯ menu, no standalone ✕)", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    // There is no standalone ✕ button (aria-label "Close tab"); closing lives
    // only inside the ⋯ menu's "Close tab…" item (not rendered until open).
    expect(screen.queryByLabelText("Close tab")).toBeNull()
    expect(screen.getAllByLabelText("Tab actions")).toHaveLength(2)
  })

  it("disables the New tab button at the per-agent cap", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={2} />)
    expect(screen.getByLabelText("New agent tab")).toHaveProperty("disabled", true)
  })

  it("also disables the provider-picker caret at the per-agent cap", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={2} />)
    expect(screen.getByLabelText("Choose provider for the new agent tab")).toHaveProperty(
      "disabled",
      true,
    )
  })

  it("clicking the main + quick-adds the project default provider (no provider arg)", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("New agent tab"))
    expect(addTabMock).toHaveBeenCalledWith("s1")
  })

  it("the caret opens a menu listing configured providers with the default marked", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("Choose provider for the new agent tab"))
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("claude")).toBeTruthy()
    expect(menu.getByText("codex")).toBeTruthy()
    expect(menu.getByText("opencode")).toBeTruthy()
    // The project default (codex, per the mocked spine) is tagged "default".
    expect(menu.getByText("default")).toBeTruthy()
  })

  it("picking a provider from the caret menu adds a tab targeting it", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("Choose provider for the new agent tab"))
    const menu = within(await screen.findByRole("menu"))
    fireEvent.click(menu.getByText("opencode"))
    expect(addTabMock).toHaveBeenCalledWith("s1", "opencode")
  })

  // The first tab is closable: closing it hands the slot to the next tab, so its
  // menu item is an ordinary enabled one, exactly like an extra tab's.
  // An agent's ONLY tab has no successor to hand the slot to, so the server
  // refuses that close. The menu refuses it too, in the same words and before
  // any dialog: the alternative is a confirmation promising a detach followed
  // by a 400.
  it("disables the Close item on an agent's only tab and says why", async () => {
    const sole = session()
    sole.tabs = [sole.tabs[0]]
    render(<AgentTabsStrip session={sole} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[0])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item?.getAttribute("aria-disabled")).toBe("true")
    expect(menu.getByText(ONLY_TAB_CLOSE_REFUSAL)).toBeTruthy()
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).not.toHaveBeenCalled()
    // Stopping needs no successor: the tab stays, so the only tab stops too.
    fireEvent.click(menu.getByText("Stop tab…"))
    expect(openStopTabMock).toHaveBeenCalledWith("s1", "s1")
  })

  it("keeps the first tab's Close item enabled and acting", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[0])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item).toBeTruthy()
    expect(item?.getAttribute("aria-disabled")).not.toBe("true")
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).toHaveBeenCalledWith("s1", "s1")
  })

  it("keeps an extra tab's Close and Stop items enabled and acting", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item?.getAttribute("aria-disabled")).not.toBe("true")
    fireEvent.click(menu.getByText("Stop tab…"))
    expect(openStopTabMock).toHaveBeenCalledWith("s1", "b2")
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    fireEvent.click(within(await screen.findByRole("menu")).getByText("Close tab…"))
    expect(openCloseTabMock).toHaveBeenCalledWith("s1", "b2")
  })

  // A tab that is not running has no process to stop, so the item is absent,
  // as the agent menu's Detach is with nothing running.
  it("offers no Stop item on a tab that is not running", async () => {
    const stopped = session()
    stopped.tabs[1] = { ...stopped.tabs[1], has_live_process: false }
    render(<AgentTabsStrip session={stopped} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("Close tab…")).toBeTruthy()
    expect(menu.queryByText("Stop tab…")).toBeNull()
  })

  it("selects a tab on Enter on its pill, never on Enter inside its ⋯", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const trigger = screen.getAllByLabelText("Tab actions")[0]
    fireEvent.keyDown(trigger, { key: "Enter" })
    fireEvent.click(trigger)
    const item = within(await screen.findByRole("menu")).getByText("Close tab…")
    fireEvent.keyDown(item, { key: "Enter" })
    expect(clickTabMock).not.toHaveBeenCalled()

    fireEvent.keyDown(screen.getAllByRole("tab")[0], { key: "Enter" })
    expect(clickTabMock).toHaveBeenCalledWith("s1", "s1")
  })

  it("marks the flagged tab's pill with an attention dot", () => {
    const s = session()
    s.tabs[1].needs_attention = true
    render(<AgentTabsStrip session={s} activeTabId="s1" maxTabs={20} />)
    expect(screen.getAllByLabelText("Needs attention")).toHaveLength(1)
  })

  it("renders no attention dot when no tab needs attention", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(screen.queryByLabelText("Needs attention")).toBeNull()
  })
})

// The strip stays above the take-over card, so it gates what changes the row
// the way the agent's menu does, whoever the card says drives it.
describe("AgentTabsStrip on a row another device drives", () => {
  const watch = (driver: "other" | "nobody") =>
    registerPaneCover("s1", "card", { row: agentRowKey("s1"), driver })

  it("disables adding, and every change in a pill's ⋯, and still selects a tab", async () => {
    watch("other")
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(screen.getByLabelText("New agent tab")).toHaveProperty("disabled", true)
    expect(screen.getByLabelText("Choose provider for the new agent tab")).toHaveProperty("disabled", true)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText(/active on another device/i)).toBeTruthy()
    for (const label of ["Change provider…", "Stop tab…", "Close tab…"]) {
      const item = menu.getByText(label).closest("[role='menuitem']")
      expect(item?.getAttribute("aria-disabled"), label).toBe("true")
    }
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).not.toHaveBeenCalled()
    fireEvent.keyDown(screen.getAllByRole("tab")[1], { key: "Enter" })
    expect(clickTabMock).toHaveBeenCalledWith("s1", "b2")
  })

  it("gates all of it while nobody drives the row, and says so truthfully", async () => {
    watch("nobody")
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(screen.getByLabelText("New agent tab")).toHaveProperty("disabled", true)
    expect(sortableProps?.disabled).toBe(true)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText(/Nobody is driving this agent/)).toBeTruthy()
    expect(menu.queryByText(/active on another device/i)).toBeNull()
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item?.getAttribute("aria-disabled")).toBe("true")
  })

  // The order is the agent's own, kept on the server, so a drag is a change.
  it("does not reorder while another device drives the row", () => {
    watch("other")
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(sortableProps?.disabled).toBe(true)
  })

  // A disabled button takes no pointer events and no focus, so the reason has
  // to be readable from what holds it.
  it("says why the + is disabled while the page is reconnecting, on focus", async () => {
    registerPaneCover("s1", "spinner", { row: agentRowKey("s1"), driver: "reconnecting" })
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const holder = screen.getByLabelText("New agent tab").parentElement as HTMLElement
    act(() => holder.focus())
    expect((await screen.findAllByText("Reconnecting to this agent")).length).toBeGreaterThan(0)
  })

  // Pressing + puts the add in flight, which disables it until the tab lands.
  // The control the user pressed is the one that keeps the focus throughout.
  // A browser takes focus off a button the moment it is disabled, which jsdom
  // does not; the + here does it the browser's way, so the wrapper must take
  // the focus itself rather than leave it on the page body.
  it("keeps the same + and chooser, and the focus on the control, while an add is in flight", () => {
    const { rerender } = render(
      <AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />,
    )
    const plus = screen.getByLabelText("New agent tab")
    const chooser = screen.getByLabelText("Choose provider for the new agent tab")
    blurWhenDisabled(plus)
    act(() => plus.focus())
    for (const [inFlight, focused] of [
      [["s1"], plus.parentElement],
      [[], plus],
    ] as const) {
      mockState = { ...mockState, createTabInFlight: [...inFlight] }
      rerender(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
      expect(screen.getByLabelText("New agent tab")).toBe(plus)
      expect(screen.getByLabelText("Choose provider for the new agent tab")).toBe(chooser)
      expect(document.activeElement).toBe(focused)
    }
  })

  // A tap opens the reason; once the control changes state nobody asked again,
  // so neither the old reason nor the new label is left on screen.
  it("closes a tapped reason when the + changes state, and never reopens it", async () => {
    mockState = { ...mockState, createTabInFlight: ["s1"] }
    const { rerender } = render(
      <AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />,
    )
    fireEvent.click(screen.getByLabelText("New agent tab").parentElement!)
    expect((await screen.findAllByText("Adding…")).length).toBeGreaterThan(0)
    for (const [inFlight, text] of [[[], "New codex agent tab"], [["s1"], "Adding…"]] as const) {
      mockState = { ...mockState, createTabInFlight: [...inFlight] }
      rerender(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
      await act(async () => {
        await new Promise((r) => setTimeout(r, 400))
      })
      expect(screen.queryByText(text)).toBeNull()
    }
  })

  // A watcher's drag is a plain select: the pill must not move while it is held.
  it("does not move a pill that is dragged while another device drives the row", () => {
    watch("other")
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const pill = screen.getAllByRole("tab")[1]
    fireEvent.mouseDown(pill, { button: 0, clientX: 10, clientY: 10 })
    fireEvent.mouseMove(document, { clientX: 30, clientY: 10 })
    fireEvent.mouseMove(document, { clientX: 31, clientY: 10 })
    expect(pill.style.transform).toBe("")
  })

  it("reorders while this device drives the row", () => {
    registerPaneCover("s1", "none", { row: agentRowKey("s1"), driver: "this" })
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(sortableProps?.disabled).toBe(false)
  })
})

describe("AgentTabsStrip order and the first-tab pin", () => {
  const PIN = "The agent's first tab: the one that starts when you open a stopped agent."

  function threeTabs(slot: string): SessionView {
    const base = session()
    return {
      ...base,
      slot_tab_id: slot,
      tabs: [
        { ...base.tabs[0], id: "t1" },
        { ...base.tabs[1], id: "t2" },
        { ...base.tabs[1], id: "t3", provider: "opencode" },
      ],
    }
  }

  it("a drop reports the new order for the agent, the slot tab included", () => {
    render(<AgentTabsStrip session={threeTabs("t1")} activeTabId="t1" maxTabs={20} />)
    expect(sortableProps?.ids).toEqual(["t1", "t2", "t3"])
    sortableProps?.onReorder(["t2", "t3", "t1"])
    expect(reorderTabsMock).toHaveBeenCalledWith("s1", ["t2", "t3", "t1"])
  })

  it("shows the order just dragged before the server confirms it", () => {
    mockState = {
      ...mockState,
      pendingTabOrder: { s1: { ids: ["t3", "t1", "t2"], answered: null } },
    }
    render(<AgentTabsStrip session={threeTabs("t1")} activeTabId="t1" maxTabs={20} />)
    expect(sortableProps?.ids).toEqual(["t3", "t1", "t2"])
    const names = screen.getAllByRole("tab").map((tab) => tab.textContent)
    expect(names[0]).toContain("opencode")
  })

  it("pins the slot tab wherever it sits, and only that one", () => {
    render(<AgentTabsStrip session={threeTabs("t2")} activeTabId="t2" maxTabs={20} />)
    const pinned = screen.getAllByRole("tab").filter((tab) => within(tab).queryByLabelText(PIN))
    expect(pinned).toHaveLength(1)
    expect(pinned[0].getAttribute("aria-selected")).toBe("true")
  })

  it("moves the pin when the slot passes on", () => {
    const { rerender } = render(
      <AgentTabsStrip session={threeTabs("t1")} activeTabId="t1" maxTabs={20} />,
    )
    expect(within(screen.getAllByRole("tab")[0]).queryByLabelText(PIN)).not.toBeNull()
    rerender(<AgentTabsStrip session={threeTabs("t3")} activeTabId="t1" maxTabs={20} />)
    const tabs = screen.getAllByRole("tab")
    expect(within(tabs[0]).queryByLabelText(PIN)).toBeNull()
    expect(within(tabs[2]).queryByLabelText(PIN)).not.toBeNull()
  })

  it("never pins a tab that is on its way out: a promotion this client just made wins", () => {
    mockState = {
      ...mockState,
      pendingSlotTab: { s1: { closedTabId: "t1", promotedTabId: "t2" } },
    }
    render(<AgentTabsStrip session={threeTabs("t1")} activeTabId="t2" maxTabs={20} />)
    const tabs = screen.getAllByRole("tab")
    expect(within(tabs[0]).queryByLabelText(PIN)).toBeNull()
    expect(within(tabs[1]).queryByLabelText(PIN)).not.toBeNull()
  })

  it("keeps a long press from selecting the pill's text or raising the touch callout", () => {
    render(<AgentTabsStrip session={threeTabs("t1")} activeTabId="t1" maxTabs={20} />)
    const pill = screen.getAllByRole("tab")[0]
    expect(pill.className).toContain("select-none")
    expect(pill.className).toContain("[-webkit-touch-callout:none]")
  })

  it("shows no pin while the agent has a single tab", () => {
    const base = session()
    const single = { ...base, tabs: [base.tabs[0]] }
    render(<AgentTabsStrip session={single} activeTabId="s1" maxTabs={20} />)
    expect(screen.queryByLabelText(PIN)).toBeNull()
  })
})

describe("AgentTabsStrip phone height", () => {
  // A deliberate per-axis relaxation of the 40px touch-target floor (see the
  // justification at the pill): the pill drops to 36px VERTICALLY
  // because its vertical neighbours are the header above (no tap target beside a
  // pill) and the PTY below (a cheap mis-tap), while every horizontal size is
  // kept because the horizontal neighbours are other tabs.
  it("pins the pill to 36px tall with no padding of its own", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    for (const pill of screen.getAllByRole("tab")) {
      expect(pill.className).toContain("max-md:min-h-9")
      expect(pill.className).toContain("max-md:py-0")
      expect(pill.className).not.toContain("max-md:min-h-11")
    }
  })

  // A browser check at 390px: Tab onto a pill, "New agent tab" and "Choose provider
  // for the new agent tab"; each ring is whole at the top and bottom, and the strip is
  // 2px taller than the 41px it was.
  it("pads the strip 3px a side, the ring's reach, so the scroller never cuts it", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const strip = screen.getByRole("tablist").parentElement
    expect(strip?.className).toContain("max-md:py-0.75")
    expect(strip?.className).not.toContain("max-md:py-0.5")
  })

  it("keeps every control's 44px WIDTH while relaxing its height", () => {
    // `size-11` on any of these would force the pill or the strip back to 44px
    // tall; dropping the width would put a mis-tap on a neighbouring tab or on
    // the wrong provider.
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const controls = [
      screen.getByLabelText("New agent tab"),
      screen.getByLabelText("Choose provider for the new agent tab"),
      ...screen.getAllByLabelText("Tab actions"),
    ]
    for (const control of controls) {
      expect(control.className).toContain("max-md:w-11")
      expect(control.className).not.toContain("max-md:size-11")
    }
  })

  // The pill's ⋯ said "always revealed on touch" in a comment and was not: the
  // width breakpoint gave the slot back below `md` but nothing ever restored
  // the paint, so a finger got an invisible 44px target at every width. The
  // pointer override is what actually keeps the promise.
  it("always reveals the pill's ⋯ where the pointer is coarse", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const wrapper = screen.getAllByLabelText("Tab actions")[0].closest("div")
    expect(wrapper).not.toBeNull()
    expect(wrapper!.className).toContain("pointer-coarse:max-w-none")
    expect(wrapper!.className).toContain("pointer-coarse:opacity-100")
    // The mouse's rules are untouched: collapsed at rest, opening on hover,
    // focus-within, or while the menu is open.
    expect(wrapper!.className).toContain(HOVER_REVEAL)
    expect(wrapper!.className).toContain(REVEALED_ON_ANY_FOCUS)
    expect(wrapper!.className).toContain("[--reveal-cap:--spacing(8)]")
    expect(screen.getAllByRole("tab")[0].classList).toContain(REVEAL_ROW)
  })
})

// A tablet is a finger at desktop width, so none of the phone's `max-md:`
// sizes reach it. jsdom cannot evaluate media queries; the classes are what can
// be pinned.
describe("AgentTabsStrip on a touch tablet", () => {
  it("makes the pills 36px tall with 8px between them", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    for (const pill of screen.getAllByRole("tab")) {
      expect(pill.className).toContain("md:pointer-coarse:min-h-9")
    }
    const strip = screen.getByRole("tablist").parentElement
    expect(strip?.className).toContain("md:pointer-coarse:gap-2")
  })

  it("gives the ⋯, + and chooser a 40px tap area without growing them", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const margins: Array<[HTMLElement, string]> = [
      // 20px box, 10px a side.
      ...screen
        .getAllByLabelText("Tab actions")
        .map((b): [HTMLElement, string] => [b, "after:-inset-2.5"]),
      // 24px boxes, 8px a side.
      [screen.getByLabelText("New agent tab"), "after:-inset-2"],
      [screen.getByLabelText("Choose provider for the new agent tab"), "after:-inset-2"],
    ]
    for (const [control, inset] of margins) {
      const classes = Array.from(control.classList)
      expect(classes).toEqual(
        expect.arrayContaining([
          "relative",
          "after:absolute",
          inset,
          "after:hidden",
          "md:pointer-coarse:after:block",
        ]),
      )
      // The phone's own sizes already clear the floor's width; a margin there
      // would overlap the neighbours.
      expect(classes).not.toContain("pointer-coarse:after:block")
      expect(classes).not.toContain("max-md:after:block")
    }
  })

  it("lets the ⋯'s tap area out of its reveal wrapper only where it is always revealed", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const wrapper = screen.getAllByLabelText("Tab actions")[0].closest("div")!
    expect(wrapper.className).toContain("pointer-coarse:overflow-visible")
    // A mouse keeps the clip that hides the collapsed trigger.
    expect(wrapper.className).toContain("overflow-hidden")
  })

  // A disabled button takes no pointer events, so its own tap margin is dead;
  // what holds it must carry the 40px area that a tap reads the reason from.
  it("gives a disabled + and chooser the same 40px tap area on what holds them", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={2} />)
    for (const label of ["New agent tab", "Choose provider for the new agent tab"]) {
      const holder = screen.getByLabelText(label).parentElement!
      expect(Array.from(holder.classList)).toEqual(
        expect.arrayContaining([
          "relative",
          "after:absolute",
          "after:-inset-2",
          "after:hidden",
          "md:pointer-coarse:after:block",
        ]),
      )
    }
  })

  it("keeps the + and chooser tap areas apart and the chooser's clear of the pane divider", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const group = screen.getByLabelText("New agent tab").closest("div")!
    expect(group.className).toContain("md:pointer-coarse:gap-4")
    // And the + clear of the last pill's ⋯.
    expect(group.className).toContain("md:pointer-coarse:ml-0.5")
    const strip = screen.getByRole("tablist").parentElement!
    expect(strip.className).toContain("md:pointer-coarse:pr-4.5")
  })
})

// The agent's strip and the terminal dock stack in one pane, so on a computer,
// a tablet, and a phone with a mouse or a finger they must come out the same
// size. jsdom cannot evaluate media queries; equal classes are equal boxes in
// all four cases, and a browser measures them.
describe("the agent and dock strips", () => {
  it("draw their strips, pills, pill menus and + from one box", () => {
    const spine = mockState.spine as unknown as Record<string, unknown>
    mockState = { ...mockState, spine: { ...spine, sessions: [session()] } } as unknown as DuxState
    render(
      <>
        <AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />
        <DockStrip
          sessionId="s1"
          terminals={[
            {
              id: "t1",
              owner: { kind: "session", session_id: "s1" },
              label: "Terminal 1",
              has_output: true,
              working: false,
              typing: false,
              foreground_cmd: null,
              sort_order: 1,
              created_at: "",
              updated_at: "",
            },
          ]}
          activeTerminal="t1"
        />
      </>,
    )
    const agent = screen.getByRole("tablist", { name: "Agent tabs" })
    const dock = screen.getByRole("tablist", { name: "Terminals" })
    const pairs: Array<[HTMLElement, HTMLElement]> = [
      [agent.parentElement!, dock.parentElement!],
      [agent, dock],
      [within(agent).getAllByRole("tab")[0], within(dock).getAllByRole("tab")[0]],
      [screen.getAllByLabelText("Tab actions")[0], screen.getByLabelText("Terminal actions")],
      [
        screen.getAllByLabelText("Tab actions")[0].parentElement!,
        screen.getByLabelText("Terminal actions").parentElement!,
      ],
      [screen.getByLabelText("New agent tab"), screen.getByLabelText("New terminal tab")],
    ]
    for (const [a, d] of pairs) {
      expect(Array.from(d.classList).sort()).toEqual(Array.from(a.classList).sort())
    }
  })
})

// The pill's cue follows its own ladder: needs-you and typing outrank
// working, so a pill flagged for either shows that state and no working cue.
describe("AgentTabsStrip working cue ladder", () => {
  for (const [name, over] of [
    ["needs attention", { needs_attention: true }],
    ["is typing", { typing: true }],
  ] as const) {
    it(`does not pulse or bounce a working tab that ${name}`, () => {
      const s = session()
      s.tabs[1] = { ...s.tabs[1], working: true, ...over }
      const { container } = render(
        <AgentTabsStrip session={s} activeTabId="s1" maxTabs={20} />,
      )
      const icon = container.querySelectorAll("[role='tab'] svg.lucide-bot")[1]
      expect(icon.getAttribute("class")).not.toContain("animate-working")
      expect(
        icon.closest("[data-slot='working-glyph']")!.getAttribute("class"),
      ).not.toContain("animate-working")
    })
  }
})

// USER-LOCKED. A working tab's pill icon bounces with the pulse and finishes its
// bounce at rest on every stop, a hand-over to needs-you included, easing back
// to full opacity like the sidebar list. The USER-LOCKED line in
// `crates/dux-web/web/AGENTS.md` names this guard: do not edit, weaken or
// delete it, or change what it pins, without an explicit request for that
// change.
describe("USER-LOCKED: working glyph bounces with the pulse and settles at rest (agent tab strip)", () => {
  const LOCKED =
    "This is a locked product decision, named by the USER-LOCKED line in " +
    "crates/dux-web/web/AGENTS.md " +
    "(the web working glyph bounces with the pulse, four per pulse, finishes its " +
    "bounce at rest and eases back to full opacity on every stop, a hand-over to " +
    "a higher state included, and takes that state's look only once it has " +
    "landed). Change it only on an explicit request for that specific change; " +
    "editing this test to make it pass is changing the decision."
  const BOUNCE = "motion-safe:animate-working-bounce"
  const PULSE = "motion-safe:animate-working-pulse"

  function withWorking(working: boolean, needsAttention = false): SessionView {
    const s = session()
    s.tabs[1] = { ...s.tabs[1], working, needs_attention: needsAttention }
    return s
  }
  const frameOf = (icon: Element) => icon.closest("[data-slot='working-glyph']")!

  it("bounces a working tab's icon and lets it finish its bounce", () => {
    const { container, rerender } = render(
      <AgentTabsStrip session={withWorking(true)} activeTabId="s1" maxTabs={20} />,
    )
    const icons = () => container.querySelectorAll("[role='tab'] svg.lucide-bot")
    expect(frameOf(icons()[0]).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
    expect(icons()[0].getAttribute("class"), LOCKED).not.toContain(PULSE)
    expect(frameOf(icons()[1]).getAttribute("class"), LOCKED).toContain(BOUNCE)
    expect(icons()[1].getAttribute("class"), LOCKED).toContain(PULSE)
    fireAnimationStart(frameOf(icons()[1]), "working-bounce")

    rerender(<AgentTabsStrip session={withWorking(false)} activeTabId="s1" maxTabs={20} />)
    expect(frameOf(icons()[1]).getAttribute("class"), LOCKED).toContain(BOUNCE)
    fireAnimationIteration(frameOf(icons()[1]), "working-bounce")
    expect(frameOf(icons()[1]).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
  })

  // A turn ending on a prompt: the attention dot is there at once and the
  // pulse is gone at once, but the bounce in flight still finishes at rest and
  // the icon eases back to full opacity rather than snapping.
  it("finishes the bounce and eases the icon when needs-you takes the tab over", () => {
    const { container, rerender } = render(
      <AgentTabsStrip session={withWorking(true)} activeTabId="s1" maxTabs={20} />,
    )
    const pill = () => container.querySelectorAll("[role='tab']")[1]
    const icon = () => pill().querySelector("svg.lucide-bot")!
    const ease = stubGlyphEase(icon())
    fireAnimationStart(frameOf(icon()), "working-bounce")

    rerender(
      <AgentTabsStrip session={withWorking(false, true)} activeTabId="s1" maxTabs={20} />,
    )
    expect(pill().querySelector("[aria-label='Needs attention']"), LOCKED).toBeTruthy()
    expect(icon().getAttribute("class"), LOCKED).not.toContain(PULSE)
    expect(ease.animate, LOCKED).toHaveBeenCalledTimes(1)
    expect(frameOf(icon()).getAttribute("class"), LOCKED).toContain(BOUNCE)
    fireAnimationIteration(frameOf(icon()), "working-bounce")
    expect(frameOf(icon()).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
  })

  it("drops a stopping tab's pulse at once, before the bounce lands", () => {
    const { container, rerender } = render(
      <AgentTabsStrip session={withWorking(true)} activeTabId="s1" maxTabs={20} />,
    )
    const icon = () => container.querySelectorAll("[role='tab'] svg.lucide-bot")[1]
    fireAnimationStart(frameOf(icon()), "working-bounce")

    rerender(<AgentTabsStrip session={withWorking(false)} activeTabId="s1" maxTabs={20} />)
    expect(icon().getAttribute("class"), LOCKED).not.toContain(PULSE)
    expect(frameOf(icon()).getAttribute("class"), LOCKED).toContain(BOUNCE)
  })
})

// A browser check: Tab onto a tab past the strip's right edge and the strip
// scrolls the whole tab into view, with its focus ring whole above and below.
describe("keyboard focus on the strip", () => {
  it("scrolls a tab taking keyboard focus into view, with room for its ring", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const pill = screen.getAllByRole("tab")[1]
    const scroll = vi.fn()
    pill.scrollIntoView = scroll
    const matches = Element.prototype.matches
    const keyboard = vi
      .spyOn(Element.prototype, "matches")
      .mockImplementation(function (this: Element, selector: string) {
        return selector === ":focus-visible" || matches.call(this, selector)
      })
    try {
      act(() => pill.focus())
    } finally {
      keyboard.mockRestore()
    }
    expect(scroll).toHaveBeenCalledTimes(1)
    const strip = screen.getByRole("tablist").parentElement!
    expect(strip.className).toContain("py-1.25")
    expect(strip.className).toContain("scroll-px-2")
  })
})

// A browser check on a computer: focus a tab pill and press ArrowRight and
// ArrowLeft; focus moves to the neighbouring pill and the strip does not scroll
// sideways under it, and nothing happens past either end.
describe("arrow keys between tab pills", () => {
  it("moves focus to the neighbouring pill and stops at the ends", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const pills = screen.getAllByRole("tab")
    expect(pills.length).toBeGreaterThan(1)
    const last = pills.length - 1
    act(() => pills[0].focus())
    fireEvent.keyDown(pills[0], { key: "ArrowLeft" })
    expect(document.activeElement).toBe(pills[0])
    fireEvent.keyDown(pills[0], { key: "ArrowRight" })
    expect(document.activeElement).toBe(pills[1])
    fireEvent.keyDown(pills[1], { key: "ArrowLeft" })
    expect(document.activeElement).toBe(pills[0])
    act(() => pills[last].focus())
    fireEvent.keyDown(pills[last], { key: "ArrowRight" })
    expect(document.activeElement).toBe(pills[last])
  })
})

// What a browser does to a focused button that becomes disabled. jsdom blurs
// only a focusable element, so the blur goes first.
function blurWhenDisabled(button: HTMLElement) {
  const setAttribute = button.setAttribute.bind(button)
  button.setAttribute = (name: string, value: string) => {
    if (name === "disabled") button.blur()
    setAttribute(name, value)
  }
}
