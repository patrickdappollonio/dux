// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { Checkbox } from "./checkbox"

afterEach(cleanup)

// A tri-state select-all is the one caller that needs the third state, and it
// has to read as mixed to a screen reader and look different from both a tick
// and an empty box.
describe("the shared checkbox in its indeterminate state", () => {
  it("reports itself as mixed rather than checked or unchecked", () => {
    render(<Checkbox indeterminate aria-label="Select all" />)
    expect(screen.getByRole("checkbox").getAttribute("aria-checked")).toBe("mixed")
  })

  it("draws a minus rather than a tick, and paints the filled box", () => {
    const { container } = render(<Checkbox indeterminate aria-label="Select all" />)
    const root = screen.getByRole("checkbox")
    expect(root.className).toContain("data-indeterminate:bg-primary")
    expect(container.querySelector(".lucide-minus")).toBeTruthy()
    expect(container.querySelector(".lucide-check")).toBeNull()
  })

  it("draws a tick when it is plainly checked", () => {
    const { container } = render(<Checkbox checked aria-label="Select one" />)
    expect(screen.getByRole("checkbox").getAttribute("aria-checked")).toBe("true")
    expect(container.querySelector(".lucide-check")).toBeTruthy()
  })
})
