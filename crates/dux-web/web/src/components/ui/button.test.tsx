// @vitest-environment jsdom
//
// jsdom resolves no CSS, so this is a class-contract test: what is pinned is
// which utilities a pressed toggle carries, and that none of them is one a
// hover can also produce.
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { Button } from "@/components/ui/button"

afterEach(cleanup)

// The utilities behind one state prefix, with the prefix (and a leading dark:)
// stripped, e.g. "bg-primary" for "dark:aria-pressed:bg-primary".
function under(className: string, prefix: string): Set<string> {
  return new Set(
    className
      .split(/\s+/)
      .map((c) => c.replace(/^dark:/, ""))
      .filter((c) => c.startsWith(prefix))
      .map((c) => c.slice(prefix.length)),
  )
}

describe("Button's toggle look", () => {
  it("fills a pressed outline toggle with the primary token, border and text included", () => {
    render(
      <Button variant="outline" toggle aria-pressed>
        File
      </Button>,
    )
    const pressed = under(screen.getByRole("button").className, "aria-pressed:")
    expect(pressed).toContain("bg-primary")
    expect(pressed).toContain("border-primary")
    expect(pressed).toContain("text-primary-foreground")
  })

  it("gives the pressed state no fill or text colour a hover also produces", () => {
    render(
      <Button variant="outline" toggle aria-pressed>
        File
      </Button>,
    )
    const className = screen.getByRole("button").className
    const pressed = under(className, "aria-pressed:")
    const hover = under(className, "hover:")
    const colours = (set: Set<string>) =>
      [...set].filter((c) => c.startsWith("bg-") || c.startsWith("text-"))
    expect(colours(pressed).length).toBeGreaterThan(0)
    for (const utility of colours(pressed)) {
      expect(hover.has(utility), utility).toBe(false)
    }
  })

  // Opt-in, so a button that carries aria-pressed for its own reasons (the
  // Theater toggle, whose pressed state is never on screen where it sits) keeps
  // the look it was designed with.
  it("adds no pressed look unless the button asks to be a toggle", () => {
    render(
      <Button variant="outline" aria-pressed>
        Theater
      </Button>,
    )
    expect(under(screen.getByRole("button").className, "aria-pressed:").size).toBe(0)
  })
})
