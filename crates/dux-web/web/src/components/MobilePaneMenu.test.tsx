// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import { stubCoarsePointer, type MatchMediaStub } from "@/test/matchMedia"

// THE PHONE'S ONE PANE MENU. What is pinned here is that it is ONE menu: the
// docked flap and the floating pill open the same body under the same name, so
// theater cannot be the state in which the agent's own actions disappear, and
// the cluster's `⋯` cannot mean something else once it has flown across the
// screen.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

function installStubs() {
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
installStubs()
const { MobileActionFlap } = await import("./MobileActionFlap")
const { TheaterPill } = await import("./TheaterPill")
const { MOBILE_PANE_MENU_LABEL } = await import("./MobilePaneMenu")

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    title: "s1",
    status: "active",
    auto_reopen_enabled: false,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [{ id: "s1", provider: "claude", order: 0 }],
  } as unknown as SessionView
}

const target = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }

function makeState(theater: boolean): DuxState {
  return {
    spine: { projects: [{ id: "p1", name: "dux" }], sessions: [session()] },
    bootstrap: {
      title: "dux",
      agent_tabs_max: 4,
      available_providers: ["claude"],
      gh_available: false,
    },
    theater,
    createTabInFlight: [],
    changes: {
      sessionId: "s1",
      phase: "loaded",
      staged: [],
      unstaged: [{ path: "a" }],
    },
  } as unknown as DuxState
}

let media: MatchMediaStub | null = null
const desktopWidth = window.innerWidth

beforeEach(() => {
  installStubs()
  mockState = makeState(false)
})

afterEach(() => {
  cleanup()
  media?.restore()
  media = null
  Object.defineProperty(window, "innerWidth", {
    value: desktopWidth,
    configurable: true,
  })
  vi.unstubAllGlobals()
})

async function openFrom(el: Element) {
  fireEvent.click(el)
  await screen.findByRole("menu")
}

function labels() {
  return screen.getAllByRole("menuitem").map((item) => item.textContent)
}

describe("the phone's one pane menu", () => {
  it("carries the agent's own actions from the docked flap", async () => {
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    await openFrom(screen.getByLabelText(MOBILE_PANE_MENU_LABEL))
    const items = labels()
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    expect(items.some((t) => t?.startsWith("Changes"))).toBe(true)
    // The way to the app's own actions, named for the control it stands in for.
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("carries the same body from the floating pill, under the same name", async () => {
    mockState = makeState(true)
    render(
      <TheaterPill
        target={target}
        session={session()}
        variant="mobile"
        flight="floating"
      />,
    )
    await openFrom(screen.getByLabelText(MOBILE_PANE_MENU_LABEL))
    const items = labels()
    // The whole point of the merge: theater was the one state in which every
    // per-agent action was unreachable on a phone.
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    expect(items.some((t) => t?.startsWith("Changes"))).toBe(true)
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("offers the way out of theater only while the mode is on", async () => {
    mockState = makeState(true)
    render(
      <TheaterPill
        target={target}
        session={session()}
        variant="mobile"
        flight="floating"
      />,
    )
    await openFrom(screen.getByLabelText(MOBILE_PANE_MENU_LABEL))
    expect(labels().some((t) => t?.includes("Leave theater mode"))).toBe(true)
    // And never the top-bar toggle, which offers to show a bar the mode has
    // already taken away.
    expect(labels().some((t) => t?.includes("top bar"))).toBe(false)

    cleanup()
    mockState = makeState(false)
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    await openFrom(screen.getByLabelText(MOBILE_PANE_MENU_LABEL))
    expect(labels().some((t) => t?.includes("Leave theater mode"))).toBe(false)
  })

  it("keeps the phone's chrome toggles where pressing them does something", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    media = stubCoarsePointer()
    render(<MobileActionFlap target={target} session={session()} band="strip" />)
    await openFrom(screen.getByLabelText(MOBILE_PANE_MENU_LABEL))
    const items = labels()
    expect(items.some((t) => t?.includes("terminal keys"))).toBe(true)
    // And nothing for the top bar: theater mode is the one way to hide the
    // phone's chrome, and a second flow for the same intent is exactly what
    // this menu must not grow back.
    expect(items.some((t) => t?.includes("top bar"))).toBe(false)
  })
})
