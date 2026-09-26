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
import type { Spine } from "@/lib/types"

// Journeys through "Change base branch…" with the REAL store and dialog: only
// the HTTP boundary (fetch) is stubbed, and the spine is overlaid on the
// store's own state because it normally arrives over the events socket.
let spine: Spine
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: (): DuxState => ({ ...actual.useDux(), spine }),
  }
})

type Call = { url: string; method: string; body: string | undefined }
let calls: Call[] = []
// What the branch listing route answers, and when: a test that wants to see
// the loading state holds the reply until it releases it.
let listing: { status: number; body: unknown }
let hold: Promise<void> = Promise.resolve()

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
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET"
      calls.push({ url: String(url), method, body: init?.body as string })
      if (String(url).endsWith("/branches")) {
        await hold
        const text =
          typeof listing.body === "string"
            ? listing.body
            : JSON.stringify(listing.body)
        return new Response(text, { status: listing.status })
      }
      return new Response(null, { status: 204 })
    }),
  )
}
installStubs()

const store = await import("@/lib/store")
const { ChangeBaseBranchDialog } = await import("./ChangeBaseBranchDialog")

function seedSpine(projects: unknown[] = [acme()]) {
  spine = {
    projects,
    sessions: [
      {
        id: "a1",
        title: "fix-login",
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "develop",
          worktree_path: "/wt/fix-login",
        },
      },
    ],
    terminals: [],
    sidebar: { groups: [], agentless_start: null },
  } as unknown as Spine
}

function acme(extra: object = {}) {
  return {
    id: "p1",
    name: "acme",
    path: "/code/acme",
    path_missing: false,
    leading_branch: "main",
    ...extra,
  }
}

const BRANCHES = {
  branches: [
    { name: "main", location: "local", held_by: null },
    { name: "develop", location: "local", held_by: "/wt/fix-login" },
    { name: "old-work", location: "local", held_by: "/elsewhere/wt" },
    { name: "release", location: "remote", held_by: null },
  ],
  fetched: true,
}

function branchRow(name: string): HTMLButtonElement {
  const found = screen
    .getAllByTestId("branch-row")
    .find((row) => row.getAttribute("data-branch") === name)
  if (!found) throw new Error(`no branch row ${name}`)
  return found as HTMLButtonElement
}

async function openFor(projectId = "p1") {
  const view = render(<ChangeBaseBranchDialog />)
  act(() => store.openChangeBaseBranch(projectId))
  await screen.findAllByTestId("branch-row")
  return view
}

beforeEach(() => {
  installStubs()
  calls = []
  hold = Promise.resolve()
  listing = { status: 200, body: BRANCHES }
  seedSpine()
})

afterEach(() => {
  act(() => store.closeChangeBaseBranch())
  cleanup()
  vi.unstubAllGlobals()
})

describe("ChangeBaseBranchDialog", () => {
  it("shows that it is loading until the listing lands", async () => {
    let release = () => {}
    hold = new Promise((resolve) => (release = resolve))
    render(<ChangeBaseBranchDialog />)
    act(() => store.openChangeBaseBranch("p1"))
    expect(screen.getByText(/Fetching origin/)).toBeTruthy()
    release()
    await screen.findAllByTestId("branch-row")
    expect(screen.queryByText(/Fetching origin/)).toBeNull()
    expect(calls[0]).toMatchObject({
      method: "GET",
      url: "/api/v1/projects/p1/branches",
    })
  })

  it("lists the branches, marks the base, and disables held ones naming the holder", async () => {
    await openFor()
    expect(
      screen.getAllByTestId("branch-row").map((r) => r.getAttribute("data-branch")),
    ).toEqual(["main", "develop", "old-work", "release"])
    expect(branchRow("main").textContent).toContain("current base")
    expect(branchRow("release").textContent).toContain("only on origin")
    // Held by an agent's worktree: named by the agent.
    expect(branchRow("develop").disabled).toBe(true)
    expect(branchRow("develop").textContent).toContain("in use by fix-login")
    // Held by a worktree no agent owns: named by its folder.
    expect(branchRow("old-work").disabled).toBe(true)
    expect(branchRow("old-work").textContent).toContain(
      "checked out at /elsewhere/wt",
    )
    expect(branchRow("release").disabled).toBe(false)
    // Nothing to say about the fetch when it worked.
    expect(screen.queryByText(/Not fetched from origin/)).toBeNull()
  })

  it("says so when there is no base recorded yet", async () => {
    seedSpine([acme({ leading_branch: null })])
    await openFor()
    expect(screen.getByText(/No base branch is recorded yet/)).toBeTruthy()
    expect(branchRow("main").textContent).not.toContain("current base")
  })

  it("shows the fetch note when origin could not be fetched", async () => {
    listing = {
      status: 200,
      body: { ...BRANCHES, fetched: false, fetch_error: "timed out after 15s" },
    }
    await openFor()
    expect(
      screen.getByText("Not fetched from origin just now: timed out after 15s."),
    ).toBeTruthy()
  })

  it("says why inside the dialog when the branches cannot be listed", async () => {
    listing = {
      status: 500,
      body: 'Couldn\'t list the branches of project "acme": boom',
    }
    render(<ChangeBaseBranchDialog />)
    act(() => store.openChangeBaseBranch("p1"))
    expect(
      await screen.findByText('Couldn\'t list the branches of project "acme": boom'),
    ).toBeTruthy()
  })

  it("narrows the branches by search", async () => {
    await openFor()
    fireEvent.change(screen.getByLabelText("Search branches"), {
      target: { value: "rel" },
    })
    expect(
      screen.getAllByTestId("branch-row").map((r) => r.getAttribute("data-branch")),
    ).toEqual(["release"])
  })

  it("confirms with the shared prose and posts the choice", async () => {
    await openFor()
    fireEvent.click(branchRow("release"))

    const body = await screen.findByText(/This switches the source checkout for/)
    expect(body.textContent).toBe(
      "This switches the source checkout for acme to release, moving HEAD in the shared repository. New worktrees branch from main now. After the switch, they branch from release.",
    )
    expect([...body.querySelectorAll("code")].map((c) => c.textContent)).toEqual(
      ["acme", "release", "main", "release"],
    )

    fireEvent.click(screen.getByRole("button", { name: "Change base branch" }))
    await waitFor(() =>
      expect(
        calls.find((c) => c.method === "POST"),
      ).toMatchObject({
        url: "/api/v1/projects/p1/base-branch",
        body: JSON.stringify({ branch: "release" }),
      }),
    )
    // Both the confirmation and the picker behind it are gone.
    expect(screen.queryByTestId("branch-row")).toBeNull()
    expect(screen.queryByText(/This switches the source checkout for/)).toBeNull()
  })

  it("returns to the list when the confirmation is cancelled", async () => {
    await openFor()
    fireEvent.click(branchRow("release"))
    await screen.findByText(/This switches the source checkout for/)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(screen.queryByText(/This switches the source checkout for/)).toBeNull()
    expect(screen.getAllByTestId("branch-row")).toHaveLength(4)
    expect(calls.some((c) => c.method === "POST")).toBe(false)
  })

  it("closes itself when the project vanishes", async () => {
    const { rerender } = await openFor()
    seedSpine([])
    rerender(<ChangeBaseBranchDialog />)
    await waitFor(() => expect(screen.queryByTestId("branch-row")).toBeNull())
  })
})
