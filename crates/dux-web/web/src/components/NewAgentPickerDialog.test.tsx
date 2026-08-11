// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

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
vi.mock("@/components/ProjectMenuItems", () => ({
  ProjectMenuItems: () => null,
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
