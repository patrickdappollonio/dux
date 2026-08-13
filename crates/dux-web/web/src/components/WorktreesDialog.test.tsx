// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import type { DuxState } from "@/lib/store"
import type { ProjectWorktreeEntryView } from "@/lib/types"

// Override only `useDux` so the dialog reads our seeded state, while the real
// store exports (closeAttachWorktree, attachWorktree) stay intact.
let mockState: DuxState
const openNewAgentPicker = vi.fn()
const deleteProjectWorktree = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openNewAgentPicker: (...args: unknown[]) => openNewAgentPicker(...args),
    deleteProjectWorktree: (...args: unknown[]) => deleteProjectWorktree(...args),
  }
})

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what each row's tooltip is wired to reveal.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: ReactNode
    content: ReactNode
  }) => (
    <>
      {children}
      <div data-testid="tooltip-content">{content}</div>
    </>
  ),
}))

// The real store boots on import (localStorage + bootstrap fetch). jsdom doesn't
// provide those as bare globals, so stub them before the component loads.
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
const { WorktreesDialog } = await import("./WorktreesDialog")

// A path and branch long enough that both would overflow the fixed-width dialog
// without the truncation fix.
const LONG_PATH =
  "/home/user/projects/really-long-worktree-directory-name-that-overflows"
const LONG_BRANCH =
  "feature/some-really-long-branch-name-that-would-overflow-the-row"

function entry(
  overrides: Partial<ProjectWorktreeEntryView> = {},
): ProjectWorktreeEntryView {
  return {
    worktree_path: LONG_PATH,
    branch_name: LONG_BRANCH,
    branch: LONG_BRANCH,
    adoptable: true,
    reason: null,
    dirty: false,
    agent_id: null,
    ...overrides,
  }
}

function seed(
  entries: ProjectWorktreeEntryView[],
  extra: Partial<DuxState> = {},
) {
  mockState = {
    attachWorktreeTarget: "p1",
    attachWorktreeEntries: entries,
    attachWorktreeLoading: false,
    attachWorktreeFromPicker: false,
    deleteWorktreeTarget: null,
    spine: {
      projects: [{ id: "p1", name: "acme" }],
      sessions: [{ id: "s1", title: "tidy-otter", branch_name: "held" }],
    },
    ...extra,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  openNewAgentPicker.mockClear()
  deleteProjectWorktree.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("WorktreesDialog", () => {
  it("is titled Worktrees and still offers adoption as a row action", () => {
    // One surface does both jobs: the dialog is the manager, and adopting is an
    // action on a row rather than the purpose of a separate dialog.
    seed([entry()])
    render(<WorktreesDialog />)
    expect(screen.getByText("Worktrees in acme")).toBeTruthy()
    expect(screen.getByRole("button", { name: "Create agent" })).toBeTruthy()
  })

  it("shows each row's branch and path, and truncates both", () => {
    seed([entry()])
    render(<WorktreesDialog />)
    // The stubbed tooltip renders its content inline too, so both strings
    // appear twice; the ROW's copy is the one carrying `truncate`.
    const truncated = (text: string) =>
      screen.getAllByText(text).some((el) => el.className.includes("truncate"))
    expect(truncated(LONG_BRANCH)).toBe(true)
    expect(truncated(LONG_PATH)).toBe(true)
  })

  it("marks a worktree with uncommitted changes and leaves a clean one unmarked", () => {
    seed([
      entry({ worktree_path: "/wt/clean", branch_name: "clean" }),
      entry({ worktree_path: "/wt/messy", branch_name: "messy", dirty: true }),
    ])
    render(<WorktreesDialog />)
    const marks = screen.getAllByText("Uncommitted changes")
    expect(marks.length).toBe(1)
  })

  it("gives an adoptable row a delete action and an attached row none", () => {
    // Deleting a worktree from under a live agent is how you get a broken
    // session, so the attached row points at the agent instead.
    seed([
      entry({ worktree_path: "/wt/free", branch_name: "free" }),
      entry({
        worktree_path: "/wt/held",
        branch_name: "held",
        adoptable: false,
        agent_id: "s1",
        reason: "Already has an agent.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.getAllByRole("button", { name: "Worktree actions" }).length).toBe(1)
    // And the attached row names the agent holding it.
    expect(screen.getByText(/tidy-otter/)).toBeTruthy()
  })

  it("names the branch, the full path and the loss in the delete confirmation", () => {
    seed([entry({ worktree_path: "/wt/messy", branch_name: "messy", dirty: true })], {
      deleteWorktreeTarget: {
        projectId: "p1",
        entry: entry({
          worktree_path: "/wt/messy",
          branch_name: "messy",
          dirty: true,
        }),
      },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)
    const confirm = screen.getByTestId("delete-worktree-confirm")
    expect(confirm.textContent).toContain("messy")
    expect(confirm.textContent).toContain("/wt/messy")
    expect(confirm.textContent).toContain("cannot be undone")
    // Dirty is called out specifically, not generically.
    expect(confirm.textContent).toContain("uncommitted changes")
  })

  it("does not claim uncommitted work when the worktree is clean", () => {
    seed([entry({ worktree_path: "/wt/clean", branch_name: "clean" })], {
      deleteWorktreeTarget: {
        projectId: "p1",
        entry: entry({ worktree_path: "/wt/clean", branch_name: "clean" }),
      },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)
    const confirm = screen.getByTestId("delete-worktree-confirm")
    expect(confirm.textContent).not.toContain("uncommitted changes")
    expect(confirm.textContent).toContain("cannot be undone")
  })

  // The confirm dialog opens ON TOP of the worktree list, and base-ui marks the
  // whole tree `aria-hidden` while two dialogs are stacked, so every role query
  // inside the confirmation needs `hidden: true`. The older confirm tests dodge
  // this by reading a testid instead.
  it("defaults the branch checkbox ON and deletes the branch with the worktree", () => {
    // The maintainer's decision: this dialog already says the removal is
    // forcible and has no trash, and deleting the branch is what the user came
    // for. The server still defaults to false for a request that says nothing.
    const target = entry({ worktree_path: "/wt/messy", branch_name: "messy", branch: "messy" })
    seed([target], {
      deleteWorktreeTarget: { projectId: "p1", entry: target },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)

    const box = screen.getByRole("checkbox", {
      name: /also delete the branch messy/i,
      hidden: true,
    })
    expect(box.getAttribute("aria-checked")).toBe("true")
    const confirm = screen.getByTestId("delete-worktree-confirm")
    expect(confirm.textContent).toContain("will be deleted")
    expect(confirm.textContent).not.toContain("is kept")

    fireEvent.click(screen.getByRole("button", { name: "Delete worktree", hidden: true }))
    expect(deleteProjectWorktree).toHaveBeenCalledWith("p1", "/wt/messy", true)
  })

  it("keeps the branch, and says so, once the checkbox is unticked", () => {
    const target = entry({ worktree_path: "/wt/messy", branch_name: "messy", branch: "messy" })
    seed([target], {
      deleteWorktreeTarget: { projectId: "p1", entry: target },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /also delete the branch messy/i,
        hidden: true,
      }),
    )

    const confirm = screen.getByTestId("delete-worktree-confirm")
    expect(confirm.textContent).toContain("is kept")
    fireEvent.click(screen.getByRole("button", { name: "Delete worktree", hidden: true }))
    expect(deleteProjectWorktree).toHaveBeenCalledWith("p1", "/wt/messy", false)
  })

  it("offers no branch checkbox for a detached worktree", () => {
    // There is no branch to delete, so offering the choice would be a lie and
    // the request must not ask for one.
    const target = entry({
      worktree_path: "/wt/loose",
      branch_name: "detached 1a2b3c4",
      branch: null,
    })
    seed([target], {
      deleteWorktreeTarget: { projectId: "p1", entry: target },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)

    expect(screen.queryByRole("checkbox", { hidden: true })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Delete worktree", hidden: true }))
    expect(deleteProjectWorktree).toHaveBeenCalledWith("p1", "/wt/loose", false)
  })

  it("offers Back only when the user drilled in from the project picker", () => {
    seed([entry()])
    render(<WorktreesDialog />)
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull()
    cleanup()

    seed([entry()], { attachWorktreeFromPicker: true } as Partial<DuxState>)
    render(<WorktreesDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    // Back returns to the project list; Cancel (still present) closes the lot.
    expect(openNewAgentPicker).toHaveBeenCalledWith("from_worktree")
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy()
  })

  it("still explains an empty project instead of dead-ending", () => {
    seed([])
    render(<WorktreesDialog />)
    expect(screen.getByText(/no worktrees/i)).toBeTruthy()
  })
})
