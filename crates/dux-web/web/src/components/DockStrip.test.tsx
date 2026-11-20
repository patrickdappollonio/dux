// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import { agentRowKey, registerPaneCover, resetPaneCovers } from "@/lib/paneCover"
import type { TerminalView } from "@/lib/types"
import {
  fireAnimationIteration,
  fireAnimationStart,
  stubGlyphEase,
} from "@/test/animationEvents"

const selectTerminalMock = vi.fn()
const reorderDockMock = vi.fn()
const openDeleteTerminalMock = vi.fn()
const createTerminalMock = vi.fn()
// The real strip, with its props captured: a drag cannot run under jsdom (the
// pure part of it is in SortableStrip.test.tsx), so the tests hand the strip
// the drop its drag would have produced.
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
    selectTerminal: (...args: unknown[]) => selectTerminalMock(...args),
    reorderDock: (...args: unknown[]) => reorderDockMock(...args),
    openDeleteTerminal: (...args: unknown[]) => openDeleteTerminalMock(...args),
    createTerminal: (...args: unknown[]) => createTerminalMock(...args),
  }
})

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
const { DockStrip } = await import("./DockStrip")

function terminal(id: string, sortOrder: number, working = false): TerminalView {
  return {
    id,
    owner: { kind: "session", session_id: "s1" },
    label: `Terminal ${sortOrder}`,
    has_output: true,
    working,
    typing: false,
    foreground_cmd: null,
    sort_order: sortOrder,
    created_at: "",
    updated_at: "",
  }
}

function strip(terminals: TerminalView[], activeTerminal = "t1") {
  return (
    <DockStrip
      sessionId="s1"
      terminals={terminals}
      activeTerminal={activeTerminal}
      layout="split"
      onLayoutAction={() => {}}
    />
  )
}

beforeEach(() => {
  selectTerminalMock.mockClear()
  reorderDockMock.mockClear()
  openDeleteTerminalMock.mockClear()
  createTerminalMock.mockClear()
  sortableProps = null
})

afterEach(cleanup)

describe("DockStrip", () => {
  it("draws one pill per terminal in the order it is given and brings a pressed one to the front", () => {
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
    const pills = screen.getAllByRole("tab")
    expect(pills.map((p) => p.textContent)).toEqual(["Terminal 1", "Terminal 2"])
    expect(pills[0].getAttribute("aria-selected")).toBe("true")
    fireEvent.click(pills[1])
    expect(selectTerminalMock).toHaveBeenCalledWith("t2", { kind: "session", sessionId: "s1" })
  })

  it("hands a drag's new order to the dock and nothing else", () => {
    render(strip([terminal("t1", 1), terminal("t2", 2), terminal("t3", 3)]))
    expect(sortableProps?.ids).toEqual(["t1", "t2", "t3"])
    sortableProps?.onReorder(["t3", "t1", "t2"])
    expect(reorderDockMock).toHaveBeenCalledWith("s1", ["t3", "t1", "t2"])
  })

  it("gives each pill a menu of that terminal's own actions only", async () => {
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
    fireEvent.click(screen.getAllByLabelText("Terminal actions")[1])
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "Close terminal…",
    ])
    fireEvent.click(menu.getByText("Close terminal…"))
    expect(openDeleteTerminalMock).toHaveBeenCalledWith("t2")
    expect(selectTerminalMock).not.toHaveBeenCalled()
  })

  it("gives a terminal the workspace does not list yet no menu, so no Close that does nothing", () => {
    render(
      <DockStrip
        sessionId="s1"
        terminals={[terminal("t1", 1), terminal("t9", 2)]}
        activeTerminal="t9"
        unlisted="t9"
      />,
    )
    expect(screen.getAllByLabelText("Terminal actions")).toHaveLength(1)
  })

  it("lifts every control on the strip to the touch floor under a finger at phone width", () => {
    render(strip([terminal("t1", 1)]))
    for (const label of ["New terminal", "Hide terminals", "Maximize terminal", "Terminal actions"]) {
      const classes = Array.from(screen.getByLabelText(label).classList)
      expect(classes, label).toContain("max-md:pointer-coarse:size-10")
      expect(classes, `${label} keeps its box on a tablet`).not.toContain("pointer-coarse:size-10")
    }
  })

  it("opens another terminal from its +", () => {
    render(strip([terminal("t1", 1)]))
    fireEvent.click(screen.getByLabelText("New terminal"))
    expect(createTerminalMock).toHaveBeenCalledWith("s1")
  })

  it.each([
    ["split", ["Hide terminals", "Maximize terminal"]],
    ["agent", ["Show terminals"]],
    ["terminal", ["Restore the split"]],
  ] as const)("offers the layout controls of the %s layout", (layout, labels) => {
    const onLayoutAction = vi.fn()
    render(
      <DockStrip
        sessionId="s1"
        terminals={[terminal("t1", 1)]}
        activeTerminal="t1"
        layout={layout}
        onLayoutAction={onLayoutAction}
      />,
    )
    for (const label of labels) expect(screen.getByLabelText(label)).toBeTruthy()
    fireEvent.click(screen.getByLabelText(labels[0]))
    expect(onLayoutAction).toHaveBeenCalledTimes(1)
  })
})

// USER-LOCKED. A running terminal's pill icon bounces with the pulse and
// finishes its bounce at rest on every stop, easing back to full opacity like
// the sidebar list and the agent tab strip. This guard is named by CLAUDE.md's
// "Locked by the user" section. Do not edit, weaken or delete it, or change
// what it pins, without the user's explicit request.
// The dock's order and terminals are the agent's own, so they change only on a
// device that drives the agent's row.
describe("DockStrip on a row this device does not drive", () => {
  afterEach(() => resetPaneCovers())

  it.each(["other", "nobody", "pending"] as const)(
    "disables the +, the reorder and every pill's actions while the row's answer is %s",
    async (driver) => {
      registerPaneCover("s1", driver === "pending" ? "spinner" : "card", {
        row: agentRowKey("s1"),
        driver,
      })
      render(strip([terminal("t1", 1), terminal("t2", 2)]))
      expect(screen.getByLabelText("New terminal")).toHaveProperty("disabled", true)
      expect(sortableProps?.disabled).toBe(true)
      fireEvent.click(screen.getAllByLabelText("Terminal actions")[1])
      const menu = within(await screen.findByRole("menu"))
      const close = menu.getByText("Close terminal…").closest("[role='menuitem']")
      expect(close?.getAttribute("aria-disabled")).toBe("true")
    },
  )

  it("keeps all of it on a row this device drives", () => {
    registerPaneCover("s1", "none", { row: agentRowKey("s1"), driver: "this" })
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
    expect(screen.getByLabelText("New terminal")).toHaveProperty("disabled", false)
    expect(sortableProps?.disabled).toBe(false)
  })
})

describe("USER-LOCKED: working glyph bounces with the pulse and settles at rest (terminal dock strip)", () => {
  const LOCKED =
    'This is a user-locked decision recorded in CLAUDE.md "Locked by the user" ' +
    "(the web working glyph bounces with the pulse, four per pulse, finishes its " +
    "bounce at rest and eases back to full opacity on every stop, a hand-over to " +
    "a higher state included, and takes that state's look only once it has " +
    "landed). Do not change it without the user's explicit request."
  const BOUNCE = "motion-safe:animate-working-bounce"
  const PULSE = "motion-safe:animate-working-pulse"
  const frameOf = (icon: Element) => icon.closest("[data-slot='working-glyph']")!
  const icons = (container: HTMLElement) =>
    container.querySelectorAll("[role='tab'] svg.lucide-square-terminal")

  it("bounces a running terminal's icon and lets it finish its bounce", () => {
    const { container, rerender } = render(
      strip([terminal("t1", 1), terminal("t2", 2, true)]),
    )
    expect(frameOf(icons(container)[0]).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
    expect(icons(container)[0].getAttribute("class"), LOCKED).not.toContain(PULSE)
    expect(frameOf(icons(container)[1]).getAttribute("class"), LOCKED).toContain(BOUNCE)
    expect(icons(container)[1].getAttribute("class"), LOCKED).toContain(PULSE)
    fireAnimationStart(frameOf(icons(container)[1]), "working-bounce")

    rerender(strip([terminal("t1", 1), terminal("t2", 2, false)]))
    expect(frameOf(icons(container)[1]).getAttribute("class"), LOCKED).toContain(BOUNCE)
    fireAnimationIteration(frameOf(icons(container)[1]), "working-bounce")
    expect(frameOf(icons(container)[1]).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
  })

  // Typing takes a running terminal over: the pulse yields at once, but the
  // bounce in flight still finishes at rest and the icon eases back to full
  // opacity rather than snapping.
  it("finishes the bounce and eases the icon when typing takes the terminal over", () => {
    const { container, rerender } = render(
      strip([terminal("t1", 1), terminal("t2", 2, true)]),
    )
    const icon = () => icons(container)[1]
    const ease = stubGlyphEase(icon())
    fireAnimationStart(frameOf(icon()), "working-bounce")

    rerender(strip([terminal("t1", 1), { ...terminal("t2", 2, true), typing: true }]))
    expect(icon().getAttribute("class"), LOCKED).not.toContain(PULSE)
    expect(ease.animate, LOCKED).toHaveBeenCalledTimes(1)
    expect(frameOf(icon()).getAttribute("class"), LOCKED).toContain(BOUNCE)
    fireAnimationIteration(frameOf(icon()), "working-bounce")
    expect(frameOf(icon()).getAttribute("class"), LOCKED).not.toContain(BOUNCE)
  })
})

// The agent tab strip's tablet shape, so the two strips stacked in one pane
// match: jsdom cannot evaluate media queries, so the classes are what can be
// pinned, and a browser must measure the 36px pills and 40px tap areas.
describe("DockStrip on a touch tablet", () => {
  it("makes the pills 36px tall with 8px between them", () => {
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
    for (const pill of screen.getAllByRole("tab")) {
      expect(pill.className).toContain("md:pointer-coarse:min-h-9")
    }
    expect(screen.getByTestId("dock-strip").className).toContain("md:pointer-coarse:gap-2")
  })

  it("gives the ⋯, + and layout controls a 40px tap area without growing them", () => {
    render(strip([terminal("t1", 1)]))
    const margins: Array<[string, string]> = [
      // 20px box, 10px a side.
      ["Terminal actions", "after:-inset-2.5"],
      // 24px boxes, 8px a side.
      ["New terminal", "after:-inset-2"],
      ["Hide terminals", "after:-inset-2"],
      ["Maximize terminal", "after:-inset-2"],
    ]
    for (const [label, inset] of margins) {
      const classes = Array.from(screen.getByLabelText(label).classList)
      expect(classes, label).toEqual(
        expect.arrayContaining([
          "relative",
          "after:absolute",
          inset,
          "after:hidden",
          "md:pointer-coarse:after:block",
        ]),
      )
      expect(classes, label).not.toContain("pointer-coarse:after:block")
    }
  })

  it("lets the ⋯'s tap area out of its reveal wrapper, keeping the mouse's clip", () => {
    render(strip([terminal("t1", 1)]))
    const wrapper = screen.getByLabelText("Terminal actions").closest("div")!
    expect(wrapper.className).toContain("pointer-coarse:overflow-visible")
    expect(wrapper.className).toContain("overflow-hidden")
  })

  it("keeps the tap areas apart and the last one clear of the pane divider", () => {
    render(strip([terminal("t1", 1)]))
    // The + clear of the last pill's ⋯.
    expect(screen.getByLabelText("New terminal").className).toContain("md:pointer-coarse:ml-0.5")
    const group = screen.getByLabelText("Hide terminals").parentElement!
    expect(group.className).toContain("md:pointer-coarse:gap-4")
    expect(screen.getByTestId("dock-strip").className).toContain("md:pointer-coarse:pr-4.5")
  })
})

// A browser check: Tab onto a tab past the strip's right edge and the strip
// scrolls the whole tab into view, with its focus ring whole above and below.
describe("keyboard focus on the strip", () => {
  it("scrolls a tab taking keyboard focus into view, with room for its ring", () => {
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
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
    expect(screen.getByTestId("dock-strip").className).toContain("py-1.25")
    expect(screen.getByTestId("dock-strip").className).toContain("scroll-px-2")
  })
})

// A browser check on a computer: focus a tab pill and press ArrowRight and
// ArrowLeft; focus moves to the neighbouring pill and the strip does not scroll
// sideways under it, and nothing happens past either end.
describe("arrow keys between tab pills", () => {
  it("moves focus to the neighbouring pill and stops at the ends", () => {
    render(strip([terminal("t1", 1), terminal("t2", 2)]))
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
