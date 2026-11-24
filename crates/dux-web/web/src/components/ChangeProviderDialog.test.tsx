// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

const { ChangeProviderDialog } = await import("./ChangeProviderDialog")

afterEach(() => {
  cleanup()
})

describe("ChangeProviderDialog", () => {
  it("names its one select by what it picks, with no label drawn over it", () => {
    mockState = {
      changeProviderTarget: "s1",
      bootstrap: { available_providers: ["claude", "codex"] },
      spine: {
        projects: [],
        sessions: [
          {
            id: "s1",
            title: "fix-login",
            provider: "claude",
            workspace: { kind: "managed", project_id: "p1", branch_name: "fix-login", worktree_path: "/wt" },
          },
        ],
      },
    } as unknown as DuxState
    render(<ChangeProviderDialog />)
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeTruthy()
    expect(screen.queryByText("Provider")).toBeNull()
  })
})
