// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

const discardMock = vi.fn()
const keepMock = vi.fn()
let mockState: DuxState

vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    discardVanishedEditor: () => discardMock(),
    keepVanishedEditor: () => keepMock(),
  }
})

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
const { ConfirmVanishedEditorDialog } = await import(
  "./ConfirmVanishedEditorDialog"
)

function seed(gone: DuxState["editorTargetGone"]) {
  mockState = { editorTargetGone: gone } as DuxState
}

beforeEach(() => {
  cleanup()
  discardMock.mockClear()
  keepMock.mockClear()
})

describe("the vanished-root confirm", () => {
  it("stays shut while the editor's root is still there", () => {
    seed(null)
    render(<ConfirmVanishedEditorDialog />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("says a terminal closed and offers keeping the text on screen", () => {
    seed({
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "standalone" },
    })
    render(<ConfirmVanishedEditorDialog />)
    expect(
      screen.getByText(/that terminal closed while you were editing/i),
    ).not.toBeNull()
    // The safe half is the default focus, and it is the one that keeps the
    // words where they can be copied out.
    const keep = screen.getByRole("button", { name: "Keep it open" })
    expect(document.activeElement).toBe(keep)
    fireEvent.click(keep)
    expect(keepMock).toHaveBeenCalledTimes(1)
    expect(discardMock).not.toHaveBeenCalled()
  })

  it("names the agent case differently, and its discard is the destructive half", () => {
    seed({ kind: "agent", sessionId: "s1" })
    render(<ConfirmVanishedEditorDialog />)
    expect(screen.getByText(/that agent is gone/i)).not.toBeNull()
    const discard = screen.getByRole("button", { name: "Discard & leave" })
    expect(discard.className).toContain("destructive")
    fireEvent.click(discard)
    expect(discardMock).toHaveBeenCalledTimes(1)
  })
})
