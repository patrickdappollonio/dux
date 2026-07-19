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

function seed(intent: Intent = "new") {
  mockState = {
    newAgentPickerOpen: true,
    newAgentPickerIntent: intent,
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
    expect(openAttachWorktree).toHaveBeenCalledWith("p1")
    expect(openCreateAgent).not.toHaveBeenCalled()
    expect(closeNewAgentPicker).toHaveBeenCalledTimes(1)
  })

  it("gives the results list a fixed height so the modal does not resize as you type", () => {
    // Content-shift fix: the scroll region is a fixed h-72 (not max-h-72), so the
    // modal occupies the same space at 0, 1, or many results.
    seed("new")
    const { container } = render(<NewAgentPickerDialog />)
    expect(container.ownerDocument.querySelector(".h-72")).not.toBeNull()
    expect(container.ownerDocument.querySelector(".max-h-72")).toBeNull()
  })
})
