// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, addTab: vi.fn(), createTerminal: vi.fn() }
})
vi.mock("@/hooks/use-agent-active-elsewhere", () => ({
  useAgentChangeBlock: () => null,
  useRowDriverLabel: () => null,
  addToAgentBlockHint: () => "",
}))

const { StackNewDialog } = await import("./StackNewDialog")

afterEach(() => cleanup())

function seed(workspace: unknown) {
  mockState = {
    spine: { sessions: [], projects: [] },
    bootstrap: null,
    createTabInFlight: [],
  } as unknown as DuxState
  return { id: "s1", provider: "claude", tabs: [{}], workspace } as unknown as SessionView
}

describe("StackNewDialog", () => {
  it("says folder for a standalone agent and worktree for a managed one", () => {
    const folder = seed({ kind: "folder", folder_path: "/home/u/notes", folder_label: "~/notes", repo_status: "no_repo" })
    const { unmount } = render(<StackNewDialog session={folder} open onClose={() => {}} />)
    expect(screen.getByText("Another tab works in the same folder; a terminal opens in its dock.")).toBeTruthy()
    expect(screen.getByText("Opens a shell in this agent's folder, in its dock.")).toBeTruthy()
    unmount()
    const managed = seed({ kind: "managed", project_id: "p1", branch_name: "b", worktree_path: "/w" })
    render(<StackNewDialog session={managed} open onClose={() => {}} />)
    expect(screen.getByText("Another tab works in the same worktree; a terminal opens in its dock.")).toBeTruthy()
  })
})
