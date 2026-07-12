// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { DeleteEntryDialog } from "./DeleteEntryDialog"

afterEach(() => {
  cleanup()
})

describe("DeleteEntryDialog", () => {
  it("Cancel has autoFocus", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "a.ts", isDir: false }}
        onClose={() => {}}
        onConfirm={() => {}}
      />,
    )
    const cancel = screen.getByRole("button", { name: "Cancel" })
    expect(document.activeElement).toBe(cancel)
  })

  it("the confirm button is styled destructive", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "a.ts", isDir: false }}
        onClose={() => {}}
        onConfirm={() => {}}
      />,
    )
    const confirm = screen.getByRole("button", { name: "Delete" })
    expect(confirm.className).toContain("destructive")
  })

  it("folder copy mentions recursive deletion of everything inside", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "src", isDir: true }}
        onClose={() => {}}
        onConfirm={() => {}}
      />,
    )
    expect(screen.getByText(/everything inside/i)).toBeTruthy()
    expect(screen.getByText(/recursive/i)).toBeTruthy()
  })

  it("copy states the deletion is permanent and cannot be undone", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "a.ts", isDir: false }}
        onClose={() => {}}
        onConfirm={() => {}}
      />,
    )
    expect(screen.getByText(/permanently deleted/i)).toBeTruthy()
    expect(screen.getByText(/cannot be undone/i)).toBeTruthy()
  })

  it("Delete fires onConfirm", () => {
    const onConfirm = vi.fn()
    render(
      <DeleteEntryDialog
        target={{ path: "a.ts", isDir: false }}
        onClose={() => {}}
        onConfirm={onConfirm}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(onConfirm).toHaveBeenCalled()
  })

  // Finding 3: an in-flight save must block deleting the same path, the same
  // way RenameEntryDialog's `isDirty` gate blocks a dirty rename.
  describe("blockedBySave", () => {
    it("disables Delete and does not fire onConfirm when clicked", () => {
      const onConfirm = vi.fn()
      render(
        <DeleteEntryDialog
          target={{ path: "a.ts", isDir: false }}
          blockedBySave
          onClose={() => {}}
          onConfirm={onConfirm}
        />,
      )
      const confirm = screen.getByRole("button", { name: "Delete" })
      expect(confirm.hasAttribute("disabled")).toBe(true)
      fireEvent.click(confirm)
      expect(onConfirm).not.toHaveBeenCalled()
    })

    it("shows a blocking note instead of the usual destructive warning", () => {
      render(
        <DeleteEntryDialog
          target={{ path: "a.ts", isDir: false }}
          blockedBySave
          onClose={() => {}}
          onConfirm={() => {}}
        />,
      )
      expect(screen.getByText(/currently being saved/i)).toBeTruthy()
      expect(screen.queryByText(/permanently deleted/i)).toBeNull()
    })

    it("Cancel still closes normally while blocked", () => {
      render(
        <DeleteEntryDialog
          target={{ path: "a.ts", isDir: false }}
          blockedBySave
          onClose={() => {}}
          onConfirm={() => {}}
        />,
      )
      const cancel = screen.getByRole("button", { name: "Cancel" })
      expect(cancel.hasAttribute("disabled")).toBe(false)
    })

    it("defaults to unblocked when the prop is omitted", () => {
      const onConfirm = vi.fn()
      render(
        <DeleteEntryDialog
          target={{ path: "a.ts", isDir: false }}
          onClose={() => {}}
          onConfirm={onConfirm}
        />,
      )
      const confirm = screen.getByRole("button", { name: "Delete" })
      expect(confirm.hasAttribute("disabled")).toBe(false)
      fireEvent.click(confirm)
      expect(onConfirm).toHaveBeenCalled()
    })
  })
})
