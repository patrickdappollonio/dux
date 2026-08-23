// @vitest-environment jsdom
//
// jsdom doesn't compute layout/resolved CSS, so this is a class-contract test:
// it asserts the popup and arrow carry the popover surface tokens (matching
// DropdownMenuContent) and never an inverted bg-foreground/text-background
// pair, which makes a white tooltip in this dark-only app.
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"

afterEach(() => {
  cleanup()
})

describe("TooltipContent", () => {
  it("uses the popover surface tokens, not the inverted bg-foreground/text-background pair", () => {
    render(
      <TooltipProvider delay={0}>
        <Tooltip open>
          <TooltipTrigger render={<button type="button">trigger</button>} />
          <TooltipContent>hello</TooltipContent>
        </Tooltip>
      </TooltipProvider>,
    )
    const popup = screen.getByText("hello")
    expect(popup.className).toContain("bg-popover")
    expect(popup.className).toContain("text-popover-foreground")
    expect(popup.className).not.toContain("bg-foreground")
    expect(popup.className).not.toContain("text-background")
  })
})
