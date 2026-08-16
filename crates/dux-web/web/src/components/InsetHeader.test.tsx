// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setChangesPaneVisibility: vi.fn(),
    showChangesPane: vi.fn(),
  }
})

// The store boots on import and reaches for browser globals; stub them so the
// render stays hermetic and off the network (mirrors the sibling tests).
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
const { InsetHeader } = await import("./InsetHeader")

// The chip a kind renders as, or null when that field is absent. Queried by the
// `data-chip` marker the header stamps on each chip, because a glyph has no
// accessible name to query by and the value alone cannot say WHICH field it is.
function chip(kind: string): HTMLElement | null {
  return document.querySelector(`[data-chip="${kind}"]`)
}
function chipValue(kind: string): string | undefined {
  return chip(kind)?.textContent ?? undefined
}
function chipKindsInOrder(): string[] {
  return [...document.querySelectorAll("[data-chip]")].map(
    (el) => el.getAttribute("data-chip") ?? "",
  )
}

function stateFor(branchName: string, initialBranch: string): DuxState {
  return {
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    changesPanePercent: 26,
    spine: {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: null,
          provider: "claude",
          branch_name: branchName,
          initial_branch: initialBranch,
          source_branch: "main",
          worktree_path: "/tmp/s1",
          status: "active",
          tabs: [
            {
              id: "s1",
              provider: "claude",
              order: 0,
              working: false,
              has_output: false,
              has_live_process: true,
            },
          ],
        },
      ],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe("InsetHeader app menu", () => {
  it("renders the app-menu cog, labelled Settings, instead of a Commands button", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.queryByText(/Commands/)).toBeNull()
    expect(screen.getByRole("button", { name: /^settings$/i })).toBeTruthy()
  })

  it("gives the labelled cog exactly the height of the icon-only buttons beside it", () => {
    // The rule the label must not break: a label changes the WIDTH only. Both
    // the button's default size token and its `icon` token resolve to 32px
    // (`h-8` / `size-8`), and asserting the classes is the only way to check it
    // in jsdom, which computes no layout.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    const settings = screen.getByRole("button", { name: /^settings$/i })
    const reopen = screen.getByRole("button", { name: /show changes pane/i })
    const macros = screen.getByRole("button", { name: /run a macro/i })
    expect(settings.className).toContain("h-8")
    expect(macros.className).toContain("h-8")
    expect(reopen.className).toContain("size-8")
    // …and no control in the row forces its own height on top of the token.
    for (const el of [settings, reopen, macros]) {
      expect(el.className).not.toMatch(/\bh-(9|10|11)\b/)
    }
  })
})

describe("InsetHeader agent chips", () => {
  it("renders a glyph and a value for each field", () => {
    const titled = stateFor("feature-x", "")
    titled.spine!.sessions[0].title = "Tab redesign"
    mockState = titled
    render(<InsetHeader />)
    expect(chipValue("project")).toBe("Repo")
    expect(chipValue("agent")).toBe("Tab redesign")
    expect(chipValue("branch")).toBe("feature-x")
    expect(chipValue("assistant")).toBe("claude")
    // A glyph, not a word: no field prints its own label into the bar.
    for (const kind of ["project", "agent", "branch", "assistant"]) {
      expect(chip(kind)!.querySelector("svg"), kind).toBeTruthy()
    }
    expect(screen.queryByText(/agent:/)).toBeNull()
    expect(screen.queryByText(/provider:/)).toBeNull()
    expect(screen.queryByText(/branch:/)).toBeNull()
    expect(screen.queryByText(/project:/)).toBeNull()
  })

  it("orders the chips project, agent, branch, terminals, assistant", () => {
    const titled = stateFor("feature-x", "")
    titled.spine!.sessions[0].title = "Tab redesign"
    titled.spine!.terminals = [
      {
        id: "t1",
        owner: { kind: "session", session_id: "s1" },
        label: "Terminal 1",
      },
      {
        id: "t2",
        owner: { kind: "session", session_id: "s1" },
        label: "Terminal 2",
      },
    ] as never
    mockState = titled
    render(<InsetHeader />)
    expect(chipKindsInOrder()).toEqual([
      "project",
      "agent",
      "branch",
      "terminal",
      "assistant",
    ])
  })

  it("drops the branch chip when the branch merely repeats the agent name", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(chipValue("agent")).toBe("main")
    expect(chip("branch")).toBeNull()
    // The words it used to print instead are gone with the caption.
    expect(screen.queryByText(/same branch/)).toBeNull()
  })

  it("shows the terminals chip only when the agent owns terminals", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(chip("terminal")).toBeNull()
    cleanup()

    const withTerminals = stateFor("main", "main")
    withTerminals.spine!.terminals = [
      {
        id: "t1",
        owner: { kind: "session", session_id: "s1" },
        label: "Terminal 1",
      },
      {
        id: "t2",
        owner: { kind: "session", session_id: "s1" },
        label: "Terminal 2",
      },
    ] as never
    mockState = withTerminals
    render(<InsetHeader />)
    expect(chipValue("terminal")).toBe("2")
  })

  it("wires every chip to a hover label", () => {
    // The glyphs are only learnable because each one names itself, so every chip
    // must actually be a tooltip trigger. Asserted as base-ui's own marker: a
    // chip that quietly stopped being wrapped would still render its value and
    // look completely fine.
    const titled = stateFor("feature-x", "")
    titled.spine!.sessions[0].title = "Tab redesign"
    mockState = titled
    render(<InsetHeader />)
    const chips = [...document.querySelectorAll("[data-chip]")]
    expect(chips.length).toBe(4)
    for (const el of chips) {
      expect(
        el.getAttribute("data-slot"),
        `${el.getAttribute("data-chip")} is not a tooltip trigger`,
      ).toBe("tooltip-trigger")
    }
  })

  it("lets every other chip give way before the agent name does", () => {
    // Asserted as the classes that carry it: a layout rule no CSS reads is a
    // rule that silently stops working.
    const titled = stateFor("feature-x", "")
    titled.spine!.sessions[0].title = "Tab redesign"
    mockState = titled
    render(<InsetHeader />)
    const agent = chip("agent")!
    expect(agent.className).toContain("min-w-0")
    expect(agent.className).toContain("shrink")
    expect(agent.className).not.toContain("shrink-[9999]")
    for (const kind of ["project", "branch", "assistant"]) {
      expect(chip(kind)!.className, kind).toContain("shrink-[9999]")
    }
    // One font and one size: the chips are peers, so nothing here is
    // distinguished by font or by type scale.
    for (const el of document.querySelectorAll("[data-chip]")) {
      expect(el.className).toContain("text-sm")
      expect(el.className).not.toContain("font-mono")
    }
  })

  it("draws no hairline divider between the fields", () => {
    // The glyph IS the separator; a rule would only spend pixels. A wider gap
    // than the header's own does the spacing.
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    const row = chip("agent")!.parentElement!
    expect(row.className).toContain("gap-3.5")
    expect(row.querySelectorAll("[data-chip]").length).toBe(
      row.children.length,
    )
  })
})

describe("InsetHeader branch drift cue", () => {
  it("keeps the original branch off the bar and on the branch chip's hover clause", () => {
    const titled = stateFor("agent-tabs", "server-mode")
    titled.spine!.sessions[0].title = "Tab redesign"
    mockState = titled
    render(<InsetHeader />)
    expect(chipValue("branch")).toBe("agent-tabs")
    // The drift is a hover clause now, not bar text: it must not be printed.
    expect(screen.queryByText(/originally/)).toBeNull()
  })

  it("keeps a branch chip at all when the branch drifted but matches the agent name", () => {
    // Without this the drift note has nowhere to live and the fact is dropped.
    mockState = stateFor("main", "server-mode")
    render(<InsetHeader />)
    expect(chipValue("branch")).toBe("main")
  })

  it("shows no branch chip when nothing about the branch is worth saying", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(chip("branch")).toBeNull()
  })
})

describe("InsetHeader chip tooltips", () => {
  it("offers nothing to reveal while the value is fully readable", async () => {
    // jsdom computes no layout, so scrollWidth === clientWidth === 0: nothing is
    // truncated and no chip may offer to repeat what the user can already read.
    const { headerChipTooltip } = await import("@/lib/headerSubject")
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(
      headerChipTooltip(
        { kind: "project", label: "Project", value: "Repo" },
        false,
      ),
    ).toBe("Project")
    expect(document.body.textContent).not.toContain("Project · Repo")
  })

  it("measures the overflow itself rather than assuming it", async () => {
    // Truncation is a MEASUREMENT (scroll width against client width), which is
    // what makes "only when actually cut off" possible at all. Drive it: make
    // every element report an overflowing scroll width and the hook must read
    // both sides.
    const scrollWidth = vi
      .spyOn(HTMLElement.prototype, "scrollWidth", "get")
      .mockReturnValue(400)
    const clientWidth = vi
      .spyOn(HTMLElement.prototype, "clientWidth", "get")
      .mockReturnValue(40)
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(scrollWidth).toHaveBeenCalled()
    expect(clientWidth).toHaveBeenCalled()
    const { headerChipTooltip } = await import("@/lib/headerSubject")
    expect(
      headerChipTooltip(
        { kind: "project", label: "Project", value: "Repo" },
        true,
      ),
    ).toBe("Project · Repo")
  })
})

describe("InsetHeader macros and the pane-edge spacer", () => {
  it("puts the macro trigger before a spacer sized to the Changes panel", () => {
    mockState = { ...stateFor("main", "main"), changesPanePercent: 31 }
    render(<InsetHeader />)
    const macros = screen.getByRole("button", { name: /run a macro/i })
    const spacer = macros.nextElementSibling as HTMLElement
    expect(spacer.style.width).toBe("31%")
  })

  it("tracks a dragged divider, because the width is the live percentage", () => {
    mockState = { ...stateFor("main", "main"), changesPanePercent: 44.5 }
    render(<InsetHeader />)
    const macros = screen.getByRole("button", { name: /run a macro/i })
    expect((macros.nextElementSibling as HTMLElement).style.width).toBe("44.5%")
  })

  it("collapses the spacer to zero when the Changes pane is hidden", () => {
    // The button slides right with the terminal pane that just grew under it.
    mockState = {
      ...stateFor("main", "main"),
      changesPanePercent: 26,
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    const macros = screen.getByRole("button", { name: /run a macro/i })
    expect((macros.nextElementSibling as HTMLElement).style.width).toBe("0%")
  })

  it("puts the controls INSIDE the spacer, right-aligned, with a floor", () => {
    // The spacer is the control cluster, not an empty box in front of it: an
    // empty one would push Macros left by the cluster's own width and only
    // pixel math could correct it. Right-aligning inside a
    // Changes-panel-sized box is what puts the cog on the window's edge and
    // Macros on the pane's. `min-w-fit` is the floor that survives both the
    // hidden pane (0%) and a pane dragged narrower than the buttons.
    mockState = { ...stateFor("main", "main"), changesPanePercent: 26 }
    render(<InsetHeader />)
    const macros = screen.getByRole("button", { name: /run a macro/i })
    const cluster = macros.nextElementSibling as HTMLElement
    expect(cluster.contains(screen.getByRole("button", { name: /^settings$/i })))
      .toBe(true)
    expect(cluster.className).toContain("justify-end")
    expect(cluster.className).toContain("min-w-fit")
    expect(cluster.className).toContain("shrink-0")
  })

  it("rules the header at the pane boundary so Macros does not float", () => {
    // The hairline is its own absolutely positioned element at right: spacer%.
    // Absolute offsets resolve against the header's FULL box while the old
    // border-l resolved against the padded interior, which drew the rule a few
    // pixels left of the panel divider below (caught from a real screenshot).
    // jsdom computes no layout, so the classes and inline offset are the check.
    mockState = { ...stateFor("main", "main"), changesPanePercent: 26 }
    render(<InsetHeader />)
    const rule = screen.getByTestId("changes-divider-continuation")
    expect(rule.className).toContain("absolute")
    expect(rule.className).toContain("inset-y-0")
    expect(rule.className).toContain("bg-border")
    expect(rule.className).toContain("pointer-events-none")
    // 26% of the full width minus the 1px panel handle's share: the panes
    // split (width - 1px), so a pure percentage sits spacer/100 px left of
    // the divider, one visible pixel under browser zoom.
    expect(rule.style.right).toBe("calc(26% - 0.26px)")
  })

  it("draws no rule when the Changes pane is hidden", () => {
    // Nothing below to continue: the pane is gone and the spacer has collapsed
    // to the control cluster, so a rule would just float in the header.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.queryByTestId("changes-divider-continuation")).toBeNull()
  })

  it("renders no macro trigger when nothing is focused", () => {
    mockState = {
      selectedSessionId: null,
      selectedTarget: null,
      changesPanePercent: 26,
      spine: { projects: [], sessions: [] },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.queryByRole("button", { name: /run a macro/i })).toBeNull()
  })
})

describe("InsetHeader project terminal chips", () => {
  it("renders project and terminal chips for a focused project terminal", () => {
    // The trap this guards (T8): every field was gated on a resolved SESSION,
    // so a focused project terminal rendered a completely blank bar.
    mockState = {
      selectedSessionId: null,
      changesPanePercent: 26,
      selectedTarget: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        sessions: [],
        terminals: [
          {
            id: "pt-1",
            owner: { kind: "project", project_id: "p1" },
            label: "Terminal 2",
            has_output: true,
            foreground_cmd: null,
          },
        ],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(chipKindsInOrder()).toEqual(["project", "terminal"])
    expect(chipValue("project")).toBe("Repo")
    expect(chipValue("terminal")).toBe("Terminal 2")
    expect(screen.queryByText(/project:/)).toBeNull()
    expect(screen.queryByText(/terminal:/)).toBeNull()
  })
})

describe("InsetHeader session terminal chips", () => {
  it("keeps the owning agent's fields and hands the primary slot to the terminal", () => {
    const base = stateFor("main", "main")
    mockState = {
      ...base,
      selectedSessionId: "s1",
      selectedTarget: {
        kind: "terminal",
        terminalId: "t1",
        owner: { kind: "session", sessionId: "s1" },
      },
      spine: {
        ...base.spine,
        terminals: [
          {
            id: "t1",
            owner: { kind: "session", session_id: "s1" },
            label: "Terminal 1",
            has_output: true,
            foreground_cmd: null,
          },
        ],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    // One terminal glyph, not two: the focused terminal's chip replaces the
    // agent's count rather than sitting beside it.
    expect(chipKindsInOrder()).toEqual([
      "project",
      "agent",
      "terminal",
      "assistant",
    ])
    expect(chipValue("terminal")).toBe("Terminal 1")
    expect(chip("terminal")!.className).not.toContain("shrink-[9999]")
    expect(chip("agent")!.className).toContain("shrink-[9999]")
  })
})

describe("InsetHeader standalone terminal chips", () => {
  it("names the directory for a focused standalone terminal", () => {
    // It has no owner to name, so a header that only knows how to name owners
    // would render blank. The directory is what it says instead, and it is the
    // same string its sidebar row shows.
    mockState = {
      selectedSessionId: null,
      changesPanePercent: 26,
      selectedTarget: {
        kind: "terminal",
        terminalId: "solo-1",
        owner: { kind: "standalone" },
      },
      spine: {
        projects: [],
        sessions: [],
        terminals: [
          {
            id: "solo-1",
            owner: { kind: "standalone", cwd_label: "~/code" },
            label: "Terminal 1",
            has_output: true,
            foreground_cmd: null,
          },
        ],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(chipKindsInOrder()).toEqual(["directory", "terminal"])
    expect(chipValue("directory")).toBe("~/code")
    expect(chipValue("terminal")).toBe("Terminal 1")
    expect(screen.queryByText(/directory:/)).toBeNull()
  })
})

describe("InsetHeader show-Changes button", () => {
  // The pane's only in-app reopen control used to live inside the pane's own
  // header menu, which unmounts with the pane; this button is the always-there
  // way back (the sidebar rail-button pattern applied to the right panel).
  it("renders only while the Changes pane is hidden, and clicking it shows the pane", async () => {
    const store = await import("@/lib/store")
    const show = vi.mocked(store.showChangesPane)
    show.mockClear()

    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    fireEvent.click(button)
    // The healing show, not the bare preference write: a pane that was dragged
    // to nothing must come back at a width, not at zero.
    expect(show).toHaveBeenCalled()
  })

  it("renders when the preference says visible but the pane is zero-width", async () => {
    // The stuck state this gate exists for: a divider dragged off the edge left
    // the pane at 0% while the preference still said "visible", so the button
    // (gated on the preference alone) stayed away and the pane's own hide item
    // was inside the zero. Nothing on screen could bring it back.
    const store = await import("@/lib/store")
    const show = vi.mocked(store.showChangesPane)
    show.mockClear()

    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: true },
      changesPanePercent: 0,
    } as unknown as DuxState
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    fireEvent.click(button)
    expect(show).toHaveBeenCalled()
  })

  it("draws no pane-boundary rule while the pane is zero-width", () => {
    // Same reason as the hidden case: there is no divider below to continue.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: true },
      changesPanePercent: 0,
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.queryByTestId("changes-divider-continuation")).toBeNull()
  })

  it("does not render while the Changes pane is visible", () => {
    // stateFor carries no bootstrap and no override, so changesPaneVisible
    // resolves to the pre-load default: visible.
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(
      screen.queryByRole("button", { name: /show changes pane/i }),
    ).toBeNull()
  })

  it("does not render when the saved preference shows the pane", () => {
    // The common loaded state: config says visible, no override in play.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: true },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(
      screen.queryByRole("button", { name: /show changes pane/i }),
    ).toBeNull()
  })

  it("does not render while the optimistic show override covers a stale hidden bootstrap", () => {
    // The moment right after the click: the persist is in flight, the
    // bootstrap still says hidden, and the optimistic override already says
    // shown. The pane is on screen, so the button must already be gone.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
      changesPaneOverride: true,
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(
      screen.queryByRole("button", { name: /show changes pane/i }),
    ).toBeNull()
  })
})

describe("InsetHeader geometry", () => {
  it("stays exactly 48px tall and never lets the chips push the controls off", () => {
    mockState = stateFor("main", "main")
    const { container } = render(<InsetHeader />)
    const header = container.querySelector("header")!
    expect(header.className).toContain("h-12")
    expect(header.className).toContain("shrink-0")
    // Buttons win: the chip row owns the whole shrink budget.
    const row = chip("agent")!.parentElement!
    expect(row.className).toContain("min-w-0")
    expect(row.className).toContain("flex-1")
    expect(row.className).toContain("overflow-hidden")
    const controls = screen.getByRole("button", { name: /^settings$/i })
      .parentElement!
    expect(controls.className).toContain("shrink-0")
  })
})
