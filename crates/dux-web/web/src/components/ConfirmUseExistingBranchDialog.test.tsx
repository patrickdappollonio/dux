// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import {
  closeExistingBranch,
  confirmCreateWithExistingBranch,
  type DuxState,
} from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  closeExistingBranch: vi.fn(),
  confirmCreateWithExistingBranch: vi.fn(),
}))

import { ConfirmUseExistingBranchDialog } from "@/components/ConfirmUseExistingBranchDialog"

afterEach(() => cleanup())

describe("ConfirmUseExistingBranchDialog", () => {
  // Attaching adopts the branch's history and destroys nothing, so it is the
  // primary action: no red button and no greyed backdrop behind the card.
  it("asks with a primary action, not a destructive one", () => {
    mockState = {
      existingBranchTarget: { name: "fix-auth", location: "remote" },
    } as unknown as DuxState
    render(<ConfirmUseExistingBranchDialog />)

    expect(screen.getByText(/already exists/).textContent).toContain(
      "already exists on the remote (origin)",
    )
    const attach = screen.getByRole("button", { name: "Attach to branch" })
    expect(attach.className).toContain("bg-primary")
    expect(attach.className).not.toContain("bg-destructive")
    expect(document.querySelector(".backdrop-grayscale")).toBeNull()

    fireEvent.click(attach)
    expect(confirmCreateWithExistingBranch).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(closeExistingBranch).toHaveBeenCalled()
  })
})
