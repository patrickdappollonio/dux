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
})
