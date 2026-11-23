// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { EmptyState } from "./empty"

afterEach(cleanup)

describe("EmptyState", () => {
  it("draws the icon, title, description and buttons it is handed", () => {
    render(
      <EmptyState
        size="page"
        icon={<svg data-testid="glyph" />}
        title="No macros yet"
        description="A macro is text you send with one click."
        actions={<button type="button">New macro</button>}
      />,
    )
    expect(screen.getByTestId("glyph")).toBeTruthy()
    expect(screen.getByText("No macros yet")).toBeTruthy()
    expect(screen.getByText("A macro is text you send with one click.")).toBeTruthy()
    expect(screen.getByRole("button", { name: "New macro" })).toBeTruthy()
  })

  // The icon is decoration beside a title that already says it all.
  it("hides the icon from a screen reader", () => {
    const { container } = render(
      <EmptyState size="section" icon={<svg />} title="No changes" />,
    )
    expect(container.querySelector("svg")?.closest("[aria-hidden='true']")).not.toBeNull()
  })

  // A list inside a dialog gets one quiet line, not the icon tile and heading
  // a whole pane gets.
  it("draws a compact one as its sentence alone", () => {
    const { container } = render(
      <EmptyState size="compact" title="No agents yet." />,
    )
    expect(container.textContent).toBe("No agents yet.")
    expect(container.querySelector("svg")).toBeNull()
    expect(container.querySelectorAll("*")).toHaveLength(1)
  })

  // A search's no-match line appears as the user types, so it is announced.
  it("is a status region only when asked to announce itself", () => {
    const { rerender } = render(<EmptyState size="compact" title="No match." />)
    expect(screen.queryByRole("status")).toBeNull()
    rerender(<EmptyState size="compact" title="No match." announce />)
    expect(screen.getByRole("status").textContent).toBe("No match.")
  })
})
