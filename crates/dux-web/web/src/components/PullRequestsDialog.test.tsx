// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { PrView, SessionView } from "@/lib/types"
import { installBrowserHistory } from "@/test/browserHistory"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closePullRequests: vi.fn(),
    hidePullRequest: vi.fn(),
    showPullRequest: vi.fn(),
    detachPullRequest: vi.fn(),
    movePullRequest: vi.fn(),
    reorderPullRequests: vi.fn(() => Promise.resolve(true)),
    setPullRequestSort: vi.fn(),
    openAttachPullRequest: vi.fn(),
  }
})

vi.mock("@/lib/sessionsApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/sessionsApi")>()
  return {
    ...actual,
    sessionsApi: { ...actual.sessionsApi, pullRequestBranches: vi.fn() },
  }
})

// jsdom never runs a sort, so dnd-kit would hand back no transition at all; a
// fixed one stands in for the animation it gives a row that is moving.
vi.mock("@dnd-kit/sortable", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@dnd-kit/sortable")>()
  return {
    ...actual,
    useSortable: (args: Parameters<typeof actual.useSortable>[0]) => ({
      ...actual.useSortable(args),
      transition: "transform 250ms ease",
    }),
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
installBrowserHistory()
const { PullRequestsDialog } = await import("./PullRequestsDialog")
const store = await import("@/lib/store")
const api = await import("@/lib/sessionsApi")
const pullRequestBranches = vi.mocked(api.sessionsApi.pullRequestBranches)

function pr(overrides: Partial<PrView>): PrView {
  return {
    number: 418,
    state: "open",
    title: "Store every tab as a row with a stable id",
    url: "https://github.com/acme/api/pull/418",
    overridden: false,
    host: "github.com",
    owner_repo: "acme/api",
    own_repo: true,
    found: true,
    attached: false,
    head_branch: "store-tab-rows",
    ...overrides,
  }
}

const found = pr({})
const attached = pr({
  number: 77,
  title: "Allow the preview host",
  url: "https://github.com/acme/infra/pull/77",
  owner_repo: "acme/infra",
  own_repo: false,
  found: false,
  attached: true,
  head_branch: undefined,
})
const both = pr({
  number: 415,
  state: "merged",
  title: "Promote the next tab",
  url: "https://github.com/acme/api/pull/415",
  attached: true,
  head_branch: "tab-slot-pointer",
})
const hidden = pr({
  number: 402,
  state: "closed",
  title: "Key provider state by tab id",
  url: "https://github.com/acme/api/pull/402",
  head_branch: "tab-state-by-id",
})

function session(over: Partial<SessionView> = {}): SessionView {
  return {
    id: "s1",
    title: "store-tab-rows",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "store-tab-rows",
      initial_branch: "store-tab-rows",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/wt/s1",
    },
    pr: found,
    prs: [found, attached, both],
    prs_hidden: [hidden],
    pr_sort: "automatic",
    pr_revision: 1,
    ...over,
  } as unknown as SessionView
}

function seed(s: SessionView | null, ghAvailable = true) {
  mockState = {
    pullRequestsTarget: "s1",
    bootstrap: { gh_available: ghAvailable },
    spine: { sessions: s ? [s] : [] },
  } as unknown as DuxState
}

let media: MatchMediaStub
beforeEach(() => {
  installBootStubs()
  media = stubMatchMedia()
  pullRequestBranches.mockReset()
  pullRequestBranches.mockResolvedValue({
    branches: [
      {
        name: "store-tab-rows",
        former_names: [],
        current: true,
        first_seen_at: "2026-11-03T14:02:00Z",
        prs: [{ host: "github.com", owner_repo: "acme/api", number: 418 }],
      },
      {
        name: "tab-state-by-id",
        former_names: ["tab-state"],
        current: false,
        first_seen_at: "2026-10-24T16:15:00Z",
        prs: [{ host: "github.com", owner_repo: "acme/api", number: 402 }],
      },
      {
        name: "spike-tab-ids",
        former_names: [],
        current: false,
        first_seen_at: "2026-10-22T11:08:00Z",
        prs: [],
      },
    ],
    attached_on_no_branch: [{ host: "github.com", owner_repo: "acme/infra", number: 77 }],
  })
  vi.mocked(store.hidePullRequest).mockClear()
  vi.mocked(store.detachPullRequest).mockClear()
  vi.mocked(store.showPullRequest).mockClear()
  vi.mocked(store.movePullRequest).mockClear()
  vi.mocked(store.setPullRequestSort).mockClear()
  vi.mocked(store.openAttachPullRequest).mockClear()
})

afterEach(() => {
  cleanup()
  media.restore()
  vi.unstubAllGlobals()
})

async function openRowMenu(name: string) {
  await act(async () => {
    fireEvent.click(screen.getByLabelText(`Actions for ${name}`))
  })
  return screen.findByRole("menu")
}

function menuLabels(menu: HTMLElement): string[] {
  return [...menu.querySelectorAll('[role="menuitem"]')].map((el) => el.textContent ?? "")
}

describe("PullRequestsDialog rows", () => {
  it("offers Hide on a found pull request, and no move upward on the first row", async () => {
    seed(session())
    render(<PullRequestsDialog />)
    const menu = await openRowMenu("#418")
    expect(menuLabels(menu)).toEqual(["Open on GitHub", "Move down", "Hide"])
    fireEvent.click(screen.getByText("Hide"))
    expect(store.hidePullRequest).toHaveBeenCalledWith("s1", {
      host: "github.com",
      owner_repo: "acme/api",
      number: 418,
    })
  })

  it("offers Detach on an attached pull request, with no ellipsis and no confirm", async () => {
    seed(session())
    render(<PullRequestsDialog />)
    const menu = await openRowMenu("acme/infra#77")
    expect(menuLabels(menu)).toEqual([
      "Open on GitHub",
      "Move to top",
      "Move up",
      "Move down",
      "Detach",
    ])
    fireEvent.click(screen.getByText("Detach"))
    expect(store.detachPullRequest).toHaveBeenCalledWith("s1", {
      host: "github.com",
      owner_repo: "acme/infra",
      number: 77,
    })
  })

  it("offers Detach on one both found and attached, and no move downward on the last row", async () => {
    seed(session())
    render(<PullRequestsDialog />)
    const menu = await openRowMenu("#415")
    expect(menuLabels(menu)).toEqual(["Open on GitHub", "Move to top", "Move up", "Detach"])
    fireEvent.click(screen.getByText("Move to top"))
    expect(store.movePullRequest).toHaveBeenCalledWith(
      "s1",
      { host: "github.com", owner_repo: "acme/api", number: 415 },
      "top",
    )
  })

  it("keeps Hidden collapsed until opened, and updates a hidden row while open", async () => {
    seed(session())
    const { rerender } = render(<PullRequestsDialog />)
    expect(screen.queryByText("Key provider state by tab id")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /Hidden/ }))
    expect(screen.getByText("Key provider state by tab id")).toBeTruthy()
    expect(screen.getByText("closed")).toBeTruthy()

    seed(session({ prs_hidden: [{ ...hidden, state: "merged" }], pr_revision: 2 }))
    rerender(<PullRequestsDialog />)
    expect(screen.queryByText("closed")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Show again" }))
    expect(store.showPullRequest).toHaveBeenCalledWith("s1", {
      host: "github.com",
      owner_repo: "acme/api",
      number: 402,
    })
  })

  it("says how the order is kept, and Sort switches it", async () => {
    seed(session())
    const { rerender } = render(<PullRequestsDialog />)
    expect(
      screen.getByText(/Open ones come first, newest first, and the first one is what the banner shows\./),
    ).toBeTruthy()
    seed(session({ pr_sort: "manual" }))
    rerender(<PullRequestsDialog />)
    expect(
      screen.getByText(/In your order; the first one is what the banner shows\. New ones found later go after yours\./),
    ).toBeTruthy()
    await act(async () => {
      fireEvent.click(screen.getByLabelText("Sort pull requests"))
    })
    fireEvent.click(await screen.findByText("Automatic"))
    expect(store.setPullRequestSort).toHaveBeenCalledWith("s1", "automatic")
  })

  it("animates a moving row only when motion is welcome", () => {
    seed(session({ pr_sort: "manual" }))
    const { unmount } = render(<PullRequestsDialog />)
    const row = () => screen.getByLabelText("Drag #418").closest("li") as HTMLElement
    expect(row().style.transition).toBe("transform 250ms ease")
    unmount()
    media.set("(prefers-reduced-motion: reduce)", true)
    render(<PullRequestsDialog />)
    expect(row().style.transition).toBe("")
  })

  it("says where dux looks when the agent has none", () => {
    seed(session({ pr: undefined, prs: undefined, prs_hidden: undefined }))
    render(<PullRequestsDialog />)
    const sentence = screen.getByText(/No pull requests yet/)
    expect(sentence.textContent).toBe(
      "No pull requests yet. dux looks on every branch store-tab-rows has been on, every few minutes.",
    )
    expect(sentence.querySelector("code")?.textContent).toBe("store-tab-rows")
  })
})

describe("PullRequestsDialog header", () => {
  it("gives the view switch the touch floor under a finger, and fills the selected view", () => {
    seed(session())
    render(<PullRequestsDialog />)
    for (const tab of screen.getAllByRole("tab")) {
      expect(tab.className).toMatch(/(^| )pointer-coarse:h-10( |$)/)
      expect(tab.className).toMatch(/(^| )dark:data-active:bg-foreground( |$)/)
      expect(tab.className).toMatch(/(^| )dark:data-active:text-background( |$)/)
    }
  })

  it("draws Sort as its icon alone on a phone, still named for a screen reader", () => {
    const desktopWidth = window.innerWidth
    Object.defineProperty(window, "innerWidth", { value: 360, configurable: true })
    try {
      seed(session())
      render(<PullRequestsDialog />)
      const sort = screen.getByRole("button", { name: "Sort pull requests" })
      expect(sort.textContent).toBe("")
    } finally {
      Object.defineProperty(window, "innerWidth", {
        value: desktopWidth,
        configurable: true,
      })
    }
  })
})

describe("PullRequestsDialog footer", () => {
  it("offers attaching with a signed-in gh", () => {
    seed(session())
    render(<PullRequestsDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Attach pull request…" }))
    expect(store.openAttachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("says attaching needs a signed-in gh instead of offering it", () => {
    seed(session(), false)
    render(<PullRequestsDialog />)
    expect(screen.queryByRole("button", { name: "Attach pull request…" })).toBeNull()
    expect(screen.getByText("Attaching needs a signed-in gh.")).toBeTruthy()
  })
})

describe("PullRequestsDialog branches view", () => {
  it("lists every branch with its pull requests and the attachments on none", async () => {
    seed(session())
    render(<PullRequestsDialog />)
    await act(async () => {
      fireEvent.click(screen.getByRole("tab", { name: /Branches/ }))
    })
    expect(await screen.findByText("spike-tab-ids")).toBeTruthy()
    expect(screen.getByText("Current")).toBeTruthy()
    expect(screen.getByText("No pull request")).toBeTruthy()
    expect(screen.getByText("hidden")).toBeTruthy()
    expect(screen.getByText("tab-state")).toBeTruthy()
    expect(
      screen.getByText(/was attached by you, so it belongs to no branch here\./),
    ).toBeTruthy()
    for (const name of ["#418", "#402"]) {
      const chip = screen.getByRole("link", { name })
      expect(chip.className).toMatch(/(^| )pointer-coarse:h-10( |$)/)
      expect(chip.className).toMatch(/(^| )pointer-coarse:min-w-10( |$)/)
    }
  })

  it("asks again whenever the agent's pull requests change while it is open", async () => {
    seed(session())
    const { rerender } = render(<PullRequestsDialog />)
    await act(async () => {
      fireEvent.click(screen.getByRole("tab", { name: /Branches/ }))
    })
    await waitFor(() => expect(pullRequestBranches).toHaveBeenCalledWith("s1"))
    pullRequestBranches.mockClear()
    rerender(<PullRequestsDialog />)
    expect(pullRequestBranches).not.toHaveBeenCalled()
    seed(session({ pr_revision: 2 }))
    await act(async () => {
      rerender(<PullRequestsDialog />)
    })
    expect(pullRequestBranches).toHaveBeenCalledWith("s1")
  })
})
