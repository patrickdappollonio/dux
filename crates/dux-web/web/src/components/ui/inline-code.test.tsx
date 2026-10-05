// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { InlineCode } from "./inline-code"
import { renderInlineCode } from "@/lib/inlineMarkdown"

afterEach(cleanup)

// A name inside a sentence (a branch, a path, a command) is told apart from the
// prose around it by this chip, and by nothing else: no quotes.
describe("the inline code chip for a name in prose", () => {
  it("renders the value in a code element styled only with tokens", () => {
    render(
      <p>
        Also delete the branch <InlineCode>launch-at-login</InlineCode>
      </p>,
    )
    const code = screen.getByText("launch-at-login")
    expect(code.tagName).toBe("CODE")
    for (const cls of ["rounded", "bg-muted", "font-mono"]) {
      expect(code.className).toContain(cls)
    }
    expect(code.className).not.toMatch(/#[0-9a-f]{3,6}|rgb\(/i)
  })

  it("wraps a long value anywhere, so a path never scrolls a phone sideways", () => {
    render(<InlineCode>/home/someone/a/very/long/path/without/any/spaces/at/all</InlineCode>)
    const code = screen.getByText(/very\/long/)
    expect(code.className).toContain("wrap-anywhere")
  })

  it("keeps a chip whole on one row, wrapping it to the next line rather than splitting it", () => {
    // An inline block never breaks across lines, so `dux config set
    // server.auth.password` moves to the next row whole; capped at the line's
    // width, a chip longer than a whole row wraps inside itself instead of
    // pushing a phone sideways.
    render(<InlineCode>dux config set server.auth.password</InlineCode>)
    const code = screen.getByText("dux config set server.auth.password")
    expect(code.className).toContain("inline-block")
    expect(code.className).toContain("max-w-full")
  })

  it("keeps the spaces inside a value instead of collapsing them", () => {
    const { container } = render(<InlineCode>npm  run   dev</InlineCode>)
    const code = container.querySelector("code")
    expect(code?.textContent).toBe("npm  run   dev")
    expect(code?.className).toContain("whitespace-pre-wrap")
  })

  it("is plain inline text to a screen reader", () => {
    render(<InlineCode>main</InlineCode>)
    const code = screen.getByText("main")
    expect(code.getAttribute("aria-hidden")).toBeNull()
    expect(code.getAttribute("role")).toBeNull()
  })

  it("accepts extra classes without losing its own", () => {
    render(<InlineCode className="text-foreground">main</InlineCode>)
    const code = screen.getByText("main")
    expect(code.className).toContain("text-foreground")
    expect(code.className).toContain("bg-muted")
  })

  it("is the same chip the backtick renderer draws", () => {
    const { container } = render(<>{renderInlineCode("Run `gh` first.")}</>)
    render(<InlineCode>gh-direct</InlineCode>)
    const fromMarkdown = container.querySelector("code")
    expect(fromMarkdown?.className).toBe(screen.getByText("gh-direct").className)
  })
})
