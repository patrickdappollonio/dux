// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

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
    on_no_listed_branch: [{ host: "github.com", owner_repo: "acme/infra", number: 77 }],
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
    const toggle = screen.getByRole("button", { name: /Hidden/ })
    expect(within(toggle).getByText("1").className).toMatch(/(^| )text-muted-foreground( |$)/)
    fireEvent.click(toggle)
    const title = screen.getByText("Key provider state by tab id")
    expect(title.className).toMatch(/(^| )text-muted-foreground( |$)/)
    expect(screen.getByText("closed")).toBeTruthy()
    // Show again drops below the title rather than squeezing it.
    const showAgain = screen.getByRole("button", { name: "Show again" })
    expect(showAgain.parentElement?.className).toMatch(/(^| )flex-wrap( |$)/)
    expect(title.closest("li")?.className).toMatch(/(^| )items-center( |$)/)

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

  it("keeps a row's details on one line with the branch giving way first, beside a small muted Attached tag, and gives the title the width on a phone", () => {
    const twoDaysAgo = new Date(Date.now() - 2 * 24 * 3600e3).toISOString()
    seed(session({ prs: [found, attached, { ...both, merged_at: twoDaysAgo }] }))
    render(<PullRequestsDialog />)
    const branch = screen.getByText("tab-slot-pointer")
    expect(branch.className).toMatch(/(^| )truncate( |$)/)
    const details = branch.closest("[data-slot='pr-details']") as HTMLElement
    expect(details.className).toMatch(/(^| )whitespace-nowrap( |$)/)
    expect(details.className).not.toMatch(/flex-wrap/)
    const tag = screen.getAllByText("Attached")[0]
    expect(tag.className).toMatch(/(^| )text-muted-foreground( |$)/)
    expect(tag.className).toMatch(/(^| )text-\[11px\]( |$)/)
    expect(screen.getByLabelText("Drag #418").closest("li")?.className).toMatch(
      /(^| )items-center( |$)/,
    )
    // On a phone the title keeps a readable width beside the grip: the
    // relative time and the leading state icon go (the state is still the
    // first word below the title), and the grip and the menu narrow under a
    // finger, keeping the 40px floor in height.
    const when = within(details).getByText(/^merged .+/)
    expect(when.closest("[data-slot='pr-detail']")?.className).toMatch(/(^| )max-md:hidden( |$)/)
    expect(within(details).getByText("tab-slot-pointer").closest("[data-slot='pr-detail']")
      ?.className).not.toMatch(/max-md:hidden/)
    const row = screen.getByLabelText("Drag #415").closest("li") as HTMLElement
    expect(row.querySelector("svg.lucide-git-pull-request")?.getAttribute("class")).toMatch(
      /(^| )max-md:hidden( |$)/,
    )
    for (const control of [
      screen.getByLabelText("Drag #415"),
      within(row).getByLabelText("Actions for #415"),
    ]) {
      expect(control.className).toMatch(/(^| )max-md:pointer-coarse:w-7( |$)/)
      expect(control.className).toMatch(/(^| )pointer-coarse:size-10( |$)/)
    }
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
  it("lists every branch with its pull requests and the ones on none", async () => {
    const answer = await pullRequestBranches("s1")
    const onMain = pr({ number: 390, title: "Bump the toolchain", head_branch: "main" })
    const headUnknown = pr({ number: 380, title: "Carried over", head_branch: undefined })
    const hiddenOnRelease = pr({ number: 370, title: "Cut the release", head_branch: "release" })
    // Found before the worktree's remote moved, on a branch listed here.
    const otherRepoOnListed = pr({
      number: 360,
      title: "Rename the tab state",
      owner_repo: "acme/old-api",
      own_repo: false,
      head_branch: "store-tab-rows",
    })
    // The order is the server's to decide, so the dialog keeps it as sent.
    pullRequestBranches.mockResolvedValue({
      ...answer,
      branches: [answer.branches[1], answer.branches[0], answer.branches[2]],
      on_no_listed_branch: [
        ...answer.on_no_listed_branch,
        { host: "github.com", owner_repo: "acme/api", number: 390 },
        { host: "github.com", owner_repo: "acme/api", number: 380 },
        { host: "github.com", owner_repo: "acme/api", number: 370 },
        { host: "github.com", owner_repo: "acme/old-api", number: 360 },
        // Not on the spine yet: the answer and the spine are out of step.
        { host: "github.com", owner_repo: "acme/infra", number: 350 },
      ],
    })
    seed(
      session({
        prs: [found, attached, both, onMain, headUnknown, otherRepoOnListed],
        prs_hidden: [hidden, hiddenOnRelease],
      }),
    )
    render(<PullRequestsDialog />)
    await act(async () => {
      fireEvent.click(screen.getByRole("tab", { name: /Branches/ }))
    })
    expect(await screen.findByText("spike-tab-ids")).toBeTruthy()
    expect(
      [...document.querySelectorAll("[data-slot='branch-name']")].map((el) => el.textContent),
    ).toEqual(["tab-state-by-id", "store-tab-rows", "spike-tab-ids"])
    expect(screen.queryByLabelText("Sort pull requests")).toBeNull()
    // Switching views keeps the dialog's top where it was: it is anchored by
    // its top, and the other view's sentence still holds its place, unseen.
    expect(document.querySelector("[data-slot='dialog-content']")?.className).toMatch(
      /(^| )translate-y-0( |$)/,
    )
    expect(screen.getByText(/Open ones come first/).closest("[aria-hidden='true']")).not.toBeNull()
    expect(screen.getByText("Current").className).toMatch(/(^| )text-muted-foreground( |$)/)
    expect(screen.getByText("spike-tab-ids").closest("li")?.className).toMatch(
      /(^| )items-center( |$)/,
    )
    expect(screen.getByText("No pull request")).toBeTruthy()
    expect(screen.getByText("hidden")).toBeTruthy()
    expect(screen.getByText("tab-state")).toBeTruthy()
    expect(
      screen.getByText(/was attached by you, so it belongs to no branch here\./),
    ).toBeTruthy()
    const apart = [...document.querySelectorAll("p")].map((el) => el.textContent)
    expect(apart).toContain("#390 was found on main, a branch not listed here.")
    expect(apart).toContain("#380 was found on a branch dux has not learned yet.")
    expect(apart).toContain("#370 was found on release, a branch not listed here. Hidden.")
    expect(apart).toContain(
      "acme/old-api#360 was found on store-tab-rows in another repository, so it belongs to no branch here.",
    )
    expect(apart).toContain("acme/infra#350 belongs to no branch here.")
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
