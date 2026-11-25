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
const createStandaloneAgentInNewFolder = vi.fn()
const closeStandaloneAgentPicker = vi.fn()
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  browseDir: (path: string) => browseDir(path),
  createStandaloneAgent: (...a: unknown[]) => createStandaloneAgent(...a),
  createStandaloneAgentInNewFolder: (...a: unknown[]) => createStandaloneAgentInNewFolder(...a),
  closeStandaloneAgentPicker: () => closeStandaloneAgentPicker(),
}))

// The generated name and the folder answers the name step asks the server for.
const agentName = vi.fn(() => Promise.resolve({ name: "brave-otter" }))
vi.mock("@/lib/browseApi", () => ({
  browseApi: { agentName: () => agentName() },
}))
let takenNames: string[] = []
let recordedAgents: Record<string, string> = {}
const newAgentFolder = vi.fn((name: string) =>
  Promise.resolve({
    root_label: "~/.config/dux/agents",
    folder_label: name === "" ? null : `~/.config/dux/agents/${name}`,
    taken: takenNames.includes(name) || name in recordedAgents,
    taken_by_agent: recordedAgents[name] ?? null,
  }),
)
vi.mock("@/lib/sessionsApi", () => ({
  sessionsApi: { newAgentFolder: (name: string) => newAgentFolder(name) },
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

function seed(shortcuts: ProjectShortcutListView | null, projects = 2) {
  mockState = {
    spine: { projects: Array.from({ length: projects }, (_, i) => ({ id: `p-${i}` })) },
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
  takenNames = []
  recordedAgents = {}
})

// Open the dialog and go to its existing-folder step.
function renderOnFolderStep() {
  const view = render(<StandaloneAgentDialog />)
  fireEvent.click(screen.getByRole("button", { name: "Use an existing folder or project…" }))
  return view
}

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
    renderOnFolderStep()
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
    renderOnFolderStep()
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
      renderOnFolderStep()
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
      const view = renderOnFolderStep()
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
    renderOnFolderStep()
    await pick(shortcutRows()[0])
    expect(browseDir).toHaveBeenCalledWith("/home/u/code/site")
    expect(createButton().disabled).toBe(true)
    expect(shortcutRows()[0].dataset.picked).toBeUndefined()
  })

  it("keeps the footer outside the part that scrolls", () => {
    seed(SHORTCUTS)
    renderOnFolderStep()
    const body = document.querySelector('[data-slot="modal-body"]')!
    expect(body.className).toContain("overflow-y-auto")
    expect(body.contains(createButton())).toBe(false)
    expect(body.contains(shortcutRows()[0])).toBe(true)
  })

  // The phone report this pins: Cancel and Create agent stacked as two
  // full-width rows, taking as much of the screen as the folder list.
  it("covers a phone's screen with Cancel and Create agent on one row", () => {
    vi.stubGlobal("innerWidth", 390)
    try {
      seed(SHORTCUTS)
      renderOnFolderStep()
      const popup = document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!
      expect(popup.dataset.presentation).toBe("cover")
      const row = createButton().closest(".grid-cols-2")!
      expect(row.contains(screen.getByRole("button", { name: "Cancel" }))).toBe(true)
      expect(row.querySelector(".col-span-2")).toBeNull()
    } finally {
      vi.unstubAllGlobals()
    }
  })

  it("does not let a greyed-out row be picked", () => {
    seed(SHORTCUTS)
    renderOnFolderStep()
    const blocked = shortcutRows()[1]
    expect(blocked.disabled).toBe(true)
    fireEvent.click(blocked)
    expect(browseDir).not.toHaveBeenCalled()
    expect(createButton().disabled).toBe(true)
  })

  // The space the list will take is kept from the first frame, so the card
  // does not grow under the pointer when the list arrives.
  it("keeps the project list's space while the list is on its way", () => {
    seed(null)
    const view = renderOnFolderStep()
    const pending = document.querySelector("[data-project-shortcuts-pending]")
    // The rows' own height for as many rows as there are projects, which is
    // how many arrive.
    const room = pending?.querySelector<HTMLElement>("[style]")
    expect(room?.style.getPropertyValue("--rows")).toBe("2")
    expect(room?.className).toContain("h-[min(calc(var(--rows)*2.25rem),8.75rem)]")
    seed(SHORTCUTS)
    view.rerender(<StandaloneAgentDialog />)
    expect(document.querySelector("[data-project-shortcuts-pending]")).toBeNull()
    expect(shortcutRows()).toHaveLength(2)
  })

  // The app already knows there are none, so nothing is held for a list that
  // will never come and the card does not shrink once the answer arrives.
  it("holds no room while the list is on its way when there are no projects", () => {
    seed(null, 0)
    renderOnFolderStep()
    expect(document.querySelector("[data-project-shortcuts-pending]")).toBeNull()
    expect(screen.queryByText("Your projects")).toBeNull()
  })

  it("draws no list when there are no projects", () => {
    seed({ caption: "Caption from the server.", rows: [] })
    renderOnFolderStep()
    expect(shortcutRows()).toEqual([])
    expect(screen.queryByText("Your projects")).toBeNull()
    expect(screen.queryByText("Caption from the server.")).toBeNull()
  })
})

describe("StandaloneAgentDialog name field", () => {
  async function chooseFolder() {
    seed(SHORTCUTS)
    renderOnFolderStep()
    await pick(shortcutRows()[0])
    return screen.getByRole<HTMLInputElement>("textbox", { name: "Agent name" })
  }
  const description = (field: HTMLElement) =>
    (field.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ")

  // The default can be long, and a placeholder cut off at a phone's width
  // hides it, so the line under the field says it.
  it("says the name it falls back to under the field, not in its placeholder", async () => {
    const field = await chooseFolder()
    expect(field.placeholder).not.toContain("site")
    expect(description(field)).toContain("site")
  })

  it("says why a name is refused at the field, and to a screen reader", async () => {
    const field = await chooseFolder()
    fireEvent.change(field, { target: { value: "a..b" } })
    expect(field.getAttribute("aria-invalid")).toBe("true")
    expect(description(field)).toContain("That name is not one git accepts for a branch.")
    expect(createButton().disabled).toBe(true)
  })

  it("names the folder the agent runs in", async () => {
    await chooseFolder()
    const chip = screen.getByText("/home/u/code/site")
    expect(chip.parentElement!.textContent).toMatch(/runs in/i)
  })
})

describe("StandaloneAgentDialog name step", () => {
  // Let the generated name and the folder answers land.
  async function settle() {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 200))
    })
  }
  const nameField = () => screen.getByRole<HTMLInputElement>("textbox", { name: "Agent name" })

  it("asks for a name, pre-filled with a generated one, and names the folder dux creates", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    expect(screen.getByRole("heading", { name: "New standalone agent" })).toBeTruthy()
    expect(nameField().value).toBe("brave-otter")
    const chip = screen.getByText("~/.config/dux/agents/brave-otter")
    expect(chip.parentElement!.textContent).toBe(
      "The agent runs in ~/.config/dux/agents/brave-otter. dux creates the folder.",
    )
    fireEvent.click(createButton())
    expect(createStandaloneAgentInNewFolder).toHaveBeenCalledWith("brave-otter")
    expect(closeStandaloneAgentPicker).toHaveBeenCalled()
  })

  it("follows the typed name in the folder it names", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "team/notes" } })
    expect(screen.getByText("~/.config/dux/agents/team-notes")).toBeTruthy()
  })

  it("disables Create agent for an empty name, and says why", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "" } })
    expect(createButton().disabled).toBe(true)
    fireEvent.click(createButton())
    expect(createStandaloneAgentInNewFolder).not.toHaveBeenCalled()
  })

  it("refuses a name whose folder is already there", async () => {
    takenNames = ["taken"]
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "taken" } })
    await settle()
    expect(
      screen.getByText("A folder with that name is already there. Pick another name."),
    ).toBeTruthy()
    expect(createButton().disabled).toBe(true)
  })

  it("names the agent already recorded at the name's folder", async () => {
    recordedAgents = { otter: "old-otter" }
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "otter" } })
    await settle()
    const agent = screen.getByText("old-otter")
    expect(agent.parentElement!.textContent).toBe(
      "Agent old-otter already has that folder. Pick another name.",
    )
    expect(createButton().disabled).toBe(true)
  })

  it("refuses a name git would not take", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "a..b" } })
    expect(nameField().getAttribute("aria-invalid")).toBe("true")
    expect(createButton().disabled).toBe(true)
  })

  it("leads to the existing-folder picker and back, keeping the name", async () => {
    seed(SHORTCUTS)
    render(<StandaloneAgentDialog />)
    await settle()
    fireEvent.change(nameField(), { target: { value: "kept-name" } })
    fireEvent.click(screen.getByRole("button", { name: "Use an existing folder or project…" }))
    expect(screen.getByRole("heading", { name: "Use an existing folder or project" })).toBeTruthy()
    expect(shortcutRows()).toHaveLength(2)
    fireEvent.click(screen.getByRole("button", { name: /New standalone agent/ }))
    expect(nameField().value).toBe("kept-name")
  })
})
