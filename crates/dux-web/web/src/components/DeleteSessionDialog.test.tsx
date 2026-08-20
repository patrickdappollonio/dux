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
const deleteSession = vi.mocked(store.deleteSession)
const closeDelete = vi.mocked(store.closeDelete)

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

  it("says the branch goes too, because this path deletes it", () => {
    // The checkbox used to promise only the worktree while the same code path
    // also ran `git branch -D`, on the current branch AND the one the agent was
    // born on. The TUI's checkbox has always said "worktree and branch"; the web
    // has to say it as well or the user is agreeing to less than happens.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(
      screen.getByRole("checkbox", {
        name: "Also delete the git worktree and its branch (irreversible)",
      }),
    ).toBeTruthy()
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
    expect(deleteSession).toHaveBeenCalledWith("sa1", false)
  })

  it("says the branch is kept when the agent attached to one it did not create", () => {
    // Promising to delete a branch dux will deliberately keep is the same class
    // of bug from the other direction: the dialog has to describe what happens.
    seed("s3", [
      {
        id: "s3",
        title: "attached",
        workspace: {
          kind: "managed",
          project_id: "",
          branch_name: "develop",
          initial_branch: "develop",
          branch_provenance: "attached",
          source_branch: "",
          worktree_path: "",
        },
      },
    ])
    render(<DeleteSessionDialog />)
    expect(
      screen.getByRole("checkbox", {
        name: "Also delete the git worktree, keeping its branch (irreversible)",
      }),
    ).toBeTruthy()
    expect(
      screen.getByText(/existed before this agent, so dux keeps it/),
    ).toBeTruthy()
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
    expect(
      screen.getByText(
        /came with the worktree this agent adopted, so dux keeps it/,
      ),
    ).toBeTruthy()
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
    expect(
      screen.getByRole("checkbox", {
        name: "Also delete the git worktree, keeping its branch (irreversible)",
      }),
    ).toBeTruthy()
    expect(
      screen.getByText(/is not a branch dux created, so dux keeps it/),
    ).toBeTruthy()
    expect(screen.queryByText(/existed before this agent/)).toBeNull()
  })

  it("keeps the branch-deleting copy for a server too old to say", () => {
    // An older server omits the field and deletes the branch either way, so the
    // absent case must not quietly promise the safer behavior.
    seed("s1", [session1])
    render(<DeleteSessionDialog />)
    expect(
      screen.getByRole("checkbox", {
        name: "Also delete the git worktree and its branch (irreversible)",
      }),
    ).toBeTruthy()
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
