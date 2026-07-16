// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Mock the store module: `useDux` reads seeded state, and every action the
// dialog can fire is a spy so the test asserts routing without booting the
// real store (which fetches at import).
let mockState: DuxState
const inspectProjectPath = vi.fn()
const browseDir = vi.fn()
const addProject = vi.fn()
const addProjectCheckoutDefault = vi.fn()
const addProjectCreateInitialCommit = vi.fn()
const initProject = vi.fn()
const closeAddProject = vi.fn()
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  inspectProjectPath: (...a: unknown[]) => inspectProjectPath(...a),
  browseDir: (...a: unknown[]) => browseDir(...a),
  addProject: (...a: unknown[]) => addProject(...a),
  addProjectCheckoutDefault: (...a: unknown[]) =>
    addProjectCheckoutDefault(...a),
  addProjectCreateInitialCommit: (...a: unknown[]) =>
    addProjectCreateInitialCommit(...a),
  initProject: (...a: unknown[]) => initProject(...a),
  closeAddProject: () => closeAddProject(),
}))

import { AddProjectDialog } from "@/components/AddProjectDialog"

function seed(overrides: Partial<DuxState> = {}) {
  mockState = {
    addProjectOpen: true,
    addProjectIntent: "add",
    browsePath: "/home/u/notes",
    browseEntries: [
      { path: "/home/u/notes/sub", label: "sub/", is_git_repo: false },
    ],
    browseLoading: false,
    projectPathInspection: null,
    ...overrides,
  } as unknown as DuxState
}

const plainInspection = (path: string) => ({
  path,
  kind: "plain" as const,
  repoRoot: null,
  gitignoreCandidates: ["node_modules"],
  currentBranch: null,
  warning: null,
  hasCommits: false,
  error: null,
  loading: false,
})

afterEach(() => cleanup())
beforeEach(() => vi.clearAllMocks())

describe("AddProjectDialog picker", () => {
  it("keeps the footer disabled until an explicit selection exists", () => {
    // The primary button must never act on wherever the user happens to be
    // standing (the rejected location-driven design).
    seed()
    render(<AddProjectDialog />)
    const buttons = screen.getAllByRole("button")
    const primary = buttons.find((b) => b.textContent === "Add project")!
    expect(primary).toBeTruthy()
    expect(primary.hasAttribute("disabled")).toBe(true)
  })

  it("pins a Use-this-folder row that selects the browse path and fires inspect", () => {
    seed()
    render(<AddProjectDialog />)
    const pinned = screen.getByText("Use this folder").closest("button")!
    fireEvent.click(pinned)
    expect(inspectProjectPath).toHaveBeenCalledWith("/home/u/notes")
    // The selection footer shows the pinned target's path.
    expect(screen.getAllByText("/home/u/notes").length).toBeGreaterThan(0)
  })

  it("renders the pinned row FIRST, above the server-supplied entries", () => {
    seed()
    render(<AddProjectDialog />)
    // The dialog renders in a portal, so query the whole document.
    const rows = Array.from(document.body.querySelectorAll("button")).filter(
      (b) =>
        b.textContent?.includes("Use this folder") ||
        b.textContent?.includes("sub/"),
    )
    expect(rows[0].textContent).toContain("Use this folder")
  })

  it("offers Initialize Repository & Add once inspection resolves plain, and routes to initProject", () => {
    seed({
      projectPathInspection: plainInspection(
        "/home/u/notes",
      ) as unknown as DuxState["projectPathInspection"],
    })
    const { rerender } = render(<AddProjectDialog />)
    fireEvent.click(screen.getByText("Use this folder").closest("button")!)
    rerender(<AddProjectDialog />)

    // The init panel names the candidates (the confirmation surface).
    expect(
      screen.getByText("This folder is not a git repository."),
    ).toBeTruthy()
    expect(screen.getByText(/node_modules/)).toBeTruthy()

    const primary = screen
      .getAllByRole("button")
      .find((b) => b.textContent === "Initialize Repository & Add")!
    expect(primary.hasAttribute("disabled")).toBe(false)
    fireEvent.click(primary)
    expect(initProject).toHaveBeenCalledWith("/home/u/notes", "")
    expect(closeAddProject).toHaveBeenCalled()
  })

  it("blocks a repo subdirectory: disabled footer plus the root-naming panel", () => {
    seed({
      projectPathInspection: {
        ...plainInspection("/home/u/notes"),
        kind: "repo_subdir",
        repoRoot: "/home/u",
        gitignoreCandidates: [],
        hasCommits: true,
      } as unknown as DuxState["projectPathInspection"],
    })
    const { rerender } = render(<AddProjectDialog />)
    fireEvent.click(screen.getByText("Use this folder").closest("button")!)
    rerender(<AddProjectDialog />)

    expect(
      screen.getByText(
        "This folder is inside the git repository at /home/u. Add that repository instead.",
      ),
    ).toBeTruthy()
    const primary = screen
      .getAllByRole("button")
      .find((b) => b.textContent === "Add project")!
    expect(primary.hasAttribute("disabled")).toBe(true)
    fireEvent.click(primary)
    expect(initProject).not.toHaveBeenCalled()
    expect(addProject).not.toHaveBeenCalled()
  })

  it("navigates (not selects) on a plain-folder row", () => {
    seed()
    render(<AddProjectDialog />)
    fireEvent.click(screen.getByText("sub/").closest("button")!)
    expect(browseDir).toHaveBeenCalledWith("/home/u/notes/sub")
    expect(inspectProjectPath).not.toHaveBeenCalled()
  })

  it("shows the init hint only for the init intent", () => {
    seed({ addProjectIntent: "init" })
    const { unmount } = render(<AddProjectDialog />)
    expect(
      screen.getByText(/dux will\s+initialize a git repository/i),
    ).toBeTruthy()
    unmount()

    // The "only" half: the plain add intent must NOT render the hint, or the
    // intent conditional could be dropped (hint rendered unconditionally)
    // without any test noticing.
    seed({ addProjectIntent: "add" })
    render(<AddProjectDialog />)
    expect(
      screen.queryByText(/dux will\s+initialize a git repository/i),
    ).toBeNull()
  })

  it("Escape in the New-folder editor cancels the editor without closing the dialog", () => {
    // base-ui's dialog dismiss listens for Escape at the document level and
    // ignores defaultPrevented; without stopPropagation the Escape meant for
    // the inline editor dismisses the whole picker.
    seed()
    render(<AddProjectDialog />)
    fireEvent.click(screen.getByRole("button", { name: /new folder/i }))
    const input = screen.getByPlaceholderText("Folder name")
    fireEvent.change(input, { target: { value: "drafts" } })
    fireEvent.keyDown(input, { key: "Escape" })

    expect(closeAddProject).not.toHaveBeenCalled()
    expect(screen.queryByPlaceholderText("Folder name")).toBeNull()
    expect(screen.getByRole("button", { name: /new folder/i })).toBeTruthy()
  })
})
