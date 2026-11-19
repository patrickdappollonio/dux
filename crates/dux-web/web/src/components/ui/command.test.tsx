// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

import { Command, CommandItem, CommandList } from "./command"

// cmdk measures its list, which jsdom has no observer for.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
)
Element.prototype.scrollIntoView = () => {}

afterEach(cleanup)

describe("command row touch height", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("gives each row 44px at phone width and 40px under a coarse pointer at desktop width", () => {
    render(
      <Command>
        <CommandList>
          <CommandItem>Alpha</CommandItem>
        </CommandList>
      </Command>,
    )
    expect(
      Array.from(document.querySelector('[data-slot="command-item"]')?.classList ?? []),
      "44px below md, 40px for a finger at md and up, compact only for a fine pointer at md and up",
    ).toEqual(
      expect.arrayContaining([
        "min-h-11",
        "md:pointer-coarse:min-h-10",
        "md:not-pointer-coarse:min-h-0",
      ]),
    )
  })
})
