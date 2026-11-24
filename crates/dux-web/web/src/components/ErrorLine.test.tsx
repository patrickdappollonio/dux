// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { ErrorLine } from "@/components/ErrorLine"

afterEach(cleanup)

describe("ErrorLine", () => {
  it("is an alert by default and no live region when silent", () => {
    const { rerender } = render(<ErrorLine>It failed.</ErrorLine>)
    expect(screen.getByRole("alert").textContent).toBe("It failed.")
    rerender(
      <ErrorLine announce="silent" id="name-error">
        Not a valid name.
      </ErrorLine>,
    )
    expect(screen.queryByRole("alert")).toBeNull()
    expect(screen.queryByRole("status")).toBeNull()
    expect(document.getElementById("name-error")?.textContent).toBe("Not a valid name.")
  })

  it("is read with its field: aria-describedby resolves to the silent line", () => {
    render(
      <>
        <input aria-label="Name" aria-invalid aria-describedby="name-error" />
        <ErrorLine announce="silent" id="name-error">
          Not a valid name.
        </ErrorLine>
      </>,
    )
    const input = screen.getByLabelText("Name")
    expect(input.getAttribute("aria-invalid")).toBe("true")
    const described = document.getElementById(input.getAttribute("aria-describedby") ?? "")
    expect(described?.textContent).toBe("Not a valid name.")
  })

  it("keeps a parser's lines in the log shape, and takes a named size", () => {
    render(
      <>
        <ErrorLine shape="log" size="xs" data-testid="log">
          {"line 1\nline 2"}
        </ErrorLine>
        <ErrorLine size="xs" data-testid="small">
          Small.
        </ErrorLine>
      </>,
    )
    const log = screen.getByTestId("log")
    expect(log.className).toContain("font-mono")
    expect(log.className).toContain("whitespace-pre-wrap")
    expect(log.className).toContain("overflow-y-auto")
    expect(log.className).toContain("text-xs")
    expect(screen.getByTestId("small").className).toContain("text-xs")
  })

  it("renders nothing, and so announces nothing, without words", () => {
    for (const empty of [null, undefined, false, "", [], [null, false, ""]]) {
      for (const announce of ["alert", "silent"] as const) {
        const { container, unmount } = render(<ErrorLine announce={announce}>{empty}</ErrorLine>)
        expect(container.innerHTML).toBe("")
        unmount()
      }
    }
  })

  it("shows a plain string exactly as it arrives, backticks included, in both shapes", () => {
    const { rerender } = render(<ErrorLine>refusing to rename: a`b`c.txt exists</ErrorLine>)
    expect(screen.getByRole("alert").textContent).toBe("refusing to rename: a`b`c.txt exists")
    expect(screen.getByRole("alert").querySelector("code")).toBeNull()
    rerender(<ErrorLine shape="log">missing field `name`</ErrorLine>)
    expect(screen.getByRole("alert").textContent).toBe("missing field `name`")
  })
})
