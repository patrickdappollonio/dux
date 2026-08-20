// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` (seeded spine + target/draft) and spy the store actions the
// dialog fires, while every other store export stays intact. The submit's HTTP
// contract (PUT with the typed text) is covered at the store level in
// `lib/restActionsStore.test.ts`; what is pinned here is the dialog's wiring:
// the current-PR line, the submit/close paths, and the vanished-target guard.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setAttachPullRequestDraft: vi.fn(),
    submitAttachPullRequest: vi.fn(),
    closeAttachPullRequest: vi.fn(),
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
const { AttachPullRequestDialog } = await import("./AttachPullRequestDialog")
const store = await import("@/lib/store")
const setAttachPullRequestDraft = vi.mocked(store.setAttachPullRequestDraft)
const submitAttachPullRequest = vi.mocked(store.submitAttachPullRequest)
const closeAttachPullRequest = vi.mocked(store.closeAttachPullRequest)

function seed(
  target: string | null,
  draft: string,
  sessions: unknown[],
) {
  mockState = {
    attachPullRequestTarget: target,
    attachPullRequestDraft: draft,
    spine: { sessions },
  } as unknown as DuxState
}

const bare = {
  id: "s1",
  title: "quacky-mallard",
  workspace: { kind: "managed", project_id: "p1", branch_name: "dux/s1", initial_branch: "dux/s1", branch_provenance: "created", source_branch: "main", worktree_path: "/wt/s1" },
}
const withAutoPr = {
  ...bare,
  pr: {
    number: 7,
    state: "open",
    title: "Fix the flap",
    url: "https://github.com/o/r/pull/7",
    overridden: false,
  },
}
const withPinnedPr = {
  ...bare,
  pr: { ...withAutoPr.pr, overridden: true },
}

beforeEach(() => {
  installBootStubs()
  setAttachPullRequestDraft.mockClear()
  submitAttachPullRequest.mockClear()
  closeAttachPullRequest.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AttachPullRequestDialog", () => {
  it("renders the field with the exact placeholder and no current-PR line", () => {
    seed("s1", "", [bare])
    render(<AttachPullRequestDialog />)
    expect(screen.getByPlaceholderText("PR URL, #123, or 123")).toBeTruthy()
    expect(screen.queryByText(/Currently showing/)).toBeNull()
  })

  it("names the current PR so overriding is explicit", () => {
    seed("s1", "", [withAutoPr])
    render(<AttachPullRequestDialog />)
    expect(screen.getByText(/Currently showing/)).toBeTruthy()
    expect(screen.getByText(/#7 Fix the flap/)).toBeTruthy()
    // Autodetected: not called out as manually attached.
    expect(screen.queryByText(/manually attached/)).toBeNull()
  })

  it("says 'manually attached' when the shown PR is an override", () => {
    seed("s1", "", [withPinnedPr])
    render(<AttachPullRequestDialog />)
    expect(screen.getByText(/manually attached/)).toBeTruthy()
  })

  it("routes typing to the store draft", () => {
    seed("s1", "", [bare])
    render(<AttachPullRequestDialog />)
    fireEvent.change(screen.getByPlaceholderText("PR URL, #123, or 123"), {
      target: { value: "#123" },
    })
    expect(setAttachPullRequestDraft).toHaveBeenCalledWith("#123")
  })

  it("submits on the Attach button and on Enter in the field", () => {
    seed("s1", "#123", [bare])
    render(<AttachPullRequestDialog />)
    fireEvent.click(screen.getByText("Attach"))
    expect(submitAttachPullRequest).toHaveBeenCalledTimes(1)
    fireEvent.keyDown(screen.getByPlaceholderText("PR URL, #123, or 123"), {
      key: "Enter",
    })
    expect(submitAttachPullRequest).toHaveBeenCalledTimes(2)
  })

  it("does not submit an empty draft (button disabled, Enter inert)", () => {
    seed("s1", "   ", [bare])
    render(<AttachPullRequestDialog />)
    expect(
      (screen.getByText("Attach") as HTMLButtonElement).disabled,
    ).toBe(true)
    fireEvent.keyDown(screen.getByPlaceholderText("PR URL, #123, or 123"), {
      key: "Enter",
    })
    expect(submitAttachPullRequest).not.toHaveBeenCalled()
  })

  it("cancel closes without submitting", () => {
    seed("s1", "#123", [bare])
    render(<AttachPullRequestDialog />)
    fireEvent.click(screen.getByText("Cancel"))
    expect(closeAttachPullRequest).toHaveBeenCalled()
    expect(submitAttachPullRequest).not.toHaveBeenCalled()
  })

  it("closes itself when the target agent vanishes from the ViewModel", () => {
    seed("s1", "", [])
    render(<AttachPullRequestDialog />)
    expect(screen.queryByText("Attach pull request")).toBeNull()
    expect(closeAttachPullRequest).toHaveBeenCalled()
  })
})
