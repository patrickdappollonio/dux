// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { readFileSync } from "node:fs"

import type { DuxState, SelectedTarget } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

// THE DOCKED FLAP, the phone's action cluster while theater is off. What is
// pinned here is what the mock is a spec about: which four controls it carries,
// that its silhouette is generated from its own measured box rather than drawn
// at a fixed width, and that its body takes the color of whichever band it is
// hanging from.

let mockState: DuxState
const toggleTheaterMock = vi.fn()
const openChangesScreenMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    toggleTheater: (...a: unknown[]) => toggleTheaterMock(...a),
    openChangesScreen: (...a: unknown[]) => openChangesScreenMock(...a),
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
}
installBootStubs()
const { MobileActionFlap } = await import("./MobileActionFlap")

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [{ id: "s1", provider: "claude" } as unknown as AgentTabView],
  } as unknown as SessionView
}

const target: SelectedTarget = { kind: "agent", sessionId: "s1", tabId: "s1" }

function makeState(over: Partial<DuxState> = {}): DuxState {
  return {
    spine: {
      projects: [{ id: "p1", name: "dux" }],
      sessions: [session()],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: { title: "dux", agent_tabs_max: 4, available_providers: ["claude"] },
    selectedSessionId: "s1",
    selectedTarget: target,
    theater: false,
    createTabInFlight: [],
    changes: {
      sessionId: "s1",
      phase: "loaded",
      staged: [],
      unstaged: [{ path: "a" }, { path: "b" }, { path: "c" }],
    },
    ...over,
  } as unknown as DuxState
}

/// The `z-N` utility the class list carries, as a number to compare.
function zLevel(className: string): number {
  const found = /(?:^| )z-(\d+)(?: |$)/.exec(className)
  if (!found) throw new Error(`no z utility in ${className}`)
  return Number(found[1])
}

/// jsdom lays nothing out, so the flap's own box reads zero and no silhouette
/// is drawn. Give it a real one, the way a browser would.
function stubBox(width: number, height: number) {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => width,
  })
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => height,
  })
}

beforeEach(() => {
  installBootStubs()
  toggleTheaterMock.mockReset()
  openChangesScreenMock.mockReset()
  mockState = makeState()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  // @ts-expect-error restoring jsdom's own zero-size getters
  delete HTMLElement.prototype.offsetWidth
  // @ts-expect-error restoring jsdom's own zero-size getters
  delete HTMLElement.prototype.offsetHeight
})

describe("the docked action flap", () => {
  it("carries the four controls the header gave up, and nothing else", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    expect(screen.getByLabelText("Theater mode")).toBeTruthy()
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
    expect(screen.getByLabelText("3 changed files")).toBeTruthy()
    expect(screen.getByLabelText("Session actions")).toBeTruthy()
  })

  it("prints the BARE count beside the diff glyph, which already draws the ±", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    const count = screen.getByTestId("mobile-changes-count")
    expect(count.textContent).toBe("3")
    expect(count.textContent).not.toContain("±")
  })

  it("keeps every control on the 40px touch floor", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    for (const label of ["Theater mode", "Run a macro", "Session actions"]) {
      expect(screen.getByLabelText(label).className).toContain("size-10")
    }
    // The count is the one control wider than it is tall, because it is data.
    expect(screen.getByTestId("mobile-changes-count").className).toContain("h-10")
    expect(screen.getByTestId("mobile-changes-count").className).toContain("w-auto")
  })

  it("opens the changes screen from the count", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    fireEvent.click(screen.getByTestId("mobile-changes-count"))
    expect(openChangesScreenMock).toHaveBeenCalled()
  })

  it("asks the store for theater rather than toggling anything itself", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    fireEvent.click(screen.getByLabelText("Theater mode"))
    expect(toggleTheaterMock).toHaveBeenCalled()
  })

  it("stacks both theater icons so the flight has something to morph between", () => {
    const { container } = render(
      <MobileActionFlap target={target} session={session()} band="strip" />,
    )
    expect(container.querySelector(".dux-ic-max")).not.toBeNull()
    expect(container.querySelector(".dux-ic-min")).not.toBeNull()
  })

  it("draws no silhouette until it has been measured", () => {
    const { container } = render(
      <MobileActionFlap target={target} session={session()} band="strip" />,
    )
    expect(container.querySelector("svg path[stroke]")).toBeNull()
  })

  it("generates the silhouette from its own measured box", () => {
    stubBox(196, 50)
    const { container } = render(
      <MobileActionFlap target={target} session={session()} band="strip" />,
    )
    const svg = container.querySelector("svg[viewBox]") as SVGSVGElement | null
    expect(svg).not.toBeNull()
    // 196 body + a fillet and an overhang on each side.
    expect(svg?.getAttribute("viewBox")).toBe("0 0 226 53")
    // One fill and one OPEN stroke: nothing crosses the top, which is where the
    // band is.
    const stroke = container.querySelector("path[stroke]")
    expect(stroke?.getAttribute("d")).not.toContain("Z")
  })

  it("takes the band's own color, and the plain background with no band", () => {
    const strip = render(
      <MobileActionFlap target={target} session={session()} band="strip" />,
    )
    expect(
      (strip.getByTestId("mobile-action-flap") as HTMLElement).style.getPropertyValue(
        "--dux-flap-fill",
      ),
    ).toBe("var(--dux-flap-bg)")
    cleanup()
    const plain = render(
      <MobileActionFlap target={target} session={session()} band="plain" />,
    )
    expect(
      (plain.getByTestId("mobile-action-flap") as HTMLElement).style.getPropertyValue(
        "--dux-flap-fill",
      ),
    ).toBe("var(--background)")
  })

  it("paints over the chrome stack, so it can interrupt the band's hairline", () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    const box = screen.getByTestId("mobile-action-flap")
    expect(box.className).toContain("-top-px")
    expect(box.className).toContain("z-30")
  })

  it("paints over the pane's full-pane covers, which is what it is for", () => {
    // The flap is the ONLY surface carrying these controls while a cover owns
    // the terminal (the pane's overlay slot is withheld there), and a watcher
    // reaching the session's actions is exactly that state. Neither the pane's
    // root nor the shell's column starts a stacking context, so the two levels
    // are compared directly and the later element in the document wins a tie.
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    const flapZ = zLevel(screen.getByTestId("mobile-action-flap").className)
    const pane = readFileSync("src/components/TerminalPane.tsx", "utf8")
    const covers = [...pane.matchAll(/absolute inset-0 z-(\d+)/g)].map((m) =>
      Number(m[1]),
    )
    expect(covers.length).toBeGreaterThan(0)
    for (const cover of covers) expect(flapZ).toBeGreaterThan(cover)
  })
})
