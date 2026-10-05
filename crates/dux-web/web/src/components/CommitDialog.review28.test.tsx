// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

// Review 28 (cancel during commit). Built on review 26: "a commit's final clears the message box only if the box still
// holds the committed text". The web's commit dialog clears and closes on the
// commit's answer unconditionally, whatever the box holds by then.

let resolveCommit: (() => void) | null = null
const commit = vi.hoisted(() =>
  vi.fn(
    () =>
      new Promise<void>((resolve) => {
        resolveCommit = resolve
      }),
  ),
)
const notifySuccess = vi.hoisted(() => vi.fn())
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return { ...actual, notifySuccess }
})
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
const { openCommit } = await import("@/lib/store")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("review 28: cancelling while a commit runs", () => {
  it("does not claim a message is being written when the dialog was just cancelled", async () => {
    act(() => openCommit("agent-a"))
    render(<CommitDialog />)
    fireEvent.change(screen.getByPlaceholderText("Commit message…"), {
      target: { value: "commit for A" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    // The user presses Cancel while the commit is in flight.
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(screen.queryByPlaceholderText("Commit message…")).toBeNull()
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    const said = notifySuccess.mock.calls.map((c) => String(c[0]))
    expect(
      said.filter((s) => s.includes("The message you are writing now")),
      "no dialog is open, yet the toast says a message being written was left alone",
    ).toEqual([])
  })
})
