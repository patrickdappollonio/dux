// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

// The agent ⋯ menu's pull-request entries. What is pinned here is the GATING:
// the attach item exists only with a usable gh (matching the project menu's
// from-PR gate) and its label flips on the OVERRIDE (a manually attached PR),
// not on mere PR presence; the detach item exists whenever ANY pull request is
// associated, pinned or autodetected; and the resume item exists only while
// the agent is detached. Neither detach nor resume needs gh, since both talk
// to dux's own state.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openAttachPullRequest: vi.fn(),
    detachPullRequest: vi.fn(),
    resumePullRequestAutodetection: vi.fn(),
  }
})

// The store touches localStorage at import time (pulled in transitively), so
// stub the browser globals BEFORE the module graph evaluates.
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
  // AgentActionsMenu calls useIsMobile() (the bar quick toggles are
  // mobile-scoped), whose subscription needs matchMedia; jsdom has none.
  // Same inert stub TerminalPane.test.tsx installs. matches:false = desktop,
  // which is what these desktop-menu tests want.
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
const { AgentActionsMenu } = await import("./FlatAgentList")
const { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } = await import(
  "@/components/ui/dropdown-menu"
)
const store = await import("@/lib/store")
const openAttachPullRequest = vi.mocked(store.openAttachPullRequest)
const detachPullRequest = vi.mocked(store.detachPullRequest)
const resumePullRequestAutodetection = vi.mocked(
  store.resumePullRequestAutodetection,
)

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: over.id,
      initial_branch: over.id,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${over.id}`,
    },
    title: over.id,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function seed(session: SessionView, ghAvailable: boolean) {
  mockState = {
    bootstrap: {
      gh_available: ghAvailable,
      available_providers: ["claude"],
      agent_tabs_max: 20,
    },
    spine: { projects: [], sessions: [session] },
    createTabInFlight: [],
  } as unknown as DuxState
}

function openMenu(session: SessionView) {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <AgentActionsMenu session={session} />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

const prAuto = {
  number: 12,
  state: "open" as const,
  title: "Add a thing",
  url: "https://github.com/o/r/pull/12",
  overridden: false,
}
const prPinned = { ...prAuto, overridden: true }

beforeEach(() => {
  installStubs()
  openAttachPullRequest.mockClear()
  detachPullRequest.mockClear()
  resumePullRequestAutodetection.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AgentActionsMenu while the agent is active on another device", () => {
  // Ownership reaches the menu two ways: a MOUNTED TerminalPane's live
  // verdict in the `ptyOwnership` ledger, and the server-published
  // `input_owner` field on the spine's tabs (compared against this client's
  // own PTY-socket ids), which needs NO pane mounted at all. While any of the
  // agent's tab PTYs is input-owned by another connection, the entries that
  // MUTATE the agent disable; read-only entries (info, project submenu,
  // editor, terminals) stay usable.
  const disabledLabels = [
    /^New agent tab for /,
    "Force recreate agent…",
    "Enable agent auto-reopen",
    "Rename agent…",
    "Change agent provider…",
    "Change attached pull request…",
    "Detach pull request",
    "Delete agent…",
  ]
  const enabledLabels = [
    "Project…",
    "Fork agent…",
    "Agent info…",
    "Configure startup command…",
    "Configure environment variables…",
    "Rerun startup command",
    "Startup command logs…",
    "Open editor in new tab",
    "New terminal",
    "Copy local path",
  ]

  function itemFor(label: string | RegExp): Element {
    const el = screen.getByText(label).closest('[role="menuitem"]')
    if (!el) throw new Error(`no menuitem for ${label}`)
    return el
  }

  it("disables the mutating entries and keeps the read-only ones enabled", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, true)
    ;(mockState as unknown as { ptyOwnership: Record<string, string> }).ptyOwnership =
      { s1: "elsewhere" }
    await openMenu(session)
    for (const label of disabledLabels) {
      expect(
        itemFor(label).getAttribute("aria-disabled"),
        `expected disabled: ${label}`,
      ).toBe("true")
    }
    for (const label of enabledLabels) {
      expect(
        itemFor(label).getAttribute("aria-disabled"),
        `expected enabled: ${label}`,
      ).not.toBe("true")
    }
    // The reason is stated inline (disabled items are pointer-events-none, so
    // a hover tooltip could never fire, and touch has no hover at all).
    expect(
      screen.getByText(/active on another device/i),
    ).toBeTruthy()
  })

  it("keeps every entry enabled and shows no hint when nothing is owned elsewhere", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, true)
    ;(mockState as unknown as { ptyOwnership: Record<string, string> }).ptyOwnership =
      {}
    await openMenu(session)
    for (const label of [...disabledLabels, ...enabledLabels]) {
      expect(
        itemFor(label).getAttribute("aria-disabled"),
        `expected enabled: ${label}`,
      ).not.toBe("true")
    }
    expect(screen.queryByText(/active on another device/i)).toBeNull()
  })

  it("disables from the server-published input_owner alone, with no pane mounted", async () => {
    // The hub/sidebar row case: this client never attached to the agent, so
    // the ledger is empty and there are no own connection ids; the spine's
    // `input_owner` is the only signal, and it must be enough.
    const session = makeSession({
      id: "s1",
      pr: prPinned,
      tabs: [
        { id: "s1", provider: "claude", order: 0, input_owner: "42" },
      ] as unknown as SessionView["tabs"],
    })
    seed(session, true)
    await openMenu(session)
    expect(itemFor("Delete agent…").getAttribute("aria-disabled")).toBe("true")
    expect(itemFor("Rename agent…").getAttribute("aria-disabled")).toBe("true")
    expect(screen.getByText(/active on another device/i)).toBeTruthy()
  })

  it("stays enabled when the published owner is one of this client's own connections", async () => {
    const session = makeSession({
      id: "s1",
      pr: prPinned,
      tabs: [
        { id: "s1", provider: "claude", order: 0, input_owner: "42" },
      ] as unknown as SessionView["tabs"],
    })
    seed(session, true)
    ;(mockState as unknown as { ownPtyConnIds: Record<string, true> }).ownPtyConnIds =
      { "42": true }
    await openMenu(session)
    expect(itemFor("Delete agent…").getAttribute("aria-disabled")).not.toBe(
      "true",
    )
    expect(screen.queryByText(/active on another device/i)).toBeNull()
  })

  it("reads ownership through the agent's EXTRA tab ids too, not just the session slot", async () => {
    const session = makeSession({
      id: "s1",
      pr: prPinned,
      tabs: [
        { id: "s1", provider: "claude", order: 0 },
        { id: "tab-2", provider: "claude", order: 1 },
      ] as unknown as SessionView["tabs"],
    })
    seed(session, true)
    ;(mockState as unknown as { ptyOwnership: Record<string, string> }).ptyOwnership =
      { "tab-2": "elsewhere" }
    await openMenu(session)
    expect(itemFor("Delete agent…").getAttribute("aria-disabled")).toBe("true")
  })
})

describe("AgentActionsMenu pull-request entries", () => {
  it("hides both entries without a usable gh and no override", async () => {
    seed(makeSession({ id: "s1" }), false)
    await openMenu(makeSession({ id: "s1" }))
    expect(screen.queryByText(/Attach pull request/)).toBeNull()
    expect(screen.queryByText(/attached pull request/)).toBeNull()
    expect(screen.queryByText("Detach pull request")).toBeNull()
  })

  it("offers the attach entry with gh, with icon and trailing ellipsis, routed to the store", async () => {
    const session = makeSession({ id: "s1" })
    seed(session, true)
    await openMenu(session)
    const item = screen
      .getByText("Attach pull request…")
      .closest('[role="menuitem"]')
    expect(item).toBeTruthy()
    // Trailing "…" marks an item that opens a dialog; a leading lucide icon is
    // required on every item in these menus.
    expect(item!.textContent?.endsWith("…")).toBe(true)
    expect(item!.querySelector("svg")).toBeTruthy()
    // No PR associated at all: nothing to detach.
    expect(screen.queryByText("Detach pull request")).toBeNull()
    fireEvent.click(screen.getByText("Attach pull request…"))
    expect(openAttachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("keeps the attach label on an AUTODETECTED PR (label flips on the override, not presence)", async () => {
    const session = makeSession({ id: "s1", pr: prAuto })
    seed(session, true)
    await openMenu(session)
    expect(screen.getByText("Attach pull request…")).toBeTruthy()
    expect(screen.queryByText("Change attached pull request…")).toBeNull()
  })

  it("offers detach on an AUTODETECTED PR, the case detaching exists for", async () => {
    const session = makeSession({ id: "s1", pr: prAuto })
    seed(session, true)
    await openMenu(session)
    fireEvent.click(screen.getByText("Detach pull request"))
    expect(detachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("offers the resume way back only while the agent is detached", async () => {
    const detached = makeSession({ id: "s1", pr_autodetect_suppressed: true })
    seed(detached, true)
    await openMenu(detached)
    const resume = screen
      .getByText("Resume PR autodetection")
      .closest('[role="menuitem"]')
    expect(resume).toBeTruthy()
    // Opens no dialog, so no trailing ellipsis; a leading icon is required.
    expect(resume!.textContent?.endsWith("…")).toBe(false)
    expect(resume!.querySelector("svg")).toBeTruthy()
    // Nothing is associated while detached, so there is nothing to detach.
    expect(screen.queryByText("Detach pull request")).toBeNull()
    fireEvent.click(screen.getByText("Resume PR autodetection"))
    expect(resumePullRequestAutodetection).toHaveBeenCalledWith("s1")
  })

  it("keeps resume available without gh (the suppression is dux's own state)", async () => {
    const detached = makeSession({ id: "s1", pr_autodetect_suppressed: true })
    seed(detached, false)
    await openMenu(detached)
    expect(screen.getByText("Resume PR autodetection")).toBeTruthy()
  })

  it("hides resume on an agent nobody detached", async () => {
    const session = makeSession({ id: "s1", pr: prAuto })
    seed(session, true)
    await openMenu(session)
    expect(screen.queryByText("Resume PR autodetection")).toBeNull()
  })

  it("flips to the change label and offers detach while an override is pinned", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, true)
    await openMenu(session)
    expect(screen.getByText("Change attached pull request…")).toBeTruthy()
    expect(screen.queryByText("Attach pull request…")).toBeNull()
    const detach = screen
      .getByText("Detach pull request")
      .closest('[role="menuitem"]')
    expect(detach).toBeTruthy()
    // Reversible action: icon yes, but NO trailing ellipsis and no confirm.
    expect(detach!.textContent?.endsWith("…")).toBe(false)
    expect(detach!.querySelector("svg")).toBeTruthy()
    fireEvent.click(screen.getByText("Detach pull request"))
    expect(detachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("keeps detach available without gh (detaching talks to nothing)", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, false)
    await openMenu(session)
    expect(screen.queryByText(/pull request…/)).toBeNull()
    expect(screen.getByText("Detach pull request")).toBeTruthy()
  })
})
