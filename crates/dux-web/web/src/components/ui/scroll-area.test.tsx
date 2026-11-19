// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render } from "@testing-library/react"

import { ScrollArea } from "./scroll-area"

afterEach(cleanup)

describe("scroll area focus ring", () => {
  // jsdom has no layout: a browser check must Tab onto a scroll area inside an
  // overflow-hidden card and see the whole ring on all four sides.
  it("draws the viewport's ring inside its own box, where no ancestor can clip it", () => {
    render(<ScrollArea>content</ScrollArea>)
    const viewport = document.querySelector('[data-slot="scroll-area-viewport"]')!
    expect(Array.from(viewport.classList)).toEqual(
      expect.arrayContaining(["focus-visible:ring-[3px]", "focus-visible:ring-inset"]),
    )
  })

  // A browser check: in Preferences, Tab to a control below the visible part
  // (the Teal swatch, "Reset section to defaults…") and Shift-Tab back up past
  // the top; the control's 3px ring is whole at the bottom and at the top.
  it("keeps a keyboard-focused control's ring in view by scroll padding on the scrolling element", () => {
    render(<ScrollArea>content</ScrollArea>)
    const viewport = document.querySelector('[data-slot="scroll-area-viewport"]')!
    expect(viewport.classList).toContain("scroll-py-1")
  })
})
