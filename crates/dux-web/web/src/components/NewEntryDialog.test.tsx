// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { NewEntryDialog } from "./NewEntryDialog"

afterEach(() => {
  cleanup()
})

describe("NewEntryDialog", () => {
  it("shows 'New file' for kind file and 'New folder' for kind folder", () => {
    const { rerender } = render(
      <NewEntryDialog
        target={{ kind: "file", dir: "" }}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    expect(screen.getByText("New file")).toBeTruthy()

    rerender(
      <NewEntryDialog
        target={{ kind: "folder", dir: "" }}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    expect(screen.getByText("New folder")).toBeTruthy()
  })

  it("disables Create and shows an error for an invalid name", () => {
    render(
      <NewEntryDialog
        target={{ kind: "file", dir: "src" }}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    const input = screen.getByPlaceholderText("example.ts")
    fireEvent.change(input, { target: { value: "a/b" } })
    expect(screen.getByText(/cannot contain a slash/i)).toBeTruthy()
    const create = screen.getByRole("button", { name: /create/i })
    expect(create.hasAttribute("disabled")).toBe(true)
  })

  it("Enter submits a valid name", () => {
    const onSubmit = vi.fn(() => Promise.resolve())
    render(
      <NewEntryDialog
        target={{ kind: "file", dir: "src" }}
        onClose={() => {}}
        onSubmit={onSubmit}
      />,
    )
    const input = screen.getByPlaceholderText("example.ts")
    fireEvent.change(input, { target: { value: "new.ts" } })
    fireEvent.keyDown(input, { key: "Enter" })
    expect(onSubmit).toHaveBeenCalledWith("new.ts")
  })

  it("shows the target dir, using / for the root", () => {
    render(
      <NewEntryDialog
        target={{ kind: "file", dir: "" }}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    )
    expect(screen.getByText("/")).toBeTruthy()
  })
})
