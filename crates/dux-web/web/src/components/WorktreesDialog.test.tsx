// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import type { DuxState } from "@/lib/store"
import type { LeftoverFolderView, ProjectWorktreeEntryView } from "@/lib/types"

// Override only `useDux` so the dialog reads our seeded state, while the real
// store exports (closeAttachWorktree, attachWorktree) stay intact.
let mockState: DuxState
const openNewAgentPicker = vi.fn()
const deleteProjectWorktree = vi.fn()
const deleteLeftoverFolder = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openNewAgentPicker: (...args: unknown[]) => openNewAgentPicker(...args),
    deleteProjectWorktree: (...args: unknown[]) => deleteProjectWorktree(...args),
    deleteLeftoverFolder: (...args: unknown[]) => deleteLeftoverFolder(...args),
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
    branch_name: LONG_BRANCH,
    worktree_path: LONG_PATH,
    branch: LONG_BRANCH,
    adoptable: true,
    reason: null,
    dirty: false,
    agent_id: null,
    being_removed: false,
    in_use: null,
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
    attachWorktreeLeftovers: [],
    attachWorktreeLoading: false,
    attachWorktreeFromPicker: false,
    deleteWorktreeTarget: null,
    deleteLeftoverTarget: null,
    spine: {
      projects: [{ id: "p1", name: "acme" }],
      sessions: [
        {
          id: "s1",
          title: "tidy-otter",
          workspace: { kind: "managed", project_id: "p1", branch_name: "held", initial_branch: "held", branch_provenance: "created", source_branch: "main", worktree_path: "/wt/held" },
        },
      ],
    },
    ...extra,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  openNewAgentPicker.mockClear()
  deleteProjectWorktree.mockClear()
  deleteLeftoverFolder.mockClear()
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
    expect(screen.getByRole("heading").textContent).toBe("Worktrees in acme")
    expect(screen.getByText("acme", { selector: "code" })).toBeTruthy()
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
        branch_name: "held",
        worktree_path: "/wt/held",
        adoptable: false,
        agent_id: "s1",
        reason: "Already has an agent.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.getAllByRole("button", { name: "Worktree actions" }).length).toBe(1)
    // And the attached row names the agent holding it.
    // The agent after the "Held by" label is a name, so it is the shared chip.
    const held = screen.getByText("tidy-otter", { selector: "code" })
    expect(held.parentElement?.textContent).toBe("Held by tidy-otter")
  })

  it("files a worktree its own agent holds under its agent, with no in-use line", () => {
    // The agent that owns a worktree is not something else using it.
    seed([
      entry({
        branch_name: "held",
        worktree_path: "/wt/held",
        adoptable: false,
        agent_id: "s1",
        reason: "Already has an agent.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.getByText("Already has an agent")).toBeTruthy()
    expect(screen.queryByText("In use")).toBeNull()
    expect(screen.queryByText(/^In use:/)).toBeNull()
  })

  it("says what else is using a worktree its agent holds, still under its agent", () => {
    seed([
      entry({
        branch_name: "held",
        worktree_path: "/wt/held",
        adoptable: false,
        agent_id: "s1",
        reason: "Already has an agent. In use: a terminal open in it.",
        in_use: "In use: a terminal open in it.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.getByText("Already has an agent")).toBeTruthy()
    // No section of its own: the agent holding it is the way to remove it.
    expect(screen.queryByText("In use")).toBeNull()
    const held = screen.getByText("tidy-otter", { selector: "code" })
    expect(held.parentElement?.textContent).toBe("Held by tidy-otter")
    expect(screen.getByText("In use: a terminal open in it.")).toBeTruthy()
  })

  it("labels a worktree whose agent was just deleted as being removed, with no delete", () => {
    // Its agent's delete is still letting the agent exit, so the folder is
    // neither free nor held: it is already going, and cannot go twice.
    seed([
      entry({ worktree_path: "/wt/free", branch_name: "free" }),
      entry({
        branch_name: "going",
        worktree_path: "/wt/going",
        adoptable: false,
        being_removed: true,
        reason: "Being removed: its agent was just deleted.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.getAllByRole("button", { name: "Worktree actions" }).length).toBe(1)
    expect(screen.getByText("Being removed")).toBeTruthy()
    expect(screen.queryByText("Already has an agent")).toBeNull()
  })

  it("REVIEW: does not file a worktree nothing but a stopping process holds under an agent", () => {
    // The server marks a folder busy (not adoptable, no agent, not being
    // removed) when something dux started is still running there, such as
    // the CLI of an agent deleted a moment ago with its worktree kept. No
    // agent holds it, so telling the user to "delete the agent holding it"
    // points at an agent that does not exist.
    seed([
      entry({
        branch_name: "busy",
        worktree_path: "/wt/busy",
        adoptable: false,
        agent_id: null,
        being_removed: false,
        reason: "In use: a process dux started there that is still stopping.",
      }),
    ])
    render(<WorktreesDialog />)
    expect(screen.queryByText("Already has an agent")).toBeNull()
    expect(
      screen.queryByText("To remove one of these, delete the agent holding it."),
    ).toBeNull()
    // It sits under its own heading, with what is using it on the row.
    expect(screen.getByText("In use")).toBeTruthy()
    expect(
      screen.getAllByText("In use: a process dux started there that is still stopping.")
        .length,
    ).toBeGreaterThan(0)
    expect(screen.queryByRole("button", { name: "Worktree actions" })).toBeNull()
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

  // The words, pinned.
  //
  // The TUI's worktree manager says the same sentences and pins them in its own
  // suite (`the_worktree_removal_copy_is_the_web_dialogs_copy` in render.rs).
  // The two dialogs are parallel implementations of one piece of copy, and
  // nothing but a test on each side keeps them in step: the TUI's drifted away
  // from this one immediately, silently, in the change that introduced it.
  it("says the sentences the TUI's worktree manager says", () => {
    const target = entry({
      worktree_path: "/wt/messy",
      branch_name: "messy",
      branch: "messy",
      dirty: true,
    })
    seed([target], {
      deleteWorktreeTarget: { projectId: "p1", entry: target },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)

    expect(
      screen.getByRole("heading", {
        name: "Delete the worktree for messy?",
        hidden: true,
      }),
    ).toBeInstanceOf(HTMLElement)
    expect(
      screen.getAllByText("messy", { selector: "code" }).length,
    ).toBeGreaterThan(0)
    const confirm = screen.getByTestId("delete-worktree-confirm")
    expect(confirm.textContent).toContain(
      "will be removed from disk. This action cannot be undone: dux has no trash and removes the directory forcibly.",
    )
    expect(confirm.textContent).toContain(
      "This worktree has uncommitted changes, and they go with it. Nothing in there that is not committed exists anywhere else.",
    )
    expect(confirm.textContent).toContain(
      "will be deleted with it, forcibly. Any commits on it that are not merged anywhere else go too.",
    )
    expect(
      screen.getByRole("button", { name: "Delete worktree", hidden: true }),
    ).toBeInstanceOf(HTMLElement)
  })

  it("says there is no choice to make for a detached worktree", () => {
    const target = entry({
      worktree_path: "/wt/loose",
      branch_name: "detached 1a2b3c4",
      branch: null,
    })
    seed([target], {
      deleteWorktreeTarget: { projectId: "p1", entry: target },
    } as unknown as Partial<DuxState>)
    render(<WorktreesDialog />)
    expect(
      screen.getByTestId("delete-worktree-confirm").textContent,
    ).toContain(
      "This worktree is not on a branch, so there is no branch to keep or delete. Only the working directory is removed.",
    )
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

  describe("left-over folders", () => {
    function leftover(
      overrides: Partial<LeftoverFolderView> = {},
    ): LeftoverFolderView {
      return {
        path: "/wt/acme/stuck",
        branches: ["feat"],
        undeletable: [],
        branch_provenance: "created",
        left_by: "agent",
        offers_delete: true,
        in_use: null,
        folder_present: true,
        ...overrides,
      }
    }

    async function openRowMenu() {
      fireEvent.click(
        screen.getByRole("button", { name: "Left-over folder actions" }),
      )
      await screen.findByRole("menu")
    }

    function menuLabels(): string[] {
      return screen.getAllByRole("menuitem").map((item) => item.textContent ?? "")
    }

    it("lists a left-over folder with its kept branches as chips", async () => {
      seed([], {
        attachWorktreeLeftovers: [leftover({ branches: ["feat", "feat-old"] })],
      } as Partial<DuxState>)
      render(<WorktreesDialog />)
      expect(screen.getByText("Left over after a delete")).toBeTruthy()
      expect(screen.getByText("stuck")).toBeTruthy()
      expect(screen.getByText("/wt/acme/stuck")).toBeTruthy()
      const kept = screen.getByText("feat", { selector: "code" })
      expect(kept.parentElement?.textContent).toBe("Kept branches feat and feat-old")
      // A project whose only rows are left-over folders is not empty.
      expect(screen.queryByText(/no worktrees/i)).toBeNull()
      await openRowMenu()
      expect(menuLabels()).toEqual(["Copy path", "Delete folder…"])
    })

    it("offers no delete while something is using the folder", async () => {
      seed([], {
        attachWorktreeLeftovers: [
          leftover({ in_use: "In use: a terminal open in it." }),
        ],
      } as Partial<DuxState>)
      render(<WorktreesDialog />)
      expect(screen.getByText("In use: a terminal open in it.")).toBeTruthy()
      await openRowMenu()
      expect(menuLabels()).toEqual(["Copy path"])
    })

    it("says a row whose folder is gone is about its kept branch", async () => {
      seed([], {
        attachWorktreeLeftovers: [leftover({ folder_present: false })],
      } as Partial<DuxState>)
      render(<WorktreesDialog />)
      expect(screen.getByText("Folder already removed; branch kept")).toBeTruthy()
      await openRowMenu()
      expect(menuLabels()).toEqual(["Copy path", "Delete branch…"])
    })

    it("offers only Copy path for a branch dux will not delete, and says why", async () => {
      seed([], {
        attachWorktreeLeftovers: [
          leftover({
            folder_present: false,
            branches: [],
            undeletable: [{ name: "feat", why: "moved" }],
            offers_delete: false,
          }),
        ],
      } as Partial<DuxState>)
      render(<WorktreesDialog />)
      expect(
        screen.getByTestId("leftover-kept-branch").textContent,
      ).toBe(
        "The branch feat is kept: it has moved since the folder was left behind. Delete it yourself with git branch -D 'feat'.",
      )
      await openRowMenu()
      expect(menuLabels()).toEqual(["Copy path"])
    })

    function confirmFor(
      folder: LeftoverFolderView,
      listed: LeftoverFolderView = folder,
    ) {
      seed([], {
        attachWorktreeLeftovers: [listed],
        deleteLeftoverTarget: { projectId: "p1", folder },
      } as Partial<DuxState>)
      render(<WorktreesDialog />)
    }

    const branchBox = () =>
      screen.getByRole("checkbox", { name: /also delete the branch/i, hidden: true })
    const confirmButton = () =>
      screen.getByRole("button", { name: "Delete folder", hidden: true })

    it.each(["created", null] as const)(
      "ticks the branch box for a %s branch and sends it with the delete",
      (provenance) => {
        confirmFor(leftover({ branch_provenance: provenance }))
        expect(branchBox().getAttribute("aria-checked")).toBe("true")
        expect(screen.queryByTestId("delete-leftover-branch-warning")).toBeNull()
        fireEvent.click(confirmButton())
        expect(deleteLeftoverFolder).toHaveBeenCalledWith(
          "p1",
          "/wt/acme/stuck",
          ["feat"],
        )
      },
    )

    it("leaves a branch that predates its agent unticked, warns, and keeps it", () => {
      confirmFor(leftover({ branch_provenance: "attached" }))
      expect(branchBox().getAttribute("aria-checked")).toBe("false")
      expect(
        screen.getByTestId("delete-leftover-branch-warning").textContent,
      ).toBe("This branch existed before the agent.")
      fireEvent.click(confirmButton())
      expect(deleteLeftoverFolder).toHaveBeenCalledWith("p1", "/wt/acme/stuck", [])
    })

    it("says what is lost, and offers no branch box when no branch was kept", () => {
      confirmFor(leftover({ branches: [] }))
      expect(screen.getByTestId("delete-leftover-confirm").textContent).toBe(
        "/wt/acme/stuck and everything in it will be deleted from disk. Something was still writing into it when its agent was deleted; stop it first, or the delete fails again. This cannot be undone: dux has no trash.",
      )
      expect(screen.queryByRole("checkbox", { hidden: true })).toBeNull()
      fireEvent.click(confirmButton())
      expect(deleteLeftoverFolder).toHaveBeenCalledWith("p1", "/wt/acme/stuck", [])
    })

    const fate = () =>
      screen.getByTestId("delete-leftover-branch-fate").textContent

    it("says the branch's fate the way the box is ticked", () => {
      confirmFor(leftover())
      expect(fate()).toBe(
        "The branch feat will be deleted with it, unless it has moved since the folder was left behind.",
      )
      fireEvent.click(branchBox())
      expect(fate()).toBe("The branch feat is kept.")
    })

    it("deletes the kept branches when the folder is already gone", () => {
      confirmFor(
        leftover({
          folder_present: false,
          branches: ["feat", "feat-old"],
          branch_provenance: "attached",
        }),
      )
      expect(
        screen.getByRole("heading", { name: "Delete the kept branches?", hidden: true }),
      ).toBeTruthy()
      expect(screen.getByTestId("delete-leftover-confirm").textContent).toContain(
        "The left-over folder at /wt/acme/stuck is already gone, so nothing is deleted from disk.",
      )
      expect(fate()).toBe(
        "The branches feat and feat-old will be deleted, unless they have moved since the folder was left behind.",
      )
      expect(screen.getByTestId("delete-leftover-branch-warning").textContent).toBe(
        "feat-old existed before the agent.",
      )
      // The delete IS the branches, so there is no box to untick.
      expect(screen.queryByRole("checkbox", { hidden: true })).toBeNull()
      fireEvent.click(
        screen.getByRole("button", { name: "Delete branches", hidden: true }),
      )
      expect(deleteLeftoverFolder).toHaveBeenCalledWith("p1", "/wt/acme/stuck", [
        "feat",
        "feat-old",
      ])
    })

    it("offers the box for the branch it can delete and says why it keeps the other", () => {
      confirmFor(
        leftover({
          undeletable: [
            { name: "feat-old", why: "checked_out", path: "/wt/acme/other" },
          ],
        }),
      )
      expect(
        screen.getByRole("checkbox", { name: "Also delete the branch feat", hidden: true }),
      ).toBeTruthy()
      expect(fate()).toBe(
        "The branch feat will be deleted with it, unless it has moved since the folder was left behind.",
      )
      expect(screen.getByTestId("delete-leftover-kept-branch").textContent).toBe(
        "The branch feat-old is kept: it is checked out in /wt/acme/other. Delete it yourself with git branch -D 'feat-old' once it is not.",
      )
      fireEvent.click(confirmButton())
      expect(deleteLeftoverFolder).toHaveBeenCalledWith("p1", "/wt/acme/stuck", ["feat"])
    })

    it("says the worktree manager's removal left the folder, by what left it", () => {
      confirmFor(leftover({ left_by: "manager" }))
      expect(screen.getByTestId("delete-leftover-confirm").textContent).toContain(
        "when its worktree was removed;",
      )
    })

    it.each([
      ["its kept branches changed", { branches: ["feat", "feat-old"] }],
      ["its folder went", { folder_present: false }],
      [
        "it keeps a branch it did not",
        { undeletable: [{ name: "feat-old", why: "moved" }] },
      ],
      ["it no longer offers a delete", { offers_delete: false }],
    ] as const)("closes when the row it was opened on changed: %s", (_why, change) => {
      confirmFor(leftover(), leftover(change))
      expect(screen.queryByTestId("delete-leftover-confirm")).toBeNull()
    })

    it("closes when a branch it keeps is kept for another reason", () => {
      confirmFor(
        leftover({ undeletable: [{ name: "feat-old", why: "no_commit" }] }),
        leftover({ undeletable: [{ name: "feat-old", why: "moved" }] }),
      )
      expect(screen.queryByTestId("delete-leftover-confirm")).toBeNull()
    })
  })
})
