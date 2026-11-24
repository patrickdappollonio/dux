// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { PASSWORD_EDIT_REFUSAL } from "@/lib/configApi"

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
  default: ({ value, onChange }: { value: string; onChange: (text: string) => void }) => (
    <textarea
      data-testid="code-editor"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
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
    const editor = (await screen.findByTestId("code-editor")) as HTMLTextAreaElement
    expect(editor.value).toContain("[server]")
  })

  // Enter on a focused Cancel would close the editor the moment it opened.
  it("opens with focus on no button, and leaves it there when the config arrives", async () => {
    seed({ configEditorOpen: true, configEditorLoading: true })
    const { rerender } = render(<ConfigEditorDialog />)
    const dialog = await screen.findByRole("dialog")
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
    const opened = document.activeElement
    expect(opened?.tagName).not.toBe("BUTTON")
    seed({ configEditorOpen: true, configEditorContent: "[ui]\n" })
    rerender(<ConfigEditorDialog />)
    await screen.findByTestId("code-editor")
    await new Promise((r) => setTimeout(r, 50))
    expect(document.activeElement).toBe(opened)
  })

  it("asks before Escape throws away what was typed", async () => {
    const store = await import("@/lib/store")
    vi.mocked(store.closeConfigEditor).mockClear()
    seed({ configEditorOpen: true, configEditorContent: "[ui]\n" })
    render(<ConfigEditorDialog />)
    const editor = await screen.findByTestId("code-editor")
    fireEvent.change(editor, { target: { value: "[ui]\ntheme = 1\n" } })
    fireEvent.keyDown(editor, { key: "Escape" })
    expect(await screen.findByText("Discard unsaved changes?")).toBeTruthy()
    expect(store.closeConfigEditor).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }))
    expect(store.closeConfigEditor).toHaveBeenCalled()
  })

  it("takes no Save while the file is loading", () => {
    seed({ configEditorOpen: true, configEditorLoading: true })
    render(<ConfigEditorDialog />)
    const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
  })

  it("keeps a failed read a failed read while it closes", () => {
    seed({ configEditorOpen: true, configEditorError: "boom", configEditorContent: "" })
    const { rerender } = render(<ConfigEditorDialog />)
    // What closing leaves in the store: no error and no content.
    seed({ configEditorOpen: false })
    rerender(<ConfigEditorDialog />)
    expect(screen.queryByTestId("code-editor")).toBeNull()
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull()
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
  it("draws dux's password refusal with its command as a chip, and a parse error as typed", () => {
    seed({
      configEditorOpen: true,
      configEditorContent: "[ui]\n",
      configEditorError: PASSWORD_EDIT_REFUSAL,
    } as Partial<DuxState>)
    const { unmount } = render(<ConfigEditorDialog />)
    expect(
      [...document.querySelectorAll("[data-slot='inline-code']")].map((c) => c.textContent),
    ).toContain("dux config set server.auth.password")
    unmount()
    const parseError = "TOML parse error at line 2, key = `x`, expected `\"`"
    seed({
      configEditorOpen: true,
      configEditorContent: "[ui]\n",
      configEditorError: parseError,
    } as Partial<DuxState>)
    render(<ConfigEditorDialog />)
    expect(document.body.textContent).toContain(parseError)
  })
  it("draws the command in the refusal as a chip, with its words and no backticks", () => {
    const refusal =
      "config.toml changed on disk since you opened it (a `dux config set`, a blocked address, or another editor), and saving would undo that change."
    seed({
      configEditorOpen: true,
      configEditorContent: "[ui]\n",
      configEditorConflict: refusal,
    } as Partial<DuxState>)
    render(<ConfigEditorDialog />)
    const box = document.querySelector("[data-stale-save]")
    const chips = box!.querySelectorAll("[data-slot='inline-code']")
    expect([...chips].map((chip) => chip.textContent)).toEqual(["dux config set"])
    expect(box?.textContent).toContain(refusal.replaceAll("`", ""))
    expect(box?.textContent).not.toContain("`")
  })
})
