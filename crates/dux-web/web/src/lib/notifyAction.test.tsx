// @vitest-environment jsdom
import type { ReactElement } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    info: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
    getToasts: vi.fn(() => []),
  }),
}))

const { toast } = await import("sonner")
const { notifyInfo } = await import("./notify")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

// Draws the action button the last raise handed the toaster, focused.
function pressAction(): HTMLElement {
  const options = vi.mocked(toast.info).mock.calls.at(-1)![1] as { action: ReactElement }
  render(options.action)
  const button = screen.getByRole("button", { name: "Go" })
  button.focus()
  fireEvent.click(button)
  return button
}

describe("a toast's action button", () => {
  it("keeps focus when its action leaves the toast up", () => {
    notifyInfo("Something failed.", { action: { label: "Go", onClick: () => {} } })
    const button = pressAction()
    expect(document.activeElement).toBe(button)
  })

  it("lets go of focus first when its action opens something to take it", () => {
    let focusedDuring: Element | null = null
    const onClick = vi.fn(() => {
      focusedDuring = document.activeElement
    })
    notifyInfo("Closed with edits.", { action: { label: "Go", onClick, opensDialog: true } })
    const button = pressAction()
    expect(onClick).toHaveBeenCalledTimes(1)
    expect(focusedDuring).not.toBeNull()
    expect(focusedDuring).not.toBe(button)
  })
})
