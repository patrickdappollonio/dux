// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { closeCopyText, showCopyText } from "@/lib/agentCopy"

import { CopiedTextDialog } from "./CopiedTextDialog"

afterEach(() => {
  act(() => closeCopyText())
  cleanup()
})

describe("CopiedTextDialog", () => {
  it("is closed until a refused copy asks for it", () => {
    render(<CopiedTextDialog />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("shows the whole text read-only, selected, and names the pane as a chip", async () => {
    render(<CopiedTextDialog />)
    const text = "line one\nline two, which the toast only previewed"
    act(() => showCopyText({ source: "fix-login", text }))
    const box = (await screen.findByLabelText("Copied text")) as HTMLTextAreaElement
    expect(box.value).toBe(text)
    expect(box.readOnly).toBe(true)
    fireEvent.focus(box)
    expect(box.selectionStart).toBe(0)
    expect(box.selectionEnd).toBe(text.length)
    const chip = screen.getByText("fix-login")
    expect(chip.closest("code")).not.toBeNull()
  })

  it("closes from its Close button", async () => {
    render(<CopiedTextDialog />)
    act(() => showCopyText({ source: "fix-login", text: "x" }))
    fireEvent.click(await screen.findByRole("button", { name: "Close" }))
    expect(screen.queryByLabelText("Copied text")).toBeNull()
  })
})
