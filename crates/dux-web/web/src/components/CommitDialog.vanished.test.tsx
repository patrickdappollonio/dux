// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeCommit: vi.fn() }
})

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
const { CommitDialog } = await import("./CommitDialog")
const store = await import("@/lib/store")

afterEach(() => {
  cleanup()
  vi.mocked(store.closeCommit).mockClear()
})

function seed(sessions: { id: string }[]) {
  mockState = {
    commitTarget: "agent-a",
    commitDraft: "a message",
    spine: { sessions, projects: [], terminals: [] },
  } as unknown as DuxState
}

describe("CommitDialog over an agent that is gone", () => {
  it("stays open while its agent exists", () => {
    seed([{ id: "agent-a" }])
    render(<CommitDialog />)
    expect(screen.getByPlaceholderText("Commit message…")).toBeTruthy()
    expect(store.closeCommit).not.toHaveBeenCalled()
  })

  it("closes itself once its agent is deleted", () => {
    seed([])
    render(<CommitDialog />)
    expect(store.closeCommit).toHaveBeenCalled()
  })
})
