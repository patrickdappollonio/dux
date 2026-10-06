// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView } from "@/lib/types"

// Override `useDux` so the dialog reads our seeded spine + close target, and
// spy `closeCloseTab` so the vanished-target guard is observable; the other
// real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeCloseTab: vi.fn() }
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
const { ConfirmCloseTabDialog } = await import("./ConfirmCloseTabDialog")
const store = await import("@/lib/store")
const closeCloseTab = vi.mocked(store.closeCloseTab)

function tab(overrides: Partial<AgentTabView>): AgentTabView {
  return {
    id: "s1",
    provider: "claude",
    order: 0,
    working: false,
    has_output: false,
    has_live_process: true,
    ...overrides,
  }
}

// Seed the dialog to close `tabId` of a session whose tabs are `tabs`.
// `slotTabId` names the tab holding the session slot; left out, no tab does,
// which is the shape the extra-tab cases care about.
function seed(tabId: string, tabs: AgentTabView[], slotTabId?: string) {
  mockState = {
    closeTabTarget: { sessionId: "s1", tabId },
    spine: {
      sessions: [{ id: "s1", slot_tab_id: slotTabId, tabs }],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  closeCloseTab.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmCloseTabDialog", () => {
  it("warns the agent detaches when closing its last LIVE tab (a dormant sibling doesn't count)", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
    // The provider name is named in the body.
    expect(screen.getByText(/This ends the/).textContent).toContain(
      "This ends the claude session",
    )
  })

  // The way back from a closed tab is a NEW tab, and a new tab always starts
  // fresh, so the copy must point at the provider's own history command rather
  // than at a resume dux will not perform.
  it("says a new tab starts fresh and names the history command", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: true }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/deletes the tab for good/)).toBeTruthy()
    expect(screen.getByText(/A new tab always starts fresh/)).toBeTruthy()
    expect(screen.getByText(/history command/)).toBeTruthy()
  })

  it("shows no detach warning when a live sibling keeps the agent running", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: true }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(screen.queryByText(/the agent detaches/)).toBeNull()
  })

  // The closed tab itself is DORMANT (has_live_process: false, liveTabs counts 0
  // among OTHER tabs): the `liveTabs === 0` branch of `willDetach`. Closing an
  // already-dormant tab is still meaningful: it deletes the
  // dormant tab's row (or, for the session-slot tab, its slot) outright.
  it("shows no detach warning when closing an already-dormant tab that has a live sibling", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(screen.queryByText(/the agent detaches/)).toBeNull()
  })

  it("warns the agent detaches when closing an already-dormant tab with no live sibling", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: false }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
  })

  // Closing the tab in the session slot is not refused any more: the slot moves
  // to the next tab in strip order, and the copy has to say so and name it, or
  // the user cannot tell this close apart from an extra tab's.
  it("says which tab takes the slot when closing the agent's first tab", () => {
    seed(
      "t1",
      [
        tab({ id: "t1", provider: "claude", has_live_process: true }),
        tab({ id: "t2", provider: "codex", has_live_process: true }),
      ],
      "t1",
    )
    render(<ConfirmCloseTabDialog />)
    const body = screen.getByText(/This ends the/)
    expect(body.textContent).toContain("This ends the claude session")
    expect(screen.getByText("claude", { selector: "code" })).toBeTruthy()
    expect(body.textContent).toMatch(
      /The next tab, Codex, takes its place as the agent’s first tab/,
    )
    // The tab's name is the shared chip.
    expect(screen.getByText("Codex", { selector: "code" })).toBeTruthy()
  })

  // Two tabs on the same provider are told apart by the strip's own suffix, and
  // the sentence has to use it: "codex" names two pills, "Codex 2" names one.
  it("names the successor the way the strip labels it when providers repeat", () => {
    seed(
      "t1",
      [
        tab({ id: "t1", provider: "codex", has_live_process: true }),
        tab({ id: "t2", provider: "codex", has_live_process: true }),
      ],
      "t1",
    )
    render(<ConfirmCloseTabDialog />)
    const body = screen.getByText(/This ends the/)
    expect(body.textContent).toContain("This ends the codex session")
    expect(body.textContent).toMatch(
      /The next tab, Codex 2, takes its place as the agent’s first tab/,
    )
    expect(screen.getByText("Codex 2", { selector: "code" })).toBeTruthy()
  })

  it("says nothing about a successor when closing an extra tab", () => {
    seed(
      "t2",
      [
        tab({ id: "t1", provider: "claude", has_live_process: true }),
        tab({ id: "t2", provider: "codex", has_live_process: true }),
      ],
      "t1",
    )
    render(<ConfirmCloseTabDialog />)
    expect(screen.queryByText(/takes its place/)).toBeNull()
  })

  // Both facts at once: the successor is dormant, so the close takes the
  // agent's last live process AND hands the slot on.
  it("says both when the first tab's close detaches the agent onto a dormant successor", () => {
    seed(
      "t1",
      [
        tab({ id: "t1", provider: "claude", has_live_process: true }),
        tab({ id: "t2", provider: "codex", has_live_process: false }),
      ],
      "t1",
    )
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
    const body = screen.getByText(/This ends the/)
    expect(body.textContent).toContain("This ends the claude session")
    expect(screen.getByText("claude", { selector: "code" })).toBeTruthy()
    expect(body.textContent).toMatch(
      /The next tab, Codex, takes its place as the agent’s first tab/,
    )
    // The tab's name is the shared chip.
    expect(screen.getByText("Codex", { selector: "code" })).toBeTruthy()
  })

  // The vanished-target guard: the tab (or its session) disappearing from the
  // ViewModel while the dialog is open must close it instead of leaving a
  // stale confirm pointed at a gone target.
  it("closes itself when the target tab is gone from the ViewModel", () => {
    seed("gone-tab", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    expect(screen.queryByText("Close tab?")).toBeNull()
    expect(closeCloseTab).toHaveBeenCalled()
  })

  it("stays open (and does not self-close) while the target tab still exists", () => {
    seed("s1", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(closeCloseTab).not.toHaveBeenCalled()
  })

  // Through the real store and client: the server's refusal because somebody
  // else is attached keeps the dialog open naming them, focus goes back to
  // Cancel, and only Close tab anyway sends `force_connected=true`.
  it("names who is attached when the close is refused, and closes over them only through the override", async () => {
    seed("s1", [tab({ id: "s1", provider: "claude" })])
    Object.assign(mockState.spine!.sessions[0], { title: "fix-auth" })
    const answers = [
      {
        status: 409,
        body: {
          error: "attached",
          blockers: [
            {
              surface: "browser",
              device: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15",
              address: "100.64.0.2",
              verified: true,
              driving: true,
              target: { kind: "tab", id: "s1", agent: "s1" },
            },
          ],
        },
      },
      { status: 200, body: { detached: true } },
    ]
    const fetchMock = vi.fn(async () => {
      const answer = answers.shift()!
      return {
        ok: answer.status < 300,
        status: answer.status,
        text: async () => JSON.stringify(answer.body),
        headers: { get: () => null },
      }
    })
    vi.stubGlobal("fetch", fetchMock)
    render(<ConfirmCloseTabDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Close tab" }))
    const override = await screen.findByRole("button", { name: "Close tab anyway" })
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Cancel" }),
    )
    expect(screen.getByText("Safari on macOS").tagName).toBe("CODE")
    expect(screen.getByText(/at 100\.64\.0\.2, typing in tab/)).toBeTruthy()
    expect(closeCloseTab).not.toHaveBeenCalled()

    fireEvent.click(override)
    await vi.waitFor(() => expect(closeCloseTab).toHaveBeenCalled())
    const urls = fetchMock.mock.calls.map((call) => String(call[0]))
    expect(urls).toEqual([
      "/api/v1/sessions/s1/tabs/s1",
      "/api/v1/sessions/s1/tabs/s1?force_connected=true",
    ])
  })

  it("links to the docs, safely, in a new tab", () => {
    seed("s1", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    const link = screen.getByRole("link", { name: /how closing a tab works/i })
    expect(link.getAttribute("href")).toBe(
      "https://getdux.app/docs/agent-tabs#closing-a-tab-is-one-way",
    )
    expect(link.getAttribute("target")).toBe("_blank")
    // noopener/noreferrer: don't hand the docs tab a window.opener back into dux.
    expect(link.getAttribute("rel")).toContain("noopener")
    expect(link.getAttribute("rel")).toContain("noreferrer")
  })
})
