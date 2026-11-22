// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

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
    expect(screen.getByText(/Discarding them leaves/).textContent).toBe(
      "The global environment has 1 unsaved change. Discarding them leaves the global environment as it was.",
    )
    expect(store.closeGlobalEnv).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
    await waitFor(() => expect(screen.queryByText("Discard unsaved changes?")).toBeNull())
    expect((text as HTMLTextAreaElement).value).toBe("FOO=1\nBAR=2")
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    fireEvent.click(await screen.findByRole("button", { name: "Discard changes" }))
    expect(store.closeGlobalEnv).toHaveBeenCalled()
  })
})
