// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"

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
let sortableProps: { ids: string[]; onReorder: (next: string[]) => void } | null = null
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
