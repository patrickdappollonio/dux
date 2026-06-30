// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { TerminalView } from "@/lib/types"

// Override only `useDux` so the dialog reads our seeded spine + delete target,
// while the real store exports (closeDeleteTerminal, deleteTerminal) stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

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
const { ConfirmDeleteTerminalDialog } = await import(
  "./ConfirmDeleteTerminalDialog"
)

function term(overrides: Partial<TerminalView>): TerminalView {
  return {
    id: "term-1",
    label: "Terminal 1",
    has_output: true,
    foreground_cmd: null,
    ...overrides,
  }
}

function seed(terminal: TerminalView) {
  mockState = {
    deleteTerminalTarget: terminal.id,
    spine: {
      sessions: [{ id: "s1", terminals: [terminal] }],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmDeleteTerminalDialog", () => {
  it("warns that the running app will be killed when one is detected", () => {
    seed(term({ foreground_cmd: "vim" }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(
      screen.getByText(/is running in this terminal and will be killed/),
    ).toBeTruthy()
    expect(screen.getByText("vim")).toBeTruthy()
  })

  it("shows no kill warning when only the shell is running", () => {
    // The bare shell is not an app worth warning about, so an idle terminal
    // confirms with just the title and no "will be killed" line.
    seed(term({ foreground_cmd: null }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(screen.getByText("Close Terminal 1?")).toBeTruthy()
    expect(screen.queryByText(/will be killed/)).toBeNull()
  })
})
