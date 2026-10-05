// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

// Review 26: "a commit's final clears the message box only if the box still
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
const { openCommit, closeCommit, setCommitDraft } = await import("@/lib/store")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("review 26: the web commit box", () => {
  it("keeps a message typed for another agent while an earlier commit lands", async () => {
    act(() => openCommit("agent-a"))
    render(<CommitDialog />)
    fireEvent.change(screen.getByPlaceholderText("Commit message…"), {
      target: { value: "commit for A" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    expect(commit).toHaveBeenCalledWith("agent-a", "commit for A")
    // While A's commit runs, the user closes the dialog, opens B's and
    // starts writing B's message.
    act(() => {
      closeCommit()
      openCommit("agent-b")
      setCommitDraft("half-written message for B")
    })
    expect(
      (screen.getByPlaceholderText("Commit message…") as HTMLTextAreaElement)
        .value,
    ).toBe("half-written message for B")
    // A's commit lands.
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    const box = screen.queryByPlaceholderText(
      "Commit message…",
    ) as HTMLTextAreaElement | null
    expect(
      box?.value,
      "A's commit closed B's dialog and wiped the message typed for B",
    ).toBe("half-written message for B")
  })
})
