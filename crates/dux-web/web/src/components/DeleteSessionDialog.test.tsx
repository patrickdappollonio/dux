// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

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
const attachedSession = {
  id: "s3",
  title: "attached",
  workspace: {
    kind: "managed" as const,
    project_id: "p1",
    branch_name: "develop",
    initial_branch: "develop",
    branch_provenance: "attached" as const,
    source_branch: "main",
    worktree_path: "/tmp/develop",
  },
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
  branchUnpushed.mockResolvedValue({ branch: "develop", unpushed_commits: 0 })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("DeleteSessionDialog", () => {
  it("opens for an existing session", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(screen.getByText("Delete agent?")).toBeTruthy()
    expect(screen.getByText(/quacky-mallard/)).toBeTruthy()
  })

  // The branch is its own decision now, so it is its own box. It appears only
  // once the worktree is going, because git will not delete a branch that is
  // still checked out in a worktree.
  it("offers the branch box only once the worktree box is ticked", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(screen.queryByRole("checkbox", { name: /Also delete the branch/ }))
      .toBeNull()

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Also delete the git worktree (irreversible)",
      }),
    )
    const branchBox = screen.getByRole("checkbox", {
      name: /Also delete the branch/,
    })
    expect(branchBox.textContent).toBeDefined()
    expect(screen.getByText("dux/s1")).toBeTruthy()
  })

  it("starts the branch box ticked for a branch dux created", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    const branchBox = screen.getByRole("checkbox", {
      name: /Also delete the branch/,
    })
    expect(branchBox.getAttribute("aria-checked")).toBe("true")
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s1", true, true)
  })

  // The mirror, and the reason the box is a control rather than a label: it
  // spares a branch dux would otherwise have deleted unasked.
  it("sends a declined branch deletion for a branch dux created", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    fireEvent.click(
      screen.getByRole("checkbox", { name: /Also delete the branch/ }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s1", true, false)
  })

  // With the worktree kept there is no branch offer on screen at all, so the
  // request must carry no branch answer either: an answer nobody was asked for
  // is the server deciding on a click that never happened.
  it("sends no branch answer when the worktree is kept", () => {
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s1", false, null)
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
    expect(deleteSession).toHaveBeenCalledWith("sa1", false, null)
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
    expect(deleteSession).toHaveBeenCalledWith("sa1", false, null)
  })

  // A branch that predates the agent is still offered, because the user may
  // genuinely want it gone and nothing else can reach it once the worktree is
  // removed. It starts UNTICKED and it comes with the sentence that says why.
  it("offers a pre-existing branch unticked, with the warning that says why", () => {
    seed("s3", [attachedSession])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    const branchBox = screen.getByRole("checkbox", {
      name: /Also delete the branch/,
    })
    expect(branchBox.getAttribute("aria-checked")).toBe("false")
    expect(screen.getByText(/This branch existed before the agent\./)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s3", true, false)
  })

  it("sends the override when the user ticks a pre-existing branch", () => {
    seed("s3", [attachedSession])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    fireEvent.click(
      screen.getByRole("checkbox", { name: /Also delete the branch/ }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteSession).toHaveBeenCalledWith("s3", true, true)
  })

  it("warns about nothing for a branch dux created", () => {
    // The branch is dux's own; there is no prior owner to warn about, and a
    // sentence here would be noise on the one screen that must be read.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(screen.queryByText(/existed before the agent/)).toBeNull()
  })

  it("says an adopted branch came with its worktree", () => {
    seed("s4", [
      {
        id: "s4",
        title: "adopted",
        workspace: {
          kind: "managed",
          project_id: "",
          branch_name: "main",
          initial_branch: "main",
          branch_provenance: "adopted",
          source_branch: "",
          worktree_path: "",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      screen.getByText(
        /This branch came with the worktree this agent adopted\./,
      ),
    ).toBeTruthy()
  })

  // The count is a git answer that arrives after the dialog is already up, so
  // the warning grows a sentence rather than waiting for it.
  it("names how many commits are pushed nowhere once the count arrives", async () => {
    branchUnpushed.mockResolvedValue({ branch: "develop", unpushed_commits: 3 })
    seed("s3", [attachedSession])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      await screen.findByText(/It has 3 commits not pushed anywhere\./),
    ).toBeTruthy()
  })

  it("says nothing about commits when there are none unpushed", async () => {
    branchUnpushed.mockResolvedValue({ branch: "develop", unpushed_commits: 0 })
    seed("s3", [attachedSession])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      await screen.findByText(/This branch existed before the agent\./),
    ).toBeTruthy()
    expect(screen.queryByText(/not pushed anywhere/)).toBeNull()
  })

  // git may simply not answer (the branch is already gone, the repository is
  // locked). The warning that matters does not depend on it.
  it("keeps the provenance warning when git cannot count", async () => {
    branchUnpushed.mockRejectedValue(new Error("no repo"))
    seed("s3", [attachedSession])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      await screen.findByText(/This branch existed before the agent\./),
    ).toBeTruthy()
    expect(screen.queryByText(/not pushed anywhere/)).toBeNull()
  })

  it("never asks git about a branch dux created", () => {
    // One git call per open, bought only where the answer is rendered.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(branchUnpushed).not.toHaveBeenCalled()
  })

  // A provenance a NEWER server writes and this page has never heard of. The
  // branch survives, so the dialog must say so, but it must not borrow one of
  // the sentences it does know and assert something nobody here can know.
  it("says only what is known about an unrecognized provenance", () => {
    seed("s5", [
      {
        id: "s5",
        title: "from the future",
        workspace: {
          kind: "managed",
          project_id: "",
          branch_name: "mystery",
          initial_branch: "mystery",
          branch_provenance: "unknown",
          source_branch: "",
          worktree_path: "",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      screen
        .getByRole("checkbox", { name: /Also delete the branch/ })
        .getAttribute("aria-checked"),
    ).toBe("false")
    expect(
      screen.getByText(/This branch is not one dux created\./),
    ).toBeTruthy()
    expect(screen.queryByText(/existed before the agent/)).toBeNull()
  })

  it("keeps the branch-deleting default for a server too old to say", () => {
    // An older server omits the field, and it deletes the branch alongside the
    // worktree, so the absent case must default to ticked rather than quietly
    // promising the safer behavior.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    fireEvent.click(screen.getByRole("checkbox"))
    expect(
      screen
        .getByRole("checkbox", { name: /Also delete the branch/ })
        .getAttribute("aria-checked"),
    ).toBe("true")
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
