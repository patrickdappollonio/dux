// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { FailedLoad, RetryButton } from "./retry-button"

afterEach(cleanup)

describe("RetryButton", () => {
  it("says Retry with the one retry icon and asks again on a press", () => {
    const onRetry = vi.fn()
    render(<RetryButton onRetry={onRetry} />)
    const button = screen.getByRole("button", { name: "Retry" })
    expect(button.querySelector("svg.lucide-rotate-cw")).not.toBeNull()
    fireEvent.click(button)
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it("keeps the place's own wording, and the touch floor in every size that has none by height", () => {
    render(<RetryButton onRetry={() => {}} label="Couldn't refresh · Retry" size="row" />)
    const row = screen.getByRole("button", { name: "Couldn't refresh · Retry" })
    expect(row.className).toContain("pointer-coarse:min-h-11")
    cleanup()
    render(<RetryButton onRetry={() => {}} />)
    expect(screen.getByRole("button").className).toContain("pointer-coarse:min-h-10")
  })

  it("takes no press while disabled", () => {
    const onRetry = vi.fn()
    render(<RetryButton onRetry={onRetry} disabled />)
    fireEvent.click(screen.getByRole("button"))
    expect(onRetry).not.toHaveBeenCalled()
  })
})

describe("FailedLoad", () => {
  it("shows the failure's sentence with the retry beside it", () => {
    const onRetry = vi.fn()
    render(<FailedLoad message="Failed to fetch" onRetry={onRetry} />)
    expect(screen.getByText("Failed to fetch")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })
})
