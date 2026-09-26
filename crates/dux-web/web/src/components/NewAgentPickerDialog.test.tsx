// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` so the picker reads our seeded state, and replace the store
// actions each project row dispatches with spies so we can assert exactly which
// hand-off the click fires (and that the "new" intent does NOT create an agent
// straight from the picker). The rest of the real store exports stay intact.
let mockState: DuxState
const openCreateAgent = vi.fn()
const openCreateAgentFromPr = vi.fn()
const openAttachWorktree = vi.fn()
const openAddProject = vi.fn()
const closeNewAgentPicker = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openCreateAgent: (...args: unknown[]) => openCreateAgent(...args),
    openCreateAgentFromPr: (...args: unknown[]) => openCreateAgentFromPr(...args),
    openAttachWorktree: (...args: unknown[]) => openAttachWorktree(...args),
    openAddProject: (...args: unknown[]) => openAddProject(...args),
    closeNewAgentPicker: (...args: unknown[]) => closeNewAgentPicker(...args),
  }
})

// ProjectMenuItems only mounts inside the (closed) row ⋯ menu, but it reads the
// store on import; keep the tests focused by rendering nothing for it.
// The props it was last rendered with are kept, so a test can check what the
// picker hands its row menus.
let menuItemsProps: { id: string; onLeave?: () => void } | null = null
vi.mock("@/components/ProjectMenuItems", () => ({
  ProjectMenuItems: (props: { id: string; onLeave?: () => void }) => {
    menuItemsProps = props
    return null
  },
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

const { NewAgentPickerDialog } = await import("./NewAgentPickerDialog")

type Intent = DuxState["newAgentPickerIntent"]

function seed(intent: Intent = "new", extra: Partial<DuxState> = {}) {
  mockState = {
    newAgentPickerOpen: true,
    newAgentPickerIntent: intent,
    ...extra,
    spine: {
      projects: [
        { id: "p1", name: "acme", default_provider: "claude" },
        { id: "p2", name: "beta", default_provider: "codex" },
      ],
      sessions: [],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  vi.clearAllMocks()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("NewAgentPickerDialog", () => {
  // What the user typed is a value, so the empty state shows it as the shared
  // chip rather than in quotes.
  it("names the search that matched nothing as a chip", () => {
    seed("new")
    render(<NewAgentPickerDialog />)
    fireEvent.change(screen.getByLabelText("Search projects"), {
      target: { value: "zzz" },
    })
    const empty = screen.getByText(/No projects match/)
    expect(empty.textContent).toBe("No projects match zzz.")
    expect(screen.getByText("zzz", { selector: "code" })).toBeTruthy()
  })

  // New agent… and New terminal from a row's menu take the user somewhere
  // else, so the picker must not stay open over where they land.
  it("closes the picker before a row menu item that leaves it", async () => {
    seed("new")
    menuItemsProps = null
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getAllByLabelText("Project actions")[0])
    await waitFor(() => expect(menuItemsProps).not.toBeNull())
    expect(menuItemsProps!.onLeave).toBeTypeOf("function")
    menuItemsProps!.onLeave!()
    expect(closeNewAgentPicker).toHaveBeenCalledTimes(1)
  })

  it("opens the create-agent name dialog for the clicked project in the new intent", () => {
    seed("new")
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getByRole("button", { name: /acme/ }))
    // The row hands off to the shared name dialog (which honors the pet-name /
    // copy-changes config); it must NOT create the agent directly here.
    expect(openCreateAgent).toHaveBeenCalledTimes(1)
    expect(openCreateAgent).toHaveBeenCalledWith("p1")
    expect(closeNewAgentPicker).toHaveBeenCalledTimes(1)
  })

  it("does not render a provider selector or a Create button in the new intent", () => {
    seed("new")
    render(<NewAgentPickerDialog />)
    // Provider is chosen post-creation via the agent ⋯ menu, so no provider pills
    // and no in-picker Create button remain.
    expect(screen.queryByRole("button", { name: "Create agent" })).toBeNull()
    expect(screen.queryByRole("button", { name: "claude" })).toBeNull()
    expect(screen.queryByRole("button", { name: "codex" })).toBeNull()
  })

  it("hands off to the from-PR dialog on row click in the from_pr intent", () => {
    seed("from_pr")
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getByRole("button", { name: /beta/ }))
    expect(openCreateAgentFromPr).toHaveBeenCalledWith("p2")
    expect(openCreateAgent).not.toHaveBeenCalled()
    expect(closeNewAgentPicker).toHaveBeenCalledTimes(1)
  })

  it("hands off to the attach-worktree dialog on row click in the from_worktree intent", () => {
    seed("from_worktree")
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getByRole("button", { name: /acme/ }))
    // `true` marks the drill-down, which is what earns the Worktrees dialog a
    // Back control returning to this list.
    expect(openAttachWorktree).toHaveBeenCalledWith("p1", true)
    expect(openCreateAgent).not.toHaveBeenCalled()
    expect(closeNewAgentPicker).toHaveBeenCalledTimes(1)
  })

  it("labels each project row with its worktree count in the from_worktree intent", () => {
    // The dead end this fixes: drilling into a project only to find nothing.
    // An empty project stays listed and stays clickable, because disabling it
    // gives no reason and reads as broken.
    seed("from_worktree", {
      projectWorktreeCounts: { p1: 3, p2: 0 },
    } as Partial<DuxState>)
    render(<NewAgentPickerDialog />)
    expect(screen.getByRole("button", { name: /acme/ }).textContent).toContain(
      "3 worktrees",
    )
    const beta = screen.getByRole("button", { name: /beta/ })
    expect(beta.textContent).toContain("none")
    expect(beta.hasAttribute("disabled")).toBe(false)
  })

  it("labels rows with agent counts in the other intents", () => {
    // The worktree count replaces the agent count only in the worktree intent;
    // the create flows still care about agents.
    seed("new", { projectWorktreeCounts: { p1: 3 } } as Partial<DuxState>)
    render(<NewAgentPickerDialog />)
    expect(screen.getByRole("button", { name: /acme/ }).textContent).toContain(
      "0 agents",
    )
  })

  it("lists projects most recently touched first", () => {
    // acme is stored second and was added first, but it just received an agent,
    // so it leads the more recently added beta.
    mockState = {
      newAgentPickerOpen: true,
      newAgentPickerIntent: "new",
      spine: {
        projects: [
          {
            id: "p2",
            name: "beta",
            default_provider: "codex",
            created_at: "2026-07-02T09:00:00Z",
          },
          {
            id: "p1",
            name: "acme",
            default_provider: "claude",
            created_at: "2026-07-01T09:00:00Z",
          },
        ],
        sessions: [
          {
            id: "a1",
            workspace: { kind: "managed", project_id: "p1" },
            created_at: "2026-07-05T09:00:00Z",
          },
        ],
      },
    } as unknown as DuxState
    render(<NewAgentPickerDialog />)
    const names = screen
      .getAllByRole("button")
      .map((button) => button.textContent ?? "")
      .filter((text) => text.includes("acme") || text.includes("beta"))
    expect(names[0]).toContain("acme")
    expect(names[1]).toContain("beta")
  })

  it("keeps the recency order inside a narrowed candidate set", () => {
    // A pull-request reference matched acme and beta; gamma is out of the set,
    // and the two that remain lead with the one touched most recently.
    mockState = {
      newAgentPickerOpen: true,
      newAgentPickerIntent: "from_pr",
      newAgentPickerOnlyIds: ["p1", "p2"],
      spine: {
        projects: [
          {
            id: "p1",
            name: "acme",
            default_provider: "claude",
            created_at: "2026-07-01T09:00:00Z",
          },
          {
            id: "p2",
            name: "beta",
            default_provider: "codex",
            created_at: "2026-07-06T09:00:00Z",
          },
          {
            id: "p3",
            name: "gamma",
            default_provider: "claude",
            created_at: "2026-07-20T09:00:00Z",
          },
        ],
        sessions: [],
      },
    } as unknown as DuxState
    render(<NewAgentPickerDialog />)
    const names = screen
      .getAllByRole("button")
      .map((button) => button.textContent ?? "")
      .filter((text) => /acme|beta|gamma/.test(text))
    expect(names).toHaveLength(2)
    expect(names[0]).toContain("beta")
    expect(names[1]).toContain("acme")
  })

  // The order is snapshotted at open, so a spine update while the picker is up
  // cannot slide a row out from under the pointer.
  describe("the order frozen at open", () => {
    function projectRows(): string[] {
      return screen
        .getAllByRole("button")
        .map((button) => button.textContent ?? "")
        .filter((text) => /acme|beta|gamma/.test(text))
    }

    function stateWith(
      projects: unknown[],
      sessions: unknown[] = [],
    ): DuxState {
      return {
        newAgentPickerOpen: true,
        newAgentPickerIntent: "new",
        spine: { projects, sessions },
      } as unknown as DuxState
    }

    const acme = {
      id: "p1",
      name: "acme",
      default_provider: "claude",
      created_at: "2026-07-01T09:00:00Z",
    }
    const beta = {
      id: "p2",
      name: "beta",
      default_provider: "codex",
      created_at: "2026-07-02T09:00:00Z",
    }
    const agentOnAcme = {
      id: "a1",
      workspace: { kind: "managed", project_id: "p1" },
      created_at: "2026-07-05T09:00:00Z",
    }

    it("does not move a row when a newer agent would re-sort the list", () => {
      mockState = stateWith([beta, acme], [agentOnAcme])
      const { rerender } = render(<NewAgentPickerDialog />)
      expect(projectRows()[0]).toContain("acme")

      // beta now holds the newest agent, so a live sort would lift it.
      mockState = stateWith(
        [beta, acme],
        [
          agentOnAcme,
          {
            id: "a2",
            workspace: { kind: "managed", project_id: "p2" },
            created_at: "2026-07-09T09:00:00Z",
          },
        ],
      )
      rerender(<NewAgentPickerDialog />)

      const rows = projectRows()
      expect(rows[0]).toContain("acme")
      expect(rows[1]).toContain("beta")
    })

    it("drops a project that is removed while the picker is open", () => {
      mockState = stateWith([beta, acme], [agentOnAcme])
      const { rerender } = render(<NewAgentPickerDialog />)

      mockState = stateWith([beta], [])
      rerender(<NewAgentPickerDialog />)

      expect(projectRows().map((text) => text.replace(/\s+/g, " "))).toHaveLength(
        1,
      )
      expect(projectRows()[0]).toContain("beta")
    })

    it("puts a project added while the picker is open at the bottom", () => {
      mockState = stateWith([beta, acme], [agentOnAcme])
      const { rerender } = render(<NewAgentPickerDialog />)

      // Freshly added, so a live sort would put it first.
      mockState = stateWith(
        [
          beta,
          acme,
          {
            id: "p3",
            name: "gamma",
            default_provider: "claude",
            created_at: "2026-07-20T09:00:00Z",
          },
        ],
        [agentOnAcme],
      )
      rerender(<NewAgentPickerDialog />)

      const rows = projectRows()
      expect(rows[0]).toContain("acme")
      expect(rows[1]).toContain("beta")
      expect(rows[2]).toContain("gamma")
    })
  })

  it("gives the results list a fixed height so the modal does not resize as you type", () => {
    // Content-shift fix: the scroll region is a fixed h-72 (not max-h-72), so the
    // modal occupies the same space at 0, 1, or many results.
    seed("new")
    const { container } = render(<NewAgentPickerDialog />)
    expect(container.ownerDocument.querySelector(".h-72")).not.toBeNull()
    expect(container.ownerDocument.querySelector(".max-h-72")).toBeNull()
  })

  it("shrinks the list instead of clipping when the popup hits its viewport cap", () => {
    // Phone-keyboard regression pin (the "cannot scroll the New Agent modal
    // with a finger" bug): the popup must NOT be overflow-hidden with a rigid
    // inner list. The idiom is a flex column whose list is the one shrinkable
    // child, so when the soft keyboard shrinks the popup's dvh cap the list
    // gives up height and the header + Add-project footer stay reachable; the
    // popup keeps its base overflow-y-auto as the last-resort scroll.
    seed("new")
    const { container } = render(<NewAgentPickerDialog />)
    const doc = container.ownerDocument
    const popup = doc.querySelector('[data-slot="dialog-content"]')
    expect(popup).not.toBeNull()
    expect(popup!.className).toContain("flex-col")
    expect(popup!.className).not.toContain("overflow-hidden")
    const list = doc.querySelector('[data-slot="scroll-area"]')
    expect(list).not.toBeNull()
    expect(list!.classList.contains("shrink")).toBe(true)
    expect(list!.classList.contains("shrink-0")).toBe(false)
    expect(list!.classList.contains("min-h-0")).toBe(true)
    // The list must be the ONLY child that gives way: header and footer pin
    // their height so shrinking cannot crush the controls themselves.
    const header = doc.querySelector('[data-slot="dialog-header"]')
    expect(header!.className).toContain("shrink-0")
    const footerButton = [...doc.querySelectorAll("button")].find((b) =>
      b.textContent!.includes("Add a new project"),
    )
    expect(footerButton!.parentElement!.className).toContain("shrink-0")
  })
})
