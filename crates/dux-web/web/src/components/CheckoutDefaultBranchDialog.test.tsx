// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import {
  checkoutDefaultBranch,
  closeCheckoutDefaultBranch,
  type DuxState,
} from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  checkoutDefaultBranch: vi.fn(),
  closeCheckoutDefaultBranch: vi.fn(),
}))

import { CheckoutDefaultBranchDialog } from "@/components/CheckoutDefaultBranchDialog"

afterEach(() => cleanup())

function openFor(projectId: string) {
  mockState = {
    checkoutDefaultBranchTarget: projectId,
    spine: { projects: [{ id: "p1", name: "dux", leading_branch: "develop" }] },
  } as unknown as DuxState
  render(<CheckoutDefaultBranchDialog />)
}

describe("CheckoutDefaultBranchDialog", () => {
  it("asks with the terminal UI's words and its confirm label", () => {
    mockState = {
      checkoutDefaultBranchTarget: "p1",
      spine: { projects: [{ id: "p1", name: "dux", leading_branch: "develop" }] },
    } as unknown as DuxState
    render(<CheckoutDefaultBranchDialog />)

    // The terminal UI's words, with the project and the base drawn as chips in
    // place of its quotes (the lib test pins the quoted spelling byte for byte).
    const body = screen.getByText(/This switches the source checkout for/)
    expect(body.textContent).toBe(
      "This switches the source checkout for dux back to its default branch, moving HEAD in the shared repository. New worktrees branch from develop now. After the checkout, they branch from the default branch.",
    )
    expect(
      [...body.querySelectorAll("code")].map((c) => c.textContent),
    ).toEqual(["dux", "develop"])
    // The terminal UI's button reads the same, in the verb form.
    expect(
      screen.getByRole("button", { name: "Check out default branch" }),
    ).toBeTruthy()
  })

  describe("its buttons and Escape", () => {
    beforeEach(() => {
      vi.mocked(checkoutDefaultBranch).mockClear()
      vi.mocked(closeCheckoutDefaultBranch).mockClear()
    })

    it("checks out the project it was opened for and closes", () => {
      openFor("p1")
      fireEvent.click(
        screen.getByRole("button", { name: "Check out default branch" }),
      )
      expect(checkoutDefaultBranch).toHaveBeenCalledTimes(1)
      expect(checkoutDefaultBranch).toHaveBeenCalledWith("p1")
      expect(closeCheckoutDefaultBranch).toHaveBeenCalled()
    })

    it("opens on Cancel, closes on it and checks nothing out", async () => {
      openFor("p1")
      const cancel = screen.getByRole("button", { name: "Cancel" })
      await waitFor(() => expect(document.activeElement).toBe(cancel))
      fireEvent.click(cancel)
      expect(closeCheckoutDefaultBranch).toHaveBeenCalled()
      expect(checkoutDefaultBranch).not.toHaveBeenCalled()
    })

    it("closes on Escape and checks nothing out", () => {
      openFor("p1")
      fireEvent.keyDown(document.activeElement ?? document.body, {
        key: "Escape",
      })
      expect(closeCheckoutDefaultBranch).toHaveBeenCalled()
      expect(checkoutDefaultBranch).not.toHaveBeenCalled()
    })
  })
})
