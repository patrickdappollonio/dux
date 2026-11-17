// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ProjectShortcutListView } from "@/lib/types"

// The store is mocked: `useDux` reads seeded state and the actions are spies,
// so the test asserts what the dialog does with the rows the server sent.
let mockState: DuxState
const browseDir = vi.fn()
const createStandaloneAgent = vi.fn()
const closeStandaloneAgentPicker = vi.fn()
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  browseDir: (...a: unknown[]) => browseDir(...a),
  createStandaloneAgent: (...a: unknown[]) => createStandaloneAgent(...a),
  closeStandaloneAgentPicker: () => closeStandaloneAgentPicker(),
}))

import { StandaloneAgentDialog } from "@/components/StandaloneAgentDialog"

const SHORTCUTS: ProjectShortcutListView = {
  caption: "Caption from the server.",
  rows: [
    {
      project_id: "p-site",
      name: "site",
      folder_label: "~/code/site",
      path: "/home/u/code/site",
      state: "available",
      reason: null,
    },
    {
      project_id: "p-api",
      name: "api",
      folder_label: "~/code/api",
      path: "/home/u/code/api",
      state: "already_has_standalone_agent",
      reason: "reason from the server",
    },
  ],
}

function seed(shortcuts: ProjectShortcutListView | null) {
  mockState = {
    standaloneAgentPickerOpen: true,
    standaloneProjectShortcuts: shortcuts,
    browsePath: "/home/u",
    browseEntries: [],
    browseLoading: false,
  } as unknown as DuxState
}

const shortcutRows = () =>
  Array.from(
    document.querySelectorAll<HTMLButtonElement>("[data-project-shortcut]"),
  )

afterEach(() => cleanup())
beforeEach(() => vi.clearAllMocks())

describe("StandaloneAgentDialog project shortcuts", () => {
  it("renders the server's rows in the server's order and words", () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    const rows = shortcutRows()
    expect(rows.map((row) => row.dataset.projectShortcut)).toEqual([
      "p-site",
      "p-api",
    ])
    // The folder label is the server's, never derived from the path here.
    expect(rows[0].textContent).toContain("~/code/site")
    expect(rows[1].textContent).toContain("reason from the server")
    expect(screen.getByText("Caption from the server.")).toBeTruthy()
  })

  it("moves the explorer to a picked project's folder and targets it", () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    fireEvent.click(shortcutRows()[0])
    expect(browseDir).toHaveBeenCalledWith("/home/u/code/site")
    // The folder is now the target, exactly as the pinned row would make it.
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }))
    expect(createStandaloneAgent).toHaveBeenCalledWith("/home/u/code/site", "")
  })

  it("does not let a greyed-out row be picked", () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    const blocked = shortcutRows()[1]
    expect(blocked.disabled).toBe(true)
    fireEvent.click(blocked)
    expect(browseDir).not.toHaveBeenCalled()
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Create agent" })
        .disabled,
    ).toBe(true)
  })

  it("draws no list when there are no projects", () => {
    seed({ caption: "Caption from the server.", rows: [] })
    render(<StandaloneAgentDialog />)
    expect(shortcutRows()).toEqual([])
    expect(screen.queryByText("Your projects")).toBeNull()
    expect(screen.queryByText("Caption from the server.")).toBeNull()
  })
})
