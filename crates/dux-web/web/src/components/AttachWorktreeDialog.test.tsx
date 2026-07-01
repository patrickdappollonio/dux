// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import type { DuxState } from "@/lib/store"
import type { ProjectWorktreeEntryView } from "@/lib/types"

// Override only `useDux` so the dialog reads our seeded state, while the real
// store exports (closeAttachWorktree, attachWorktree) stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what each row's tooltip is wired to reveal.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: ReactNode
    content: ReactNode
  }) => (
    <>
      {children}
      <div data-testid="tooltip-content">{content}</div>
    </>
  ),
}))

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
const { AttachWorktreeDialog } = await import("./AttachWorktreeDialog")

// A path and branch long enough that both would overflow the fixed-width dialog
// without the truncation fix.
const LONG_PATH =
  "/home/user/projects/really-long-worktree-directory-name-that-overflows"
const LONG_BRANCH =
  "feature/some-really-long-branch-name-that-would-overflow-the-row"
const PATH_TAIL = "really-long-worktree-directory-name-that-overflows"

function entry(
  overrides: Partial<ProjectWorktreeEntryView> = {},
): ProjectWorktreeEntryView {
  return {
    worktree_path: LONG_PATH,
    branch_name: LONG_BRANCH,
    adoptable: true,
    reason: null,
    ...overrides,
  }
}

function seed(entries: ProjectWorktreeEntryView[]) {
  mockState = {
    attachWorktreeTarget: "p1",
    attachWorktreeEntries: entries,
    attachWorktreeLoading: false,
    spine: { projects: [{ id: "p1", name: "acme" }] },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AttachWorktreeDialog", () => {
  it("renders the renamed title and submit button", () => {
    // The menu label, dialog title, and button copy must stay aligned; pin the
    // two strings this component owns so a partial rename fails the build.
    seed([entry()])
    render(<AttachWorktreeDialog />)
    expect(
      screen.getByText("New agent from existing worktree in acme"),
    ).toBeTruthy()
    expect(screen.getByRole("button", { name: "Create agent" })).toBeTruthy()
  })

  it("truncates the worktree name and branch so a long row can't overflow", () => {
    // The overflow fix: both lines live in a min-w-0 flex column and carry
    // `truncate`, so long names ellipsize instead of forcing a horizontal
    // scrollbar. Guard the structure so a refactor can't quietly drop it.
    seed([entry()])
    render(<AttachWorktreeDialog />)
    const nameSpan = screen.getByText(PATH_TAIL)
    expect(nameSpan.className).toContain("truncate")
    const column = nameSpan.parentElement
    expect(column?.className).toContain("min-w-0")
    const branchSpan = column?.querySelector("span.font-mono")
    expect(branchSpan?.className).toContain("truncate")
    expect(branchSpan?.textContent).toBe(LONG_BRANCH)
  })

  it("exposes the full path and branch via the row tooltip", () => {
    // Because the row truncates both values, the hover tooltip must carry the
    // full worktree path and branch so they stay recoverable.
    seed([entry()])
    render(<AttachWorktreeDialog />)
    const tip = screen.getByTestId("tooltip-content")
    expect(tip.textContent).toContain(LONG_PATH)
    expect(tip.textContent).toContain(LONG_BRANCH)
  })
})
