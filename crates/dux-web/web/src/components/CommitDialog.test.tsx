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
const { getSnapshot, openCommit, setCommitDraft } = await import("@/lib/store")

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

// A commit is a git write, refused by the server from a page that does not
// drive the agent, so the dialog keeps the message and offers no commit.
describe("CommitDialog while this page does not drive the agent", () => {
  it("keeps the message, refuses the commit with the reason, and gives it back on take-over", async () => {
    const { agentRowKey, registerPaneCover, resetPaneCovers } = await import(
      "@/lib/paneCover"
    )
    const { closeCommit } = await import("@/lib/store")
    act(() => openCommit("agent-a"))
    act(() => setCommitDraft("a message"))
    let retire = () => {}
    act(() => {
      retire = registerPaneCover("agent-a", "card", {
        row: agentRowKey("agent-a"),
        driver: "other",
      })
    })
    render(<CommitDialog />)
    const box = screen.getByPlaceholderText("Commit message…") as HTMLTextAreaElement
    const button = screen.getByRole("button", { name: "Commit" })
    expect(box.value).toBe("a message")
    expect(box.readOnly).toBe(true)
    expect(button.hasAttribute("disabled")).toBe(true)
    expect(
      screen.getByText(
        "This agent is active on another device, so actions that modify it are disabled. Take over the agent to use them here.",
      ),
    ).toBeTruthy()
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true })
    expect(commit).not.toHaveBeenCalled()

    act(() => {
      retire()
      retire = registerPaneCover("agent-a", "none", {
        row: agentRowKey("agent-a"),
        driver: "this",
      })
    })
    expect(box.readOnly).toBe(false)
    expect(button.hasAttribute("disabled")).toBe(false)
    act(() => {
      retire()
      resetPaneCovers()
      closeCommit()
    })
  })
})

// A message closed without committing is kept for its agent, in this page
// only, and given back when Commit is opened for that agent again.
describe("CommitDialog: the message kept for an agent", () => {
  const box = () => screen.getByPlaceholderText("Commit message…") as HTMLTextAreaElement

  it("gives a message closed by Escape back to the same agent, and none to another", async () => {
    const { closeCommit } = await import("@/lib/store")
    act(() => openCommit("agent-keep"))
    render(<CommitDialog />)
    fireEvent.change(box(), { target: { value: "half a message" } })
    fireEvent.keyDown(box(), { key: "Escape" })
    expect(getSnapshot().commitTarget).toBeNull()
    act(() => openCommit("agent-other"))
    expect(box().value).toBe("")
    act(() => closeCommit())
    act(() => openCommit("agent-keep"))
    expect(box().value).toBe("half a message")
    act(() => closeCommit())
  })

  it("forgets the message once it is committed", async () => {
    act(() => openCommit("agent-done"))
    render(<CommitDialog />)
    fireEvent.change(box(), { target: { value: "the whole message" } })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    expect(getSnapshot().commitTarget).toBeNull()
    act(() => openCommit("agent-done"))
    expect(box().value).toBe("")
  })

  it("forgets a message closed while its commit ran, once the commit lands", async () => {
    const { closeCommit } = await import("@/lib/store")
    act(() => openCommit("agent-ran"))
    render(<CommitDialog />)
    fireEvent.change(box(), { target: { value: "sent then closed" } })
    fireEvent.click(screen.getByRole("button", { name: "Commit" }))
    act(() => closeCommit())
    await act(async () => {
      resolveCommit?.()
      await Promise.resolve()
    })
    act(() => openCommit("agent-ran"))
    expect(box().value).toBe("")
    act(() => closeCommit())
  })
})
