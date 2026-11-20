// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    // Every state here is a partial; the stack's per-agent maps and the tabs
    // being created start empty.
    useDux: () => ({ stackTabs: {}, createTabInFlight: [], ...mockState }),
    setChangesPaneVisibility: vi.fn(),
    showChangesPane: vi.fn(),
    addTab: vi.fn(),
    createTerminal: vi.fn(),
    toggleTheater: vi.fn(),
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
const store = await import("@/lib/store")
const { registerPaneInputGroup, resetPaneInputGroups } = await import(
  "@/lib/paneInputGroup"
)
const { agentRowKey, registerPaneCover, resetPaneCovers } = await import("@/lib/paneCover")

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
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: branchName,
            initial_branch: initialBranch,
            branch_provenance: "created",
            source_branch: "main",
            worktree_path: "/tmp/s1",
          },
          title: null,
          provider: "claude",
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
  resetPaneInputGroups()
})

afterEach(() => {
  cleanup()
  resetPaneInputGroups()
  resetPaneCovers()
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
    // The theater button is labelled too ("Theater" beside its glyph), so it
    // is the second control whose label may only change its width.
    const theater = screen.getByRole("button", { name: /theater mode/i })
    const paneMenu = screen.getByRole("button", { name: /session actions/i })
    // The token, not a measurement: jsdom computes no layout, and the two
    // tokens the cluster uses (`default` and `icon`) both resolve to 32px.
    for (const el of [settings, reopen, macros, theater]) {
      expect(el.className).toContain("h-8")
    }
    expect(paneMenu.className).toContain("size-8")
    // …and no control in the row forces its own height on top of the token
    // for a mouse. A finger's taller size is a `pointer-coarse:` variant, which
    // this pattern deliberately does not match.
    for (const el of [settings, reopen, macros, theater, paneMenu]) {
      expect(el.className).not.toMatch(/(?:^|\s)h-(9|10|11)\b/)
      expect(el.className).not.toMatch(/(?:^|\s)size-(9|10|11)\b/)
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
    // No "same branch" caption text renders.
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
    expect(chipValue("terminal")).toBe("2 terminals")
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
  it("puts the pane's own controls before a spacer sized to the Changes panel", () => {
    mockState = { ...stateFor("main", "main"), changesPanePercent: 31 }
    render(<InsetHeader />)
    const spacer = screen.getByTestId("changes-pane-spacer")
    expect(spacer.style.width).toBe("31%")
    // Theater, Macros and the pane menu are the PANE's controls: they sit
    // ahead of the spacer, so they land on the terminal pane's right edge
    // rather than the window's.
    for (const name of [/theater/i, /run a macro/i, /session actions/i]) {
      const control = screen.getByRole("button", { name })
      expect(spacer.contains(control)).toBe(false)
      expect(
        control.compareDocumentPosition(spacer) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy()
    }
  })

  it("tracks a dragged divider, because the width is the live percentage", () => {
    mockState = { ...stateFor("main", "main"), changesPanePercent: 44.5 }
    render(<InsetHeader />)
    expect(screen.getByTestId("changes-pane-spacer").style.width).toBe("44.5%")
  })

  it("collapses the spacer to zero when the Changes pane is hidden", () => {
    // The button slides right with the terminal pane that just grew under it.
    mockState = {
      ...stateFor("main", "main"),
      changesPanePercent: 26,
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.getByTestId("changes-pane-spacer").style.width).toBe("0%")
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
    const cluster = screen.getByTestId("changes-pane-spacer")
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
    // If every field were gated on a resolved SESSION, a focused project
    // terminal would render a completely blank bar.
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
  it("names the agent's row while a terminal of its dock has focus", () => {
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
    // The header is the row's, so it reads exactly as it does with the agent
    // focused: the agent first among equals, and its terminal count.
    expect(chipKindsInOrder()).toEqual([
      "project",
      "agent",
      "terminal",
      "assistant",
    ])
    expect(chipValue("terminal")).toBe("1 terminal")
    expect(chip("agent")!.className).not.toContain("shrink-[9999]")
  })
})

describe("InsetHeader assistant chip over a dock terminal", () => {
  it("names the provider of the tab the stack keeps on top after a reload", () => {
    const base = stateFor("main", "main")
    const session = base.spine!.sessions[0]
    session.tabs = [
      ...session.tabs,
      { ...session.tabs[0], id: "s1-t2", provider: "codex", order: 1 },
    ]
    mockState = {
      ...base,
      selectedTarget: { kind: "terminal", terminalId: "t1", owner: { kind: "session", sessionId: "s1" } },
      spine: {
        ...base.spine,
        terminals: [{ id: "t1", owner: { kind: "session", session_id: "s1" }, label: "Terminal 1" }],
      },
    } as unknown as DuxState
    // This page has not shown a tab yet; the device remembers the extra one.
    localStorage.setItem("dux:stack:s1", JSON.stringify({ tab: "s1-t2" }))
    render(<InsetHeader />)
    expect(chipValue("assistant")).toBe("codex")
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

// WHICH MENU THE PANE'S `⋯` OPENS is the same question the chips answer: what
// is this header about? An agent behind the pane makes it the agent's, and a
// terminal with no agent makes it the terminal's. The alternative, reading
// `selectedSessionId`, still names the agent a project terminal was reached
// from, and would have put an agent's Delete under a project terminal.
describe("InsetHeader pane menu", () => {
  it("opens the agent's menu for an agent pane", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.getByRole("button", { name: /session actions/i })).toBeTruthy()
    expect(screen.queryByRole("button", { name: /terminal actions/i })).toBeNull()
  })

  it("opens the agent's menu for that agent's own terminal, with the terminal's own verbs in it", async () => {
    // The whole header is the agent's here (its project, its name, its
    // assistant), so the menu is too. The terminal's own Close and editor
    // entries ride along as a labelled group, and the INPUT group is read
    // under the TERMINAL's pty id, which is what that pane publishes under:
    // reading it off the agent instead left this pane with no attach item and
    // no way back from typing straight into the terminal.
    const base = stateFor("main", "main")
    mockState = {
      ...base,
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
      createTabInFlight: [],
    } as unknown as DuxState
    registerPaneInputGroup("t1", { surfaceSwitch: true, keysToggle: false })
    render(<InsetHeader />)
    const trigger = screen.getByRole("button", { name: /session actions/i })
    fireEvent.click(trigger)
    await screen.findByRole("menu")
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent)
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Close…"))).toBe(true)
    expect(items.some((t) => t?.includes("Use virtual input"))).toBe(true)
  })

  it("opens the terminal's own menu for a project terminal, even with an agent still selected", () => {
    const base = stateFor("main", "main")
    mockState = {
      ...base,
      selectedTarget: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
      spine: {
        ...base.spine,
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
    expect(screen.getByRole("button", { name: /terminal actions/i })).toBeTruthy()
    expect(screen.queryByRole("button", { name: /session actions/i })).toBeNull()
  })

  it("renders no pane menu with nothing focused", () => {
    mockState = {
      selectedSessionId: null,
      selectedTarget: null,
      changesPanePercent: 26,
      spine: { projects: [], sessions: [] },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.queryByRole("button", { name: /session actions/i })).toBeNull()
    expect(screen.queryByRole("button", { name: /terminal actions/i })).toBeNull()
  })
})

// A device that does not drive the agent's row is not told it has terminals,
// and while another device drives it, cannot add to it either. Typing goes to
// the agent's terminal here, which is what the header reads the pane under.
describe("InsetHeader under the agent's take-over card", () => {
  function watching(ownerPresent: boolean): void {
    const base = stateFor("main", "main")
    mockState = {
      ...base,
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
    registerPaneCover("t1", "card", {
      row: agentRowKey("s1"),
      driver: ownerPresent ? "other" : "nobody",
    })
    render(<InsetHeader />)
  }
  async function menuItems(): Promise<HTMLElement[]> {
    fireEvent.click(screen.getByRole("button", { name: /session actions/i }))
    await screen.findByRole("menu")
    return screen.getAllByRole("menuitem")
  }
  const item = (items: HTMLElement[], text: string) =>
    items.find((el) => el.textContent?.includes(text))

  it("names no terminal and adds nothing while another device drives it", async () => {
    watching(true)
    expect(chip("terminal")).toBeNull()
    expect(screen.getByTestId("new-tab-disabled")).toBeTruthy()
    const items = await menuItems()
    expect(item(items, "Close…")).toBeUndefined()
    expect(item(items, "New terminal")!.getAttribute("aria-disabled")).toBe("true")
  })

  it("names no terminal and adds nothing while nobody drives it", async () => {
    watching(false)
    expect(chip("terminal")).toBeNull()
    expect(screen.getByTestId("new-tab-disabled")).toBeTruthy()
    const items = await menuItems()
    expect(item(items, "Close…")).toBeUndefined()
    expect(item(items, "New terminal")!.getAttribute("aria-disabled")).toBe("true")
    expect(screen.getByText(/Nobody is driving this agent/)).toBeTruthy()
  })

  it("closes New tab…'s dialog for good when another device takes the agent while it is open", async () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    fireEvent.click(screen.getByRole("button", { name: /new tab/i }))
    expect(await screen.findByRole("dialog")).toBeTruthy()
    let retire = () => {}
    act(() => {
      retire = registerPaneCover("s1", "card", { row: agentRowKey("s1"), driver: "other" })
    })
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull())
    act(() => retire())
    expect(screen.getByRole("button", { name: /new tab/i })).toBeTruthy()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(screen.queryByRole("dialog")).toBeNull()
  })
})

describe("InsetHeader show-Changes button", () => {
  // A reopen control inside the pane's own header menu unmounts with the
  // pane; this button is the always-there
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

// The reopen control carries the phone's ±N summary, from the one shared
// helper, so the two surfaces cannot say different things about the same
// worktree while the list itself is off screen.
describe("InsetHeader show-Changes summary", () => {
  function hiddenWith(changes: unknown): DuxState {
    return {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
      changes,
    } as unknown as DuxState
  }

  const changedFile = (path: string, status: string) => ({
    path,
    status,
    additions: 0,
    deletions: 0,
    binary: false,
  })

  it("puts the changed-file count on the button while the pane is hidden", () => {
    mockState = hiddenWith({
      sessionId: "s1",
      phase: "loaded",
      rev: 1,
      staged: [changedFile("a.rs", "M")],
      unstaged: [changedFile("b.rs", "??"), changedFile("c.rs", "D")],
    })
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    expect(button.textContent).toBe("±3")
    // The icon stays: the count is what the pane holds, the glyph is what the
    // press does.
    expect(button.querySelector("svg")).toBeTruthy()
    expect(button.getAttribute("aria-label")).toBe(
      "Show Changes pane, 3 changed files",
    )
  })

  it("shows the ±0 zero state, exactly as the phone does", () => {
    mockState = hiddenWith({
      sessionId: "s1",
      phase: "loaded",
      rev: 1,
      staged: [],
      unstaged: [],
    })
    render(<InsetHeader />)
    expect(
      screen.getByRole("button", { name: /show changes pane/i }).textContent,
    ).toBe("±0")
  })

  it("keeps the cluster's one height token when it carries the number", () => {
    mockState = hiddenWith({
      sessionId: "s1",
      phase: "loaded",
      rev: 1,
      staged: [changedFile("a.rs", "M")],
      unstaged: [],
    })
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    const settings = screen.getByRole("button", { name: /^settings$/i })
    expect(button.className).toContain("h-8")
    expect(settings.className).toContain("h-8")
    // The number widens the control; it must never make it taller than its
    // row, and it must not go back to being a fixed square.
    expect(button.className).not.toMatch(/(?:^|\s)size-\d/)
    expect(button.className).not.toMatch(/(?:^|\s)h-(9|10|11)\b/)
  })

  it("stays a bare icon when no agent is in view", () => {
    // A focused project terminal has no session, so there is no worktree the
    // count would be about. The phone draws no ±N control there either.
    mockState = {
      selectedSessionId: null,
      selectedTarget: null,
      changesPanePercent: 26,
      bootstrap: { show_changes_pane: false },
      spine: { projects: [], sessions: [] },
    } as unknown as DuxState
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    expect(button.textContent).toBe("")
    expect(button.className).toContain("size-8")
    expect(button.className).toContain("pointer-coarse:size-10")
  })

  it("carries no summary while the pane itself is on screen", () => {
    // The pane lists the files; nothing in the header repeats the count.
    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: true },
      changes: {
        sessionId: "s1",
        phase: "loaded",
        rev: 1,
        staged: [changedFile("a.rs", "M")],
        unstaged: [],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(
      screen.queryByRole("button", { name: /show changes pane/i }),
    ).toBeNull()
    expect(document.body.textContent).not.toContain("±1")
  })
})

describe("InsetHeader geometry", () => {
  it("stays exactly 48px tall and never lets the chips push the controls off", () => {
    mockState = stateFor("main", "main")
    const { container } = render(<InsetHeader />)
    const header = container.querySelector("header")!
    expect(header.className).toContain("h-12")
    expect(header.className).toContain("shrink-0")
    // The chip row owns the whole shrink budget.
    const row = chip("agent")!.parentElement!
    expect(row.className).toContain("min-w-0")
    expect(row.className).toContain("flex-1")
    expect(row.className).toContain("overflow-hidden")
    const controls = screen.getByRole("button", { name: /^settings$/i })
      .parentElement!
    expect(controls.className).toContain("shrink-0")
  })
})

describe("InsetHeader new tab button", () => {
  const newTab = () => screen.queryByRole("button", { name: "New tab…" })

  it("leads the pane's cluster on an agent pane: New tab…, Theater, Macros, ⋯", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    const order = [
      newTab()!,
      screen.getByRole("button", { name: /theater/i }),
      screen.getByRole("button", { name: /run a macro/i }),
      screen.getByRole("button", { name: /session actions/i }),
    ]
    for (let i = 1; i < order.length; i++) {
      expect(
        order[i - 1].compareDocumentPosition(order[i]) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy()
    }
    // One outline family at one height token with Theater and Macros.
    for (const control of order.slice(0, 3)) {
      expect(control.className).toContain("h-8")
      expect(control.className).toContain("border-border")
    }
  })

  it("opens the agent's add dialog, which adds a tab or a terminal", async () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    fireEvent.click(newTab()!)
    const dialog = await screen.findByRole("dialog")
    // The provider is named as a chip, never in quotes.
    expect(dialog.querySelector("code")?.textContent).toBe("claude")
    fireEvent.click(screen.getByText("New agent tab"))
    expect(store.addTab).toHaveBeenCalledWith("s1")

    fireEvent.click(newTab()!)
    fireEvent.click(await screen.findByText("New terminal"))
    expect(store.createTerminal).toHaveBeenCalledWith("s1")
  })

  it("stays on the agent's pane while a terminal of its dock has focus", () => {
    const base = stateFor("main", "main")
    mockState = {
      ...base,
      selectedTarget: { kind: "terminal", terminalId: "t1", owner: { kind: "session", sessionId: "s1" } },
      spine: {
        ...base.spine,
        terminals: [{ id: "t1", owner: { kind: "session", session_id: "s1" }, label: "Terminal 1" }],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(newTab()).not.toBeNull()
  })

  it("is absent on a project terminal's pane, which has no agent to add a tab to", () => {
    mockState = {
      selectedSessionId: null,
      changesPanePercent: 26,
      selectedTarget: { kind: "terminal", terminalId: "pt-1", owner: { kind: "project", projectId: "p1" } },
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        sessions: [],
        terminals: [{ id: "pt-1", owner: { kind: "project", project_id: "p1" }, label: "Terminal 2" }],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.getByRole("button", { name: /theater/i })).toBeTruthy()
    expect(newTab()).toBeNull()
  })
})

// THE NAMES OUTRANK THE BUTTONS. While a chip's own text is cut off, New tab…,
// Theater and Macros… leave the header together for rows in the `⋯`, and they
// come back once the names fit beside them. jsdom computes no layout, so each
// test states the widths it needs: an element reporting more scroll width than
// client width is a cut-off name.
describe("InsetHeader folding the cluster into the pane menu", () => {
  const CLUSTER = ["New tab…", "Theater", "Macros…"]

  function cutOffNames() {
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(400)
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(40)
  }

  function clusterButtons(): (HTMLElement | null)[] {
    return [
      screen.queryByRole("button", { name: "New tab…" }),
      screen.queryByRole("button", { name: /theater mode/i }),
      screen.queryByRole("button", { name: /run a macro/i }),
    ]
  }

  async function menuLabels(name: RegExp): Promise<string[]> {
    fireEvent.click(screen.getByRole("button", { name }))
    await screen.findByRole("menu")
    return screen.getAllByRole("menuitem").map((el) => el.textContent ?? "")
  }

  it("keeps the cluster in the header, and out of the menu, while the names fit", async () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    for (const button of clusterButtons()) expect(button).not.toBeNull()
    const items = await menuLabels(/session actions/i)
    for (const label of CLUSTER) expect(items).not.toContain(label)
  })

  it("moves all three into the menu, under a heading, when a name is cut off", async () => {
    cutOffNames()
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    for (const button of clusterButtons()) expect(button).toBeNull()
    const items = await menuLabels(/session actions/i)
    for (const label of CLUSTER) expect(items).toContain(label)
    expect(screen.getByText("Pane")).toBeTruthy()
  })

  // In theater a full-pane cover brings the header back; the menu then offers
  // the way out once, under the theater group's own words.
  it("offers leaving theater once, and no Theater row, while theater is on", async () => {
    cutOffNames()
    mockState = { ...stateFor("main", "main"), theater: true } as unknown as DuxState
    render(<InsetHeader />)
    const items = await menuLabels(/session actions/i)
    expect(items.filter((label) => /theater/i.test(label))).toEqual([
      "Leave theater mode",
    ])
    expect(items).toContain("Macros…")
  })

  it("moves Theater and Macros… on a project terminal's pane, which has no New tab…", async () => {
    cutOffNames()
    mockState = {
      selectedSessionId: null,
      changesPanePercent: 26,
      selectedTarget: { kind: "terminal", terminalId: "pt-1", owner: { kind: "project", projectId: "p1" } },
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        sessions: [],
        terminals: [{ id: "pt-1", owner: { kind: "project", project_id: "p1" }, label: "Terminal 2" }],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    const items = await menuLabels(/terminal actions/i)
    expect(items).toContain("Theater")
    expect(items).toContain("Macros…")
    expect(items).not.toContain("New tab…")
  })

  it("runs from each row exactly what its button runs", async () => {
    cutOffNames()
    mockState = stateFor("main", "main")
    render(<InsetHeader />)

    await menuLabels(/session actions/i)
    fireEvent.click(screen.getByRole("menuitem", { name: "Theater" }))
    expect(store.toggleTheater).toHaveBeenCalled()

    await menuLabels(/session actions/i)
    fireEvent.click(screen.getByRole("menuitem", { name: "New tab…" }))
    fireEvent.click(await screen.findByText("New agent tab"))
    expect(store.addTab).toHaveBeenCalledWith("s1")
    // With no button left to return to, focus comes back to the `⋯`.
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: /session actions/i }),
      ),
    )

    await menuLabels(/session actions/i)
    fireEvent.click(screen.getByRole("menuitem", { name: "Macros…" }))
    expect(await screen.findByText(/no macros found/i)).toBeTruthy()
  })

  it("brings the cluster back when the names fit beside it again", () => {
    // The header follows its row's size, which is what a dragged divider or a
    // side panel opening and closing changes.
    const observers: (() => void)[] = []
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          observers.push(cb)
        }
        observe() {}
        disconnect() {}
      },
    )
    cutOffNames()
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    for (const button of clusterButtons()) expect(button).toBeNull()

    // Room enough: nothing is cut off and the row is wide.
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(500)
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(500)
    act(() => observers.forEach((cb) => cb()))
    for (const button of clusterButtons()) expect(button).not.toBeNull()
  })

  it("hands focus to the ⋯ when the control holding it folds away", () => {
    const observers: (() => void)[] = []
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          observers.push(cb)
        }
        observe() {}
        disconnect() {}
      },
    )
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    screen.getByRole("button", { name: /theater mode/i }).focus()

    cutOffNames()
    act(() => observers.forEach((cb) => cb()))
    for (const button of clusterButtons()) expect(button).toBeNull()
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: /session actions/i }),
    )
  })

  it("gives every control left in the header the 40px touch height under a finger, and 32px with a mouse", () => {
    for (const state of ["fits", "folded"] as const) {
      if (state === "folded") cutOffNames()
      mockState = {
        ...stateFor("main", "main"),
        bootstrap: { show_changes_pane: true },
        changesPanePercent: 0,
      } as unknown as DuxState
      render(<InsetHeader />)
      const paneMenu = screen.getByRole("button", { name: /session actions/i })
      expect(paneMenu.className).toContain("size-8")
      expect(paneMenu.className).toContain("pointer-coarse:size-10")
      const reopen = screen.getByRole("button", { name: /show changes pane/i })
      expect(reopen.className).toContain("h-8")
      expect(reopen.className).toContain("pointer-coarse:h-10")
      const cog = screen.getByRole("button", { name: "Settings" })
      expect(cog.className).toContain("h-8")
      expect(cog.className).toContain("pointer-coarse:h-10")
      for (const button of clusterButtons()) {
        if (state === "folded") continue
        expect(button!.className).toContain("h-8")
        expect(button!.className).toContain("pointer-coarse:h-10")
      }
      cleanup()
    }
  })
})
