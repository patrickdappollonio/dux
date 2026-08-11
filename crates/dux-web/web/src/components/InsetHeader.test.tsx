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

function stateFor(branchName: string, initialBranch: string): DuxState {
  return {
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
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
})

describe("InsetHeader app menu", () => {
  it("renders the app-menu cog instead of a Commands button", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.queryByText(/Commands/)).toBeNull()
    expect(screen.getByRole("button", { name: /^menu$/i })).toBeTruthy()
  })
})

describe("InsetHeader project terminal crumbs", () => {
  it("renders project and terminal crumbs for a focused project terminal", () => {
    // The trap this guards (T8): every crumb was gated on a resolved SESSION,
    // so a focused project terminal rendered a completely blank breadcrumb bar.
    mockState = {
      selectedSessionId: null,
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
    // The TERMINAL is the subject; its owning project is the caption. No labels
    // on either half.
    expect(screen.getByText("Terminal 2")).toBeTruthy()
    expect(screen.getByText("Repo · 1 terminal")).toBeTruthy()
    expect(screen.queryByText(/project:/)).toBeNull()
    expect(screen.queryByText(/terminal:/)).toBeNull()
  })
})

describe("InsetHeader standalone terminal crumbs", () => {
  it("names the directory for a focused standalone terminal", () => {
    // It has no owner to name, so a breadcrumb that only knows how to name
    // owners would render blank. The directory is what it says instead, and it
    // is the same string its sidebar row shows.
    mockState = {
      selectedSessionId: null,
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
    expect(screen.getByText("Terminal 1")).toBeTruthy()
    expect(screen.getByText("~/code · 1 terminal")).toBeTruthy()
    expect(screen.queryByText(/directory:/)).toBeNull()
  })
})

describe("InsetHeader show-Changes button", () => {
  // The pane's only in-app reopen control used to live inside the pane's own
  // header menu, which unmounts with the pane; this button is the always-there
  // way back (the sidebar rail-button pattern applied to the right panel).
  it("renders only while the Changes pane is hidden, and clicking it shows the pane", async () => {
    const store = await import("@/lib/store")
    const setVisibility = vi.mocked(store.setChangesPaneVisibility)
    setVisibility.mockClear()

    mockState = {
      ...stateFor("main", "main"),
      bootstrap: { show_changes_pane: false },
    } as unknown as DuxState
    render(<InsetHeader />)
    const button = screen.getByRole("button", { name: /show changes pane/i })
    fireEvent.click(button)
    expect(setVisibility).toHaveBeenCalledWith(true)
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

describe("InsetHeader branch drift cue", () => {
  it("shows the original branch only when the current branch differs", () => {
    mockState = stateFor("agent-tabs", "server-mode")
    render(<InsetHeader />)
    expect(screen.getByText(/originally server-mode/)).toBeTruthy()
  })

  it("omits the original branch when it matches the current branch", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.queryByText(/originally/)).toBeNull()
  })
})

describe("InsetHeader one-subject layout", () => {
  it("names the agent once, unlabelled, and says `same branch` instead of repeating it", () => {
    // The measured problem: an untitled agent takes its name from its branch, so
    // the old four-pair bar printed that word twice ("agent: main … branch:
    // main").
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.getByText("main")).toBeTruthy()
    expect(screen.getByText("Repo · claude · same branch")).toBeTruthy()
    // No labels survive anywhere in the strip.
    expect(screen.queryByText(/agent:/)).toBeNull()
    expect(screen.queryByText(/provider:/)).toBeNull()
    expect(screen.queryByText(/branch:/)).toBeNull()
  })

  it("prints the branch in the caption when it differs from the agent name", () => {
    // An untitled agent takes its name FROM the branch, so a differing branch
    // needs a titled agent.
    const titled = stateFor("feature-x", "")
    titled.spine!.sessions[0].title = "Tab redesign"
    mockState = titled
    render(<InsetHeader />)
    expect(screen.getByText("Tab redesign")).toBeTruthy()
    expect(screen.getByText("Repo · claude · feature-x")).toBeTruthy()
    expect(screen.queryByText(/same branch/)).toBeNull()
  })

  it("lets the caption give way before the agent name does", () => {
    // Both halves can truncate, but the caption's shrink factor is thousands of
    // times the subject's, so overflow is absorbed by the caption first. Asserted
    // as the classes that carry it: a layout rule no CSS reads is a rule that
    // silently stops working.
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    const subject = screen.getByText("main")
    const caption = screen.getByText("Repo · claude · same branch")
    expect(subject.className).toContain("truncate")
    expect(subject.className).toContain("min-w-0")
    expect(subject.className).toContain("shrink")
    expect(subject.className).not.toContain("shrink-0")
    expect(caption.className).toContain("truncate")
    expect(caption.className).toContain("shrink-[9999]")
    // Size and weight carry the hierarchy; one font (sans) throughout.
    expect(subject.className).toContain("text-sm")
    expect(caption.className).toContain("text-xs")
    expect(subject.className).not.toContain("font-mono")
    expect(caption.className).not.toContain("font-mono")
  })
})
