// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

// The real tooltip mounts its popup into a portal on hover only; the stub puts
// the content beside the trigger so the test can read what the hover would say.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: React.ReactNode
    content: React.ReactNode
  }) => (
    <>
      {children}
      <span data-testid="tooltip-content">{content}</span>
    </>
  ),
}))

const { StartTruncatedText } = await import("./StartTruncatedText")

afterEach(cleanup)

// jsdom does no layout, so where the ellipsis lands cannot be measured here;
// what is pinned is the structure that puts it at the start: an rtl box that
// clips and ellipsizes, holding the text as one LTR isolate so a leading "/" or
// "." stays where it was written, and left-aligned so a short text sits where
// an ordinary one would. The pixel truth is the screenshot pass.
describe("StartTruncatedText", () => {
  const PATH = "/home/patrick/Golang/src/github.com/patrickdappollonio/dux"

  it("ellipsizes at the start of the text, not the end", () => {
    render(<StartTruncatedText text={PATH} />)
    const box = screen.getByText(PATH).parentElement!
    expect(box.getAttribute("data-truncate")).toBe("start")
    expect(box.className).toContain("truncate")
    expect(box.className).toContain("[direction:rtl]")
    expect(box.className).toContain("text-left")
    expect(box.className).toContain("min-w-0")
  })

  it("keeps the text one LTR isolate, so a leading slash stays leading", () => {
    render(<StartTruncatedText text={PATH} />)
    const isolate = screen.getByText(PATH)
    expect(isolate.tagName).toBe("BDI")
    expect(isolate.getAttribute("dir")).toBe("ltr")
    // The whole text is in the DOM: the clipping is visual only, so a screen
    // reader always hears all of it.
    expect(isolate.textContent).toBe(PATH)
  })

  it("adds the caller's classes to the box", () => {
    render(<StartTruncatedText text={PATH} className="font-mono text-xs" />)
    const box = screen.getByText(PATH).parentElement!
    expect(box.className).toContain("font-mono")
    expect(box.className).toContain("text-xs")
  })

  it("shows the whole text in the shared tooltip when asked", () => {
    render(<StartTruncatedText text={PATH} tooltip />)
    expect(screen.getByTestId("tooltip-content").textContent).toBe(PATH)
  })

  it("has no tooltip unless asked", () => {
    render(<StartTruncatedText text={PATH} />)
    expect(screen.queryByTestId("tooltip-content")).toBeNull()
  })
})
