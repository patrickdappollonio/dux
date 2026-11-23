// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { LoadingRow, Spinner } from "@/components/Spinner"

afterEach(cleanup)

describe("Spinner", () => {
  it("turns only where motion is allowed, and says nothing to a screen reader", () => {
    const { container } = render(<Spinner />)
    const svg = container.querySelector("svg") as SVGElement
    expect(svg.getAttribute("class")).toContain("motion-safe:animate-spin")
    expect(svg.getAttribute("class")).not.toMatch(/(^|\s)animate-spin/)
    expect(svg.getAttribute("aria-hidden")).toBe("true")
  })

  it("makes a loading row's sentence a live region only when asked", () => {
    render(
      <>
        <LoadingRow announce>Loading A…</LoadingRow>
        <LoadingRow>Loading B…</LoadingRow>
      </>,
    )
    expect(screen.getByRole("status").textContent).toBe("Loading A…")
    expect(screen.getAllByRole("status")).toHaveLength(1)
  })
})
