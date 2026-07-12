// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { RenameEntryDialog } from "./RenameEntryDialog"

afterEach(() => {
  cleanup()
})

describe("RenameEntryDialog", () => {
  it("pre-fills the input with the current final path segment", () => {
    render(
      <RenameEntryDialog
        target={{ path: "src/a/old.ts", isDir: false }}
        isDirty={false}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    const input = screen.getByDisplayValue("old.ts")
    expect(input).toBeTruthy()
  })

  it("titles the dialog with the current final path segment", () => {
    render(
      <RenameEntryDialog
        target={{ path: "src/a/old.ts", isDir: false }}
        isDirty={false}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    expect(screen.getByText("Rename old.ts")).toBeTruthy()
  })

  it("a dirty target shows the blocking message and disables Confirm", () => {
    render(
      <RenameEntryDialog
        target={{ path: "src/a/old.ts", isDir: false }}
        isDirty
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    expect(
      screen.getByText(
        "Save or discard changes in this file before renaming.",
      ),
    ).toBeTruthy()
    const confirm = screen.getByRole("button", { name: /rename/i })
    expect(confirm.hasAttribute("disabled")).toBe(true)
  })

  it("a clean target with a valid new name submits on Enter", () => {
    const onSubmit = vi.fn(() => Promise.resolve())
    render(
      <RenameEntryDialog
        target={{ path: "old.ts", isDir: false }}
        isDirty={false}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    )
    const input = screen.getByDisplayValue("old.ts")
    fireEvent.change(input, { target: { value: "new.ts" } })
    fireEvent.keyDown(input, { key: "Enter" })
    expect(onSubmit).toHaveBeenCalledWith("new.ts")
  })
})
