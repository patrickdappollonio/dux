// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

let mockState: DuxState
const exitTheaterMock = vi.fn()
const selectTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    exitTheater: (...a: unknown[]) => exitTheaterMock(...a),
    selectTab: (...a: unknown[]) => selectTabMock(...a),
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
const { TheaterPill } = await import("./TheaterPill")
const { peekTheaterTabs } = await import("@/lib/theater")

function tab(over: Partial<AgentTabView> & { id: string }): AgentTabView {
  return {
    provider: "claude",
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
    ...over,
  } as AgentTabView
}

function session(tabs: AgentTabView[]): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "",
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: "",
    },
    tabs,
  } as unknown as SessionView
}

beforeEach(() => {
  installBootStubs()
  exitTheaterMock.mockReset()
  selectTabMock.mockReset()
  mockState = { bootstrap: null } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const agentTarget = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }
const terminalTarget = {
  kind: "terminal" as const,
  terminalId: "tm1",
  owner: { kind: "standalone" as const },
}

describe("the floating theater pill", () => {
  it("always carries the way out", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    fireEvent.click(screen.getByRole("button", { name: "Leave theater mode" }))
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
  })

  it("carries a macros trigger beside it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(screen.getByRole("button", { name: "Run a macro" })).toBeTruthy()
  })

  it("collapses to macros and exit for a terminal, which has no tabs", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(screen.queryByRole("button", { name: /other tab/i })).toBeNull()
  })

  it("collapses for a single-tab agent, so the expander is never empty", () => {
    render(
      <TheaterPill target={agentTarget} session={session([tab({ id: "s1" })])} />,
    )
    expect(screen.queryByRole("button", { name: /other tab/i })).toBeNull()
  })

  it("offers the hidden tabs when there are some, and switches to one", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(status.getAttribute("aria-expanded")).toBe("false")
    fireEvent.click(status)
    expect(status.getAttribute("aria-expanded")).toBe("true")

    // The switch CARRIES the mode rather than reading the destination's memory,
    // so reaching for a sibling that has never been in theater stays in it.
    fireEvent.click(screen.getByRole("tab", { name: /codex/i }))
    expect(selectTabMock).toHaveBeenCalledWith("s1", "t2", { theater: true })
  })

  it("puts the folded-out strip away on a tap anywhere else", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    fireEvent.click(status)
    expect(status.getAttribute("aria-expanded")).toBe("true")
    fireEvent.pointerDown(document.body)
    expect(status.getAttribute("aria-expanded")).toBe("false")
  })

  it("keeps the strip open for a press inside the pill itself", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    fireEvent.click(status)
    fireEvent.pointerDown(screen.getByTestId("theater-pill"))
    expect(status.getAttribute("aria-expanded")).toBe("true")
  })

  it("publishes the strip so the page-wide Escape rule can collapse it", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(peekTheaterTabs()?.expanded()).toBe(false)
    fireEvent.click(status)
    expect(peekTheaterTabs()?.expanded()).toBe(true)
    act(() => peekTheaterTabs()?.collapse())
    expect(status.getAttribute("aria-expanded")).toBe("false")
  })

  it("takes focus onto the way out when the chrome left nothing focused", () => {
    // Entering from the header button destroys that button, so focus falls to
    // the body; the pill is the nearest thing to what the user was doing.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Leave theater mode" }),
    )
  })

  it("leaves focus alone when something else already holds it", () => {
    const field = document.createElement("textarea")
    document.body.appendChild(field)
    field.focus()
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(document.activeElement).toBe(field)
    field.remove()
  })

  it("marks the tab on screen as the selected one", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /other tabs/i }))
    expect(
      screen.getByRole("tab", { name: /claude/i }).getAttribute("aria-selected"),
    ).toBe("true")
  })

  it("shows the attention dot for a background tab that needs you", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1" }),
          tab({ id: "t2", provider: "codex", needs_attention: true }),
        ])}
      />,
    )
    expect(screen.getAllByLabelText("Needs attention").length).toBeGreaterThan(0)
  })

  it("says nothing about the tab already filling the screen", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1", needs_attention: true }),
          tab({ id: "t2", provider: "codex" }),
        ])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(status.getAttribute("aria-label")).not.toMatch(/attention/i)
  })
})
