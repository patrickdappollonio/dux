// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { installBrowserHistory } from "@/test/browserHistory"

// Override `useDux` (seeded spine + target) and spy the store actions the
// dialog fires, while every other store export stays intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    deleteSession: vi.fn(),
    closeDelete: vi.fn(),
  }
})

// The branch-risk read is a real HTTP call; stub the one method the dialog
// uses and leave the rest of the client alone.
vi.mock("@/lib/sessionsApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/sessionsApi")>()
  return {
    ...actual,
    sessionsApi: { ...actual.sessionsApi, branchUnpushed: vi.fn() },
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
// A history the store gives the dialog an entry in, so Back reaches it the way
// the browser delivers it.
const browser = installBrowserHistory()
const { DeleteSessionDialog } = await import("./DeleteSessionDialog")
const store = await import("@/lib/store")
const api = await import("@/lib/sessionsApi")
const deleteSession = vi.mocked(store.deleteSession)
const closeDelete = vi.mocked(store.closeDelete)
const branchUnpushed = vi.mocked(api.sessionsApi.branchUnpushed)

function managed(branch: string) {
  return {
    kind: "managed" as const,
    project_id: "p1",
    branch_name: branch,
    initial_branch: branch,
    branch_provenance: "created" as const,
    source_branch: "main",
    worktree_path: "/tmp/" + branch,
  }
}

const session1 = {
  id: "s1",
  title: "quacky-mallard",
  workspace: managed("dux/s1"),
}
const session2 = {
  id: "s2",
  title: "wobbly-duckling",
  workspace: managed("dux/s2"),
}

const OID_A = "a".repeat(40)
const OID_B = "b".repeat(40)
const OID_C = "c".repeat(40)

type Origin = "dux" | "attached" | "adopted" | "not_dux" | "after" | "before" | "unknown"

// A branch offer as the server answers it. Every count is zero unless a test
// says otherwise, so no warning grows a count sentence it did not ask for.
function offer(
  current: { name: string; origin: Origin; oid?: string } | null,
  others: { name: string; origin: Origin; oid: string }[] = [],
  extra: Partial<import("@/lib/sessionsApi").BranchOffer> = {},
): import("@/lib/sessionsApi").BranchOffer {
  return {
    current_name: current?.name ?? "dux/s1",
    current: current && { oid: OID_A, ...current },
    current_unpushed: { count: 0, has_remote_refs: true },
    current_unpushed_error: null,
    others,
    others_unpushed: others.length ? { count: 0, has_remote_refs: true } : null,
    others_unpushed_error: null,
    both_unpushed: { count: 0, has_remote_refs: true },
    both_unpushed_error: null,
    protected: [],
    ...extra,
  }
}

function seed(target: string | null, sessions: unknown[]) {
  mockState = {
    deleteTarget: target,
    spine: { sessions },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  deleteSession.mockClear()
  closeDelete.mockClear()
  branchUnpushed.mockReset()
  branchUnpushed.mockResolvedValue(offer({ name: "dux/s1", origin: "dux" }))
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// A sentence with names in it is split across chips, so it is found by the
// paragraph's whole text rather than by one text node.
function paragraph(pattern: RegExp): HTMLElement {
  const found = screen
    .getAllByText((_, el) => el?.tagName === "P" && pattern.test(el.textContent ?? ""))
  return found[0]
}

function chipsIn(el: HTMLElement): (string | null)[] {
  return [...el.querySelectorAll("code")].map((c) => c.textContent)
}

function tickWorktree() {
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Also delete the git worktree (irreversible)",
    }),
  )
}

function currentBox(): HTMLElement {
  return screen.getByRole("checkbox", { name: /Also delete the branch / })
}

function othersBox(): HTMLElement | null {
  return screen.queryByRole("checkbox", { name: /Also delete the other branch/ })
}


describe("DeleteSessionDialog", () => {
  // The browser's Back abandons the dialog exactly as Cancel does: it closes,
  // and the agent is not deleted.
  it("opens for an existing session, and the browser's Back abandons it", async () => {
    seed("s1", [session1])
    const before = browser.index
    render(<DeleteSessionDialog />)
    expect(screen.getByText("Delete agent?")).toBeTruthy()
    expect(screen.getByText(/quacky-mallard/)).toBeTruthy()
    await waitFor(() => {
      expect(browser.index).toBe(before + 1)
    })
    act(() => browser.back())
    expect(closeDelete).toHaveBeenCalledOnce()
    expect(deleteSession).not.toHaveBeenCalled()
  })


  // The branch is its own decision, so it is its own box. It appears only once
  // the worktree is going, because git will not delete a branch that is still
  // checked out in a worktree.
  it("offers the branch box only once the worktree box is ticked", async () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(screen.queryByRole("checkbox", { name: /Also delete the branch/ }))
      .toBeNull()
    tickWorktree()
    expect(currentBox()).toBeTruthy()
    await waitFor(() => expect(currentBox().getAttribute("aria-disabled")).not.toBe("true"))
  })

  // Until the offer lands nothing is pinned, so nothing can be deleted on a
  // guess: the box is there, named after the spine's branch, but disabled, and
  // a Delete pressed now keeps every branch, saying why on the wire.
  it("disables the boxes while the offer is pending, and sends no pins", () => {
    branchUnpushed.mockReturnValue(new Promise(() => {}))
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    const box = screen.getByRole("checkbox", {
      name: "Also delete the branch dux/s1",
    })
    expect(box.getAttribute("aria-disabled")).toBe("true")
    expect(box.getAttribute("aria-checked")).toBe("false")
    expect(othersBox()).toBeNull()
    expect(screen.getByText("Checking the branches…")).toBeTruthy()
    const confirm = screen.getByRole("button", { name: "Delete" })
    expect((confirm as HTMLButtonElement).disabled).toBe(false)
    fireEvent.click(confirm)
    const unchecked = { delete: false, branches: [], unchecked: "pending" }
    expect(deleteSession).toHaveBeenCalledWith(
      "s1",
      true,
      { current: unchecked, others: unchecked },
      null,
    )
  })

  it("keeps the boxes disabled when the check fails, and says why", async () => {
    branchUnpushed.mockRejectedValue(new Error("git could not be run"))
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(
      await screen.findByText(
        "dux could not check the branches (git could not be run), so they will be kept.",
      ),
    ).toBeTruthy()
    expect(currentBox().getAttribute("aria-disabled")).toBe("true")
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    const unchecked = { delete: false, branches: [], unchecked: "failed" }
    expect(deleteSession).toHaveBeenCalledWith(
      "s1",
      true,
      { current: unchecked, others: unchecked },
      null,
    )
  })

  // A box left out of the body would let the server decide, so a missing
  // current branch fails BOTH boxes, the second one included.
  // Only the box git could not read fails; the other box still answers.
  it("fails the first box alone when git could not read the current branch", async () => {
    branchUnpushed.mockResolvedValue(
      offer(null, [{ name: "spike", origin: "dux", oid: OID_B }]),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(
      await screen.findByText(
        "dux could not check the branches (git could not read the branch), so they will be kept.",
      ),
    ).toBeTruthy()
    await waitFor(() => expect(othersBox()?.getAttribute("aria-checked")).toBe("true"))
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith(
      "s1",
      true,
      {
        current: { delete: false, branches: [], unchecked: "failed" },
        others: {
          delete: true,
          branches: [{ name: "spike", oid: OID_B }],
          offered: ["spike"],
        },
      },
      null,
    )
  })

  // The project's own branch is named and offered in no box: never a failed
  // check, and the delete says the box was not offered.
  it("names the project's own branch and offers no box for it", async () => {
    branchUnpushed.mockResolvedValue(offer(null, [], { protected: ["dux/s1"] }))
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(await screen.findByText(/is the project's own branch, so dux keeps it\./)).toBeTruthy()
    expect(screen.queryByText(/could not check the branches/)).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    const notOffered = { delete: false, branches: [], unchecked: "not_offered" }
    expect(deleteSession).toHaveBeenCalledWith(
      "s1",
      true,
      { current: notOffered, others: notOffered },
      null,
    )
  })

  it("offers no second box when the worktree was on no other branch", async () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(currentBox().getAttribute("aria-checked")).toBe("true"))
    expect(othersBox()).toBeNull()
  })

  // The first box is ticked for a branch made after the agent started (the
  // user's answer for a switch onto a new branch) and unticked for one that
  // predates it, which also says why.
  it("defaults the first box by origin", async () => {
    branchUnpushed.mockResolvedValue(offer({ name: "feat", origin: "after" }))
    seed("s1", [session1])
    const { unmount } = render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(currentBox().getAttribute("aria-checked")).toBe("true"))
    unmount()

    branchUnpushed.mockResolvedValue(offer({ name: "develop", origin: "before" }))
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(
      await screen.findByText(/This branch existed before the agent\./),
    ).toBeTruthy()
    expect(currentBox().getAttribute("aria-checked")).toBe("false")
  })

  // The second box is ticked only when dux can prove every branch in it is
  // its own: one branch made after the agent started is not proof.
  it("defaults the second box to ticked only when every branch is dux's", async () => {
    branchUnpushed.mockResolvedValue(
      offer({ name: "dux/s1", origin: "dux" }, [
        { name: "a", origin: "dux", oid: OID_B },
        { name: "spike", origin: "after", oid: OID_C },
      ]),
    )
    seed("s1", [session1])
    const { unmount } = render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(othersBox()).toBeTruthy())
    expect(othersBox()!.getAttribute("aria-checked")).toBe("false")
    expect(paragraph(/dux did not make spike/)).toBeTruthy()
    unmount()

    branchUnpushed.mockResolvedValue(
      offer({ name: "dux/s1", origin: "dux" }, [
        { name: "a", origin: "dux", oid: OID_B },
        { name: "b", origin: "dux", oid: OID_C },
      ]),
    )
    render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(othersBox()).toBeTruthy())
    expect(othersBox()!.getAttribute("aria-checked")).toBe("true")
  })

  // The delete carries exactly the entries the offer pinned, each with the
  // oid it was shown at, and an unticked box carries none.
  it("sends the pinned entries of a ticked box and nothing for an unticked one", async () => {
    branchUnpushed.mockResolvedValue(
      offer({ name: "dux/s1", origin: "dux" }, [
        { name: "a", origin: "dux", oid: OID_B },
        { name: "b", origin: "dux", oid: OID_C },
      ]),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(othersBox()).toBeTruthy())
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenLastCalledWith(
      "s1",
      true,
      {
        current: {
          delete: true,
          branches: [{ name: "dux/s1", oid: OID_A }],
          offered: ["dux/s1"],
        },
        others: {
          delete: true,
          branches: [
            { name: "a", oid: OID_B },
            { name: "b", oid: OID_C },
          ],
          offered: ["a", "b"],
        },
      },
      null,
    )
  })

  // Either default is overridable in either direction: a predating branch
  // ticked, dux's own branches unticked.
  it("lets the user override both boxes", async () => {
    branchUnpushed.mockResolvedValue(
      offer({ name: "develop", origin: "before" }, [
        { name: "spike", origin: "dux", oid: OID_B },
      ]),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(othersBox()).toBeTruthy())
    fireEvent.click(currentBox())
    fireEvent.click(othersBox()!)
    expect(paragraph(/Unticked, it stays as an ordinary branch\./)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenLastCalledWith(
      "s1",
      true,
      {
        current: {
          delete: true,
          branches: [{ name: "develop", oid: OID_A }],
          offered: ["develop"],
        },
        others: { delete: false, branches: [], offered: ["spike"] },
      },
      null,
    )
  })

  // A dux-made `born` with three commits of its own under a `drifted` with
  // one more: the sentence follows the ticks, and with both ticked it counts
  // all four, which neither box's own count would show.
  it("says what the boxes ticked right now lose", async () => {
    branchUnpushed.mockResolvedValue(
      offer(
        { name: "drifted", origin: "after" },
        [{ name: "born", origin: "dux", oid: OID_B }],
        {
          current_unpushed: { count: 1, has_remote_refs: true },
          others_unpushed: { count: 0, has_remote_refs: true },
          both_unpushed: { count: 4, has_remote_refs: true },
        },
      ),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(
      await screen.findByText(
        /loses 4 commits that are on no remote and no branch that stays\./,
      ),
    ).toBeTruthy()
    fireEvent.click(othersBox()!)
    expect(
      paragraph(/^Deleting drifted loses 1 commit that is on no remote/),
    ).toBeTruthy()
    fireEvent.click(currentBox())
    expect(screen.queryByText(/^Deleting /)).toBeNull()
  })

  // Whether a missing first box is the project's own branch is decided from
  // the name the offer was read for, not from the agent's branch now.
  it("decides the missing first box from the name the offer was read for", async () => {
    branchUnpushed.mockResolvedValue(
      offer(null, [], { current_name: "release-1.2", protected: ["release-1.2"] }),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    expect(await screen.findByText(/is the project's own branch/)).toBeTruthy()
    expect(screen.queryByText(/could not check the branches/)).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    const notOffered = { delete: false, branches: [], unchecked: "not_offered" }
    expect(deleteSession).toHaveBeenLastCalledWith(
      "s1",
      true,
      { current: notOffered, others: notOffered },
      null,
    )
  })

  it("asks git nothing until the branch box is on screen", () => {
    // One git call, bought only where its answer is rendered: with the worktree
    // kept there is no branch offer at all.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(branchUnpushed).not.toHaveBeenCalled()
    tickWorktree()
    expect(branchUnpushed).toHaveBeenCalledWith("s1")
  })

  // Reopening is a new question: the answers and the offer start over.
  it("forgets the answers and the offer on reopen", async () => {
    seed("s1", [session1, session2])
    const { rerender } = render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(currentBox().getAttribute("aria-checked")).toBe("true"))
    fireEvent.click(currentBox())
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    seed(null, [session1, session2])
    rerender(<DeleteSessionDialog />)

    branchUnpushed.mockReturnValue(new Promise(() => {}))
    seed("s2", [session1, session2])
    rerender(<DeleteSessionDialog />)
    tickWorktree()
    expect(
      screen.getByRole("checkbox", { name: "Also delete the branch dux/s2" })
        .getAttribute("aria-disabled"),
    ).toBe("true")
    expect(screen.getByText("Checking the branches…")).toBeTruthy()
  })
  // With the worktree kept there is no branch offer on screen at all, so the
  // request must carry no branch answer either: an answer nobody was asked for
  // is the server deciding on a click that never happened.
  //
  // Refused because a browser is watching the agent, the dialog stays open,
  // names who as chips, puts focus back on Cancel and offers Delete anyway,
  // which goes ahead over them.
  it("sends no branch answer when the worktree is kept, and goes ahead over who is attached only once they are named", async () => {
    seed("s1", [
      { ...session1, tabs: [{ id: "s1-slot", provider: "claude" }] },
    ])
    deleteSession.mockResolvedValueOnce([
      {
        surface: "browser",
        device: "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
        address: "10.0.0.7",
        verified: false,
        driving: false,
        target: { kind: "tab", id: "s1-slot", agent: "s1" },
        key: "k-firefox",
      },
    ])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s1", false, null, null)

    const override = await screen.findByRole("button", { name: "Delete anyway" })
    expect(closeDelete).not.toHaveBeenCalled()
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Cancel" }),
    )
    const who = screen.getByText((_, el) => el?.tagName === "LI")
    expect(chipsIn(who)).toEqual(["Firefox on Linux", "Claude", "quacky-mallard"])
    expect(who.textContent).toContain("at 10.0.0.7 (unverified), watching tab")

    // The second click of the double click that was refused lands on the
    // override and must not go ahead.
    fireEvent.click(override, { detail: 2 })
    expect(deleteSession).toHaveBeenCalledTimes(1)

    deleteSession.mockResolvedValueOnce(null)
    fireEvent.click(override)
    // The override goes ahead over exactly who it showed.
    expect(deleteSession).toHaveBeenLastCalledWith("s1", false, null, ["k-firefox"])
    await vi.waitFor(() => expect(closeDelete).toHaveBeenCalled())
  })

  // A refusal that arrives after its dialog was cancelled belongs to nobody:
  // the next agent's dialog names nobody and its first confirm is a plain one.
  it("ignores a refusal that arrives after the dialog moved on to another agent", async () => {
    seed("s1", [session1, session2])
    let answerLate: (blockers: unknown) => void = () => {}
    deleteSession.mockImplementationOnce(
      () => new Promise((resolve) => (answerLate = resolve)) as never,
    )
    const { rerender } = render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    seed(null, [session1, session2])
    rerender(<DeleteSessionDialog />)

    answerLate([
      {
        surface: "browser",
        device: null,
        address: "10.0.0.7",
        verified: false,
        driving: false,
        target: { kind: "tab", id: "s1-slot", agent: "s1" },
        key: "k-late",
      },
    ])
    seed("s2", [session1, session2])
    rerender(<DeleteSessionDialog />)
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(screen.queryByRole("button", { name: "Delete anyway" })).toBeNull()
    const confirm = screen.getByRole("button", { name: "Delete" })
    expect((confirm as HTMLButtonElement).disabled).toBe(false)
    deleteSession.mockResolvedValueOnce(null)
    fireEvent.click(confirm)
    expect(deleteSession).toHaveBeenLastCalledWith("s2", false, null, null)
  })

  // THE PIN. There is no worktree to remove and no branch to delete, so the
  // checkbox is not merely unchecked, it does not exist: the offer cannot be
  // rendered, so it cannot be ticked. The copy says the folder is untouched,
  // because "removes the agent from dux" alone reads as though something on
  // disk went with it.
  it("offers no worktree checkbox for a standalone agent, and says the folder is untouched", () => {
    seed("sa1", [
      {
        id: "sa1",
        title: "notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/notes",
          folder_label: "~/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    expect(screen.queryByRole("checkbox")).toBeNull()
    expect(screen.getByText(/left untouched/i)).toBeTruthy()
    expect(screen.getByText(/~\/notes/)).toBeTruthy()
  })

  // And the request that reaches the server must never ask for the removal:
  // the default is false, and with no control to flip it there is no way for
  // it to become true.
  it("never asks the server to remove a standalone agent's folder", () => {
    seed("sa1", [
      {
        id: "sa1",
        title: "notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/notes",
          folder_label: "~/notes",
          repo_status: "working_repo",
          quiet_reason: "",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("sa1", false, null, null)
  })

  // The checkbox state outlives one open: the dialog stays mounted. A box ticked
  // for a managed agent and then reopened on a standalone one would send
  // `delete_worktree: true` for an agent that has no worktree, which the server
  // refuses out loud, with no control on screen to clear it.
  it("does not carry a ticked worktree box over to a standalone agent", () => {
    const standalone = {
      id: "sa1",
      title: "notes",
      workspace: {
        kind: "folder",
        folder_path: "/home/someone/notes",
        folder_label: "~/notes",
        repo_status: "working_repo",
        quiet_reason: "",
      },
    }
    seed("s1", [session1, standalone])
    const { rerender } = render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))

    seed("sa1", [session1, standalone])
    rerender(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("sa1", false, null, null)
  })

  it("calls closeDelete when the session vanishes mid-open", () => {
    seed("s1", [session1])
    const { rerender } = render(<DeleteSessionDialog />)
    seed("s1", [])
    rerender(<DeleteSessionDialog />)
    expect(closeDelete).toHaveBeenCalled()
  })

  it("resets the checkbox on a vanish-close, so reopening for another session is unchecked", () => {
    seed("s1", [session1])
    const { rerender } = render(<DeleteSessionDialog />)

    const checkbox = screen.getByRole("checkbox")
    fireEvent.click(checkbox)
    expect(checkbox.getAttribute("aria-checked")).toBe("true")

    // The session vanishes mid-open, the dialog closes.
    seed("s1", [])
    rerender(<DeleteSessionDialog />)
    expect(closeDelete).toHaveBeenCalled()

    // Reopen for another existing session; the checkbox must render unchecked.
    seed("s2", [session2])
    rerender(<DeleteSessionDialog />)
    expect(screen.getByText(/wobbly-duckling/)).toBeTruthy()
    const reopenedCheckbox = screen.getByRole("checkbox")
    expect(reopenedCheckbox.getAttribute("aria-checked")).toBe("false")
  })
})

// Every name the dialog shows is the shared chip, and the chip replaces the
// quotes the sentences used to wrap names in.
describe("DeleteSessionDialog names", () => {
  it("chips the agent's name in the opening sentence", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    const opening = paragraph(/This removes the agent session/)
    expect(opening.textContent).toBe(
      "This removes the agent session quacky-mallard from dux.",
    )
    expect(chipsIn(opening)).toEqual(["quacky-mallard"])
  })

  it("chips every branch in both boxes' labels and warnings", async () => {
    branchUnpushed.mockResolvedValue(
      offer(
        { name: "develop", origin: "before" },
        [
          { name: "a", origin: "before", oid: OID_B },
          { name: "b", origin: "unknown", oid: OID_C },
        ],
        {
          current_unpushed: null,
          current_unpushed_error: "fatal: bad object",
        },
      ),
    )
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    tickWorktree()
    await waitFor(() => expect(othersBox()).toBeTruthy())
    const current = document.querySelector('label[for="delete-branch"]') as HTMLElement
    expect(chipsIn(current)).toEqual(["develop"])
    const others = document.querySelector(
      'label[for="delete-other-branches"]',
    ) as HTMLElement
    expect(others.textContent).toBe(
      "Also delete the other branches this agent was on: a and b",
    )
    expect(chipsIn(others)).toEqual(["a", "b"])
    // The count sentence is about the boxes ticked right now.
    fireEvent.click(currentBox())
    expect(chipsIn(paragraph(/dux could not count the unpushed commits on develop/)))
      .toEqual(["develop"])
    expect(chipsIn(paragraph(/existed before the agent\. dux cannot tell when/)))
      .toEqual(["a", "b"])
  })

  it("chips a standalone agent's folder with no quotes around it", () => {
    seed("sa1", [
      {
        id: "sa1",
        title: "notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/notes",
          folder_label: "~/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    const folder = paragraph(/Its folder/)
    expect(chipsIn(folder)).toEqual(["~/notes"])
    expect(folder.textContent).toContain("Its folder ~/notes is left untouched")
  })
})
