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
})
