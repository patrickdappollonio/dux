// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ProjectShortcutListView } from "@/lib/types"

// The store is mocked: `useDux` reads seeded state and the actions are spies,
// so the test asserts what the dialog does with the rows the server sent.
let mockState: DuxState
// What the store's browse answers: the folder landed in, or null when the
// browse failed or was replaced.
let browseLands = true
const browseDir = vi.fn((path: string) =>
  Promise.resolve(browseLands ? path : null),
)
const createStandaloneAgent = vi.fn()
const closeStandaloneAgentPicker = vi.fn()
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  browseDir: (path: string) => browseDir(path),
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
beforeEach(() => {
  vi.clearAllMocks()
  browseLands = true
})

const createButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: "Create agent" })

// Click a shortcut and let the browse it started answer.
async function pick(row: HTMLButtonElement) {
  await act(async () => {
    fireEvent.click(row)
  })
}

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

  it("moves the explorer to a picked project's folder and targets it", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await pick(shortcutRows()[0])
    expect(browseDir).toHaveBeenCalledWith("/home/u/code/site")
    expect(shortcutRows()[0].dataset.picked).toBe("")
    // The folder is now the target, exactly as the pinned row would make it.
    fireEvent.click(createButton())
    expect(createStandaloneAgent).toHaveBeenCalledWith("/home/u/code/site", "")
  })

  it("brings the name field into view once a folder is chosen, without taking focus", async () => {
    const scrolled: Element[] = []
    const original = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this)
    }
    try {
      seed(SHORTCUTS)
      render(<StandaloneAgentDialog />)
      expect(scrolled).toEqual([])
      await pick(shortcutRows()[0])
      const nameBlock = document.querySelector("[data-standalone-name]")
      expect(nameBlock).not.toBeNull()
      expect(scrolled).toEqual([nameBlock])
      // The name field is revealed, not focused: on a phone, focus would
      // raise the keyboard.
      expect(nameBlock!.contains(document.activeElement)).toBe(false)
    } finally {
      Element.prototype.scrollIntoView = original
    }
  })

  it("reveals the name field again when its block changes size, and when the project list arrives late", async () => {
    const scrolled: Element[] = []
    const originalScroll = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this)
    }
    // jsdom has no ResizeObserver; this one hands the test its callback.
    const observers: (() => void)[] = []
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          observers.push(callback)
        }
        observe() {}
        disconnect() {}
      },
    )
    try {
      // A folder is chosen in the explorer before the project list answers.
      seed(null)
      const view = render(<StandaloneAgentDialog />)
      fireEvent.click(screen.getByRole("button", { name: /Run an agent here/ }))
      expect(scrolled).toHaveLength(1)

      // The observer's opening report is not a change; a later one is.
      const report = observers[observers.length - 1]
      report()
      expect(scrolled).toHaveLength(1)
      report()
      expect(scrolled).toHaveLength(2)

      // The project list lands above the explorer and pushes the field down.
      seed(SHORTCUTS)
      view.rerender(<StandaloneAgentDialog />)
      expect(shortcutRows()).toHaveLength(2)
      expect(scrolled).toHaveLength(3)
    } finally {
      Element.prototype.scrollIntoView = originalScroll
      vi.unstubAllGlobals()
    }
  })

  it("targets nothing when the explorer never reached the project's folder", async () => {
    seed(SHORTCUTS)
    browseLands = false
    render(<StandaloneAgentDialog />)
    await pick(shortcutRows()[0])
    expect(browseDir).toHaveBeenCalledWith("/home/u/code/site")
    expect(createButton().disabled).toBe(true)
    expect(shortcutRows()[0].dataset.picked).toBeUndefined()
  })

  it("keeps the footer outside the part that scrolls", () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    const body = document.querySelector("[data-standalone-picker-body]")!
    expect(body.className).toContain("overflow-y-auto")
    expect(body.contains(createButton())).toBe(false)
    expect(body.contains(shortcutRows()[0])).toBe(true)
  })

  it("does not let a greyed-out row be picked", () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    const blocked = shortcutRows()[1]
    expect(blocked.disabled).toBe(true)
    fireEvent.click(blocked)
    expect(browseDir).not.toHaveBeenCalled()
    expect(createButton().disabled).toBe(true)
  })

  it("draws no list when there are no projects", () => {
    seed({ caption: "Caption from the server.", rows: [] })
    render(<StandaloneAgentDialog />)
    expect(shortcutRows()).toEqual([])
    expect(screen.queryByText("Your projects")).toBeNull()
    expect(screen.queryByText("Caption from the server.")).toBeNull()
  })
})
