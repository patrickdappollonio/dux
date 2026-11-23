// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { ErrorLine } from "@/components/ErrorLine"

afterEach(cleanup)

describe("ErrorLine", () => {
  it("is an alert when it appears, and a polite status when it follows typing", () => {
    const { rerender } = render(<ErrorLine>It failed.</ErrorLine>)
    expect(screen.getByRole("alert").textContent).toBe("It failed.")
    rerender(<ErrorLine announce="status">Not a valid name.</ErrorLine>)
    expect(screen.queryByRole("alert")).toBeNull()
    expect(screen.getByRole("status").textContent).toBe("Not a valid name.")
  })

  it("renders nothing, and so announces nothing, without words", () => {
    for (const empty of [null, undefined, false, ""]) {
      const { container, unmount } = render(<ErrorLine>{empty}</ErrorLine>)
      expect(container.innerHTML).toBe("")
      unmount()
    }
  })

  it("carries the id a field's aria-describedby names", () => {
    render(<ErrorLine id="field-error">Nope.</ErrorLine>)
    expect(screen.getByRole("alert").id).toBe("field-error")
  })
})
