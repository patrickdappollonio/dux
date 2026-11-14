// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { InlineCode } from "./inline-code"
import { renderInlineCode } from "@/lib/inlineMarkdown"

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

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

  // jsdom cannot lay a line out, so the classes that decide the layout are
  // pinned, and the chip's width against its row is stubbed.
  it("moves to the next row whole when it fits on one, so a hyphen never splits it", () => {
    render(
      <p>
        Saved the startup command for project <InlineCode>demo-web</InlineCode>.
      </p>,
    )
    const code = screen.getByText("demo-web")
    const classes = code.className.split(/\s+/)
    expect(code.dataset.wrap).toBe("whole")
    // No break inside it, and it stays an inline box: an inline block is a
    // break point before whatever follows it, so the closing `.` could start
    // a row alone, and a truncating row would swallow it whole.
    expect(classes).toContain("whitespace-pre")
    expect(classes).toContain("inline")
    expect(classes).not.toContain("inline-block")
    expect(classes).not.toContain("wrap-anywhere")
  })

  it("flows like text when it is longer than a row, each row's background hugging that row", () => {
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(200)
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return { width: (this.textContent ?? "").length * 8 } as DOMRect
    })
    render(
      <p>
        Saved it to <InlineCode>~/.config/dux/worktrees/dux/server-mode/.dux/uploads</InlineCode>{" "}
        and sent its path, then <InlineCode>demo-web</InlineCode> took it.
      </p>,
    )
    const long = screen.getByText(/\.dux\/uploads/)
    const classes = long.className.split(/\s+/)
    expect(long.dataset.wrap).toBe("flow")
    expect(classes).toContain("inline")
    expect(classes).not.toContain("inline-block")
    expect(classes).toContain("box-decoration-clone")
    // Breaking mid-run is the last resort: `break-all` would split every name
    // at the edge of the line.
    expect(classes).toContain("wrap-anywhere")
    expect(classes).not.toContain("break-all")
    // Its spaces are kept, but whether it wraps at all is its row's to say, so
    // in a row that truncates it stays on one line.
    expect(classes).toContain("[white-space-collapse:preserve]")
    expect(classes.some((c) => c.startsWith("whitespace-"))).toBe(false)
    expect(screen.getByText("demo-web").dataset.wrap).toBe("whole")
  })

  // A dialog opens with a zoom from 95%, and the chip is first measured inside
  // it: the row's width is a layout width, the chip's an on-screen one.
  it("measures a chip in a zooming dialog at its laid-out width, so one a little wider than the row flows", () => {
    const zoom = 0.95
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(200)
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(200)
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const laidOut = this.tagName === "P" ? 200 : (this.textContent ?? "").length * 8
      return { width: laidOut * zoom } as DOMRect
    })
    render(
      <p>
        Delete <InlineCode>feature-branch-name-xxxxxx</InlineCode> now
      </p>,
    )
    // 26 characters lay out at 208px against a 200px row, but are drawn at
    // under 198px while the zoom runs.
    expect(screen.getByText("feature-branch-name-xxxxxx").dataset.wrap).toBe("flow")
  })

  it("keeps the spaces inside a value instead of collapsing them", () => {
    const { container } = render(<InlineCode>npm  run   dev</InlineCode>)
    const code = container.querySelector("code")
    expect(code?.textContent).toBe("npm  run   dev")
    expect(code?.className.split(/\s+/)).toContain("whitespace-pre")
  })

  it("leaves the measuring probe off the chip once it has measured", () => {
    render(
      <p>
        Saved <InlineCode>demo-web</InlineCode>.
      </p>,
    )
    expect(screen.getByText("demo-web").getAttribute("style")).toBeFalsy()
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
