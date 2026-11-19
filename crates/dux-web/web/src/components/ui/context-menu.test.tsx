// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// An open context menu closes on Back through the store, so the real store is
// loaded over a history that has an entry for it.
installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "./context-menu"

afterEach(cleanup)

describe("context menu row touch height", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("gives every kind of row the dropdown rows' heights: 44px at phone width, 40px under a coarse pointer at desktop width", async () => {
    render(
      <ContextMenu>
        <ContextMenuTrigger>target</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem>Item</ContextMenuItem>
          <ContextMenuCheckboxItem checked>Checkbox</ContextMenuCheckboxItem>
          <ContextMenuRadioGroup value="a">
            <ContextMenuRadioItem value="a">Radio</ContextMenuRadioItem>
          </ContextMenuRadioGroup>
          <ContextMenuSub>
            <ContextMenuSubTrigger>More</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem>Deep item</ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
        </ContextMenuContent>
      </ContextMenu>,
    )
    fireEvent.contextMenu(screen.getByText("target"))
    await screen.findByRole("menu")
    for (const slot of ["item", "checkbox-item", "radio-item", "sub-trigger"]) {
      const classes = Array.from(
        document.querySelector(`[data-slot="context-menu-${slot}"]`)?.classList ?? [],
      )
      expect(
        classes,
        `${slot}: 44px below md, 40px for a finger at md and up, compact only for a fine pointer at md and up`,
      ).toEqual(
        expect.arrayContaining([
          "min-h-11",
          "md:pointer-coarse:min-h-10",
          "md:not-pointer-coarse:min-h-0",
        ]),
      )
    }
  })
})
