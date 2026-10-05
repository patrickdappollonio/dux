// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

// The commit dialog closes on its commit's answer only while it is still the
// dialog the commit was sent from, with the box unchanged.

let resolveCommit: (() => void) | null = null
const commit = vi.hoisted(() =>
  vi.fn(
    () =>
      new Promise<void>((resolve) => {
        resolveCommit = resolve
      }),
  ),
)
vi.mock("@/lib/git", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/git")>()
  return { ...actual, git: { ...actual.git, commit } }
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
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
installBootStubs()
const { CommitDialog } = await import("./CommitDialog")
const { openCommit, setCommitDraft } = await import("@/lib/store")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("CommitDialog: the commit's answer", () => {
  it("closes the dialog it was sent from when nothing changed", async () => {
    act(() => openCommit("agent-a"))
    render(<CommitDialog />)
    fireEvent.change(screen.getByPlaceholderText("Commit message…"), {
      target: { value: "commit for A" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    expect(screen.queryByPlaceholderText("Commit message…")).toBeNull()
  })

  it("leaves the box alone when it was edited after the commit was sent", async () => {
    act(() => openCommit("agent-a"))
    render(<CommitDialog />)
    fireEvent.change(screen.getByPlaceholderText("Commit message…"), {
      target: { value: "commit for A" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    act(() => setCommitDraft("the next message"))
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    const box = screen.getByPlaceholderText(
      "Commit message…",
    ) as HTMLTextAreaElement
    expect(box.value).toBe("the next message")
  })
})
