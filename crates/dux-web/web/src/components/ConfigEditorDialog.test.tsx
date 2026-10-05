// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` (seeded config-editor state) and spy the actions, while
// every other store export stays intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeConfigEditor: vi.fn(),
    openConfigEditor: vi.fn(),
    saveConfigEditor: vi.fn(),
    dismissConfigEditorConflict: vi.fn(),
  }
})

// CodeEditor is lazy-loaded (Monaco cannot mount under vitest, and the real
// dialog must not drag Monaco into the eager bundle; see the component).
// Mocking the module makes the lazy import() resolve to this stub, so the test
// proves the Suspense boundary actually mounts the editor once the chunk lands.
vi.mock("@/components/CodeEditor", () => ({
  default: ({ value }: { value: string }) => (
    <div data-testid="code-editor">{value}</div>
  ),
}))

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
const { ConfigEditorDialog } = await import("./ConfigEditorDialog")

function seed(overrides: Partial<DuxState>) {
  mockState = {
    configEditorOpen: false,
    configEditorContent: "",
    configEditorLoading: false,
    configEditorError: null,
    ...overrides,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfigEditorDialog", () => {
  it("renders nothing while closed", () => {
    seed({})
    render(<ConfigEditorDialog />)
    expect(screen.queryByRole("heading")).toBeNull()
  })

  it("lazily mounts the editor with the loaded config once open", async () => {
    seed({ configEditorOpen: true, configEditorContent: "[server]\nport = 1" })
    render(<ConfigEditorDialog />)
    expect(screen.getByRole("heading").textContent).toBe("Edit config.toml")
    // The file it edits is named as the shared chip, in the title and the note.
    expect(
      screen.getAllByText("config.toml", { selector: "code" }),
    ).toHaveLength(2)
    // findBy waits out the Suspense boundary: the editor arrives only after
    // the lazy chunk resolves, which is the behavior this test pins.
    const editor = await screen.findByTestId("code-editor")
    expect(editor.textContent).toContain("[server]")
  })

  it("shows the load error with a Retry instead of an editable editor", () => {
    seed({ configEditorOpen: true, configEditorError: "boom", configEditorContent: "" })
    render(<ConfigEditorDialog />)
    expect(screen.getByText("boom")).toBeTruthy()
    expect(screen.getByText("Retry")).toBeTruthy()
    expect(screen.queryByTestId("code-editor")).toBeNull()
  })
})

describe("ConfigEditorDialog conflict", () => {
  it("offers keep editing and reloading when the file changed on disk", async () => {
    const store = await import("@/lib/store")
    seed({
      configEditorOpen: true,
      configEditorContent: "[ui]\n",
      configEditorConflict: "config.toml changed on disk since you opened it",
    } as Partial<DuxState>)
    render(<ConfigEditorDialog />)
    expect(screen.getByText("config.toml changed on disk since you opened it")).toBeTruthy()
    const keep = screen.getByRole("button", { name: "Keep editing" })
    const reload = screen.getByRole("button", { name: "Reload the file" })
    keep.click()
    expect(store.dismissConfigEditorConflict).toHaveBeenCalled()
    reload.click()
    expect(store.openConfigEditor).toHaveBeenCalled()
  })
})
