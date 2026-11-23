// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SimpleTooltip } from "@/components/SimpleTooltip"

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

  it("receives a wrapper's trigger props and ref, so a tooltip around it opens", async () => {
    render(
      <SimpleTooltip content="the server said no" delay={0}>
        <RetryButton onRetry={() => {}} />
      </SimpleTooltip>,
    )
    const button = screen.getByRole("button", { name: "Retry" })
    expect(button.getAttribute("data-slot")).toBe("tooltip-trigger")
    fireEvent.focus(button)
    expect(await screen.findByText("the server said no")).toBeTruthy()
  })

  it("still repeats the load when a wrapper hands it a click of its own", () => {
    const onRetry = vi.fn()
    const own = vi.fn()
    render(<RetryButton onRetry={onRetry} onClick={own} />)
    fireEvent.click(screen.getByRole("button"))
    expect(own).toHaveBeenCalledTimes(1)
    expect(onRetry).toHaveBeenCalledTimes(1)
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
    // Only the sentence is red, never the button's own text.
    expect(screen.getByText("Failed to fetch").className).toContain("text-destructive")
    expect(screen.getByRole("button", { name: "Retry" }).closest(".text-destructive")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it("tells a screen reader the sentence once, and not the button", () => {
    render(<FailedLoad message="Failed to fetch" onRetry={() => {}} />)
    const alerts = screen.getAllByRole("alert")
    expect(alerts).toHaveLength(1)
    expect(alerts[0].textContent).toBe("Failed to fetch")
  })
})
