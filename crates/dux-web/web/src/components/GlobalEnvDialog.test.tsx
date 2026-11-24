// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeGlobalEnv: vi.fn(),
    dismissGlobalEnvConflict: vi.fn(),
    saveGlobalEnv: vi.fn(),
    reloadGlobalEnvDraft: vi.fn(() => Promise.resolve()),
  }
})

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
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
// The pointer is asked through `matchMedia`, which jsdom lacks.
function coarsePointer() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query === "(pointer: coarse)",
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
}

installBootStubs()
const { GlobalEnvDialog } = await import("./GlobalEnvDialog")
const store = await import("@/lib/store")
const { openModals } = await import("@/lib/modalHistory")

function seed() {
  mockState = {
    bootstrap: { global_env: { FOO: "1" } },
    globalEnvOpen: true,
    globalEnvDraft: null,
    globalEnvConflict: null,
    globalEnvEpoch: 0,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  seed()
})
afterEach(() => {
  cleanup()
  phone = false
  vi.unstubAllGlobals()
  vi.mocked(store.closeGlobalEnv).mockClear()
})

describe("GlobalEnvDialog", () => {
  it("puts the caret in the text under a mouse", async () => {
    render(<GlobalEnvDialog />)
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByPlaceholderText("KEY=VALUE")),
    )
  })

  it("does not focus the text under a finger, where that raises the keyboard", async () => {
    phone = true
    coarsePointer()
    render(<GlobalEnvDialog />)
    const popup = document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!
    expect(popup.dataset.presentation).toBe("cover")
    await waitFor(() => expect(document.activeElement).toBe(popup))
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy()
  })

  it("focuses Keep editing when a refused save reopens it", async () => {
    mockState = {
      ...mockState,
      globalEnvDraft: { FOO: "2" },
      globalEnvConflict: "The global environment changed.",
    } as DuxState
    render(<GlobalEnvDialog />)
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep editing" })),
    )
  })

  it("names its one box by what it takes and reads the note about lines Save leaves out with it", () => {
    render(<GlobalEnvDialog />)
    const env = screen.getByRole("textbox", { name: "Environment variables" })
    fireEvent.change(env, { target: { value: "not a variable" } })
    const note = document.getElementById(env.getAttribute("aria-describedby") ?? "")
    expect(note?.textContent).toMatch(/Save leaves it out/)
  })

  it("keeps the same box, with its focus and what was typed, when the layout flips between computer and phone", () => {
    const { rerender } = render(<GlobalEnvDialog />)
    const env = screen.getByRole("textbox", { name: "Environment variables" })
    fireEvent.change(env, { target: { value: "FOO=1\nBAR=2" } })
    env.focus()
    phone = true
    rerender(<GlobalEnvDialog />)
    expect(screen.getByRole("textbox", { name: "Environment variables" })).toBe(env)
    expect(document.activeElement).toBe(env)
    phone = false
    rerender(<GlobalEnvDialog />)
    expect(screen.getByRole("textbox", { name: "Environment variables" })).toBe(env)
    expect(document.activeElement).toBe(env)
  })

  it("draws the note about lines Save leaves out as the form's own 12px note under the box", () => {
    render(<GlobalEnvDialog />)
    const env = screen.getByRole("textbox", { name: "Environment variables" })
    fireEvent.change(env, { target: { value: "not a variable" } })
    const note = document.getElementById(env.getAttribute("aria-describedby") ?? "")!
    expect(note.className).toContain("text-xs")
    expect(note.className).not.toContain("text-sm")
    expect(note.closest("[data-form-layout]")?.contains(env)).toBe(true)
  })

  it("closes at once when nothing was changed", () => {
    render(<GlobalEnvDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(store.closeGlobalEnv).toHaveBeenCalled()
  })

  it("asks before Cancel throws an edit away, and Keep editing keeps it", async () => {
    render(<GlobalEnvDialog />)
    const text = screen.getByPlaceholderText("KEY=VALUE")
    fireEvent.change(text, { target: { value: "FOO=1\nBAR=2" } })
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(await screen.findByText("Discard unsaved changes?")).toBeTruthy()
    expect(screen.getByText(/Discarding (it|them) leaves/).textContent).toBe(
      "The global environment has 1 unsaved change. Discarding it leaves the global environment as it was.",
    )
    expect(store.closeGlobalEnv).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
    await waitFor(() => expect(screen.queryByText("Discard unsaved changes?")).toBeNull())
    expect((text as HTMLTextAreaElement).value).toBe("FOO=1\nBAR=2")
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    fireEvent.click(await screen.findByRole("button", { name: "Discard changes" }))
    expect(store.closeGlobalEnv).toHaveBeenCalled()
  })

  it("holds Save while nothing would change, and says why", async () => {
    render(<GlobalEnvDialog />)
    const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.click(save.parentElement!)
    expect(
      (await screen.findAllByText("Nothing to save yet: no variable has changed.")).length,
    ).toBeGreaterThan(0)
  })

  it("asks before a line Save cannot use is thrown away, and says Save leaves it out", async () => {
    render(<GlobalEnvDialog />)
    const text = screen.getByPlaceholderText("KEY=VALUE")
    fireEvent.change(text, { target: { value: "FOO=1\nBAR" } })
    expect(screen.getByText("Line 2 is not KEY=VALUE, so Save leaves it out.")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect((await screen.findByText(/Discarding/)).textContent).toMatch(/ 1 unsaved change\./)
    expect(store.closeGlobalEnv).not.toHaveBeenCalled()
  })

  it("takes a history entry when the person opens it, and none when a refused save reopens it", () => {
    const { unmount } = render(<GlobalEnvDialog />)
    expect(openModals()).toHaveLength(1)
    unmount()
    mockState = {
      ...mockState,
      globalEnvDraft: { FOO: "2" },
      globalEnvConflict: "The global environment changed.",
    } as DuxState
    render(<GlobalEnvDialog />)
    expect(openModals()).toHaveLength(0)
  })

  it("puts the caret back in the text after Reload the environment", async () => {
    mockState = {
      ...mockState,
      globalEnvDraft: { FOO: "2" },
      globalEnvConflict: "The global environment changed.",
    } as DuxState
    const { rerender } = render(<GlobalEnvDialog />)
    const reload = screen.getByRole("button", { name: "Reload the environment" })
    reload.focus()
    fireEvent.click(reload)
    await act(async () => {
      mockState = {
        ...mockState,
        globalEnvDraft: null,
        globalEnvConflict: null,
        globalEnvEpoch: 1,
      } as DuxState
      rerender(<GlobalEnvDialog />)
    })
    const text = screen.getByPlaceholderText("KEY=VALUE") as HTMLTextAreaElement
    expect(text.value).toBe("FOO=1")
    expect(document.activeElement).toBe(text)
  })

  it("puts focus on the dialog, not the text, after Reload the environment under a finger", async () => {
    coarsePointer()
    mockState = {
      ...mockState,
      globalEnvDraft: { FOO: "2" },
      globalEnvConflict: "The global environment changed.",
    } as DuxState
    const { rerender } = render(<GlobalEnvDialog />)
    const reload = screen.getByRole("button", { name: "Reload the environment" })
    reload.focus()
    fireEvent.click(reload)
    await act(async () => {
      mockState = {
        ...mockState,
        globalEnvDraft: null,
        globalEnvConflict: null,
        globalEnvEpoch: 1,
      } as DuxState
      rerender(<GlobalEnvDialog />)
    })
    const popup = document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!
    expect(document.activeElement).toBe(popup)
  })
})
