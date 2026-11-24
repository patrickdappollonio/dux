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

// The finger that opens the phone's card is often still over one of its items
// when it lifts, so that touch must choose nothing (lib/openingTouch.ts, which
// pins the rule on its own). Here the card itself is wired to it.
describe("the phone's card opened by a long press", () => {
  function setViewportWidth(width: number) {
    Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width })
  }
  afterEach(() => setViewportWidth(1024))

  function touch(type: "touchstart" | "touchend", touches: number) {
    const event = new Event(type, { bubbles: true, cancelable: true })
    Object.defineProperty(event, "touches", { value: new Array(touches).fill({}) })
    document.body.dispatchEvent(event)
    return event
  }

  function renderCard(width: number) {
    setViewportWidth(width)
    const onPick = vi.fn()
    render(
      <ContextMenu>
        <ContextMenuTrigger>target</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onClick={onPick}>Rename…</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>,
    )
    return onPick
  }

  it("chooses nothing on the lift of the touch that opened it, and the next tap chooses", async () => {
    const onPick = renderCard(500)
    // The finger goes down, and the menu opens while it is still down.
    touch("touchstart", 1)
    fireEvent.contextMenu(screen.getByText("target"))
    const item = (await screen.findByText("Rename…")).closest('[role="menuitem"]') as HTMLElement

    expect(touch("touchend", 0).defaultPrevented).toBe(true)
    fireEvent.mouseUp(item)
    fireEvent.click(item)
    expect(onPick).not.toHaveBeenCalled()
    expect(screen.queryByRole("menu")).not.toBeNull()

    touch("touchstart", 1)
    touch("touchend", 0)
    fireEvent.click(item)
    expect(onPick).toHaveBeenCalledTimes(1)
  })

  it("lets a mouse right-click on the same layout choose at once", async () => {
    const onPick = renderCard(500)
    fireEvent.contextMenu(screen.getByText("target"))
    const item = (await screen.findByText("Rename…")).closest('[role="menuitem"]') as HTMLElement
    fireEvent.click(item)
    expect(onPick).toHaveBeenCalledTimes(1)
  })

  it("does not guard the anchored popup a computer gets", async () => {
    const onPick = renderCard(1280)
    touch("touchstart", 1)
    fireEvent.contextMenu(screen.getByText("target"))
    const item = (await screen.findByText("Rename…")).closest('[role="menuitem"]') as HTMLElement
    expect(touch("touchend", 0).defaultPrevented).toBe(false)
    fireEvent.click(item)
    expect(onPick).toHaveBeenCalledTimes(1)
  })
})

describe("where focus goes when a context menu closes", () => {
  afterEach(() => {
    document.querySelectorAll('[role="dialog"]').forEach((el) => el.remove())
  })

  function renderWithFinalFocus() {
    render(
      <>
        <button type="button">elsewhere</button>
        <ContextMenu>
          <ContextMenuTrigger>target</ContextMenuTrigger>
          <ContextMenuContent finalFocus={() => screen.getByText("elsewhere")}>
            <ContextMenuItem>Rename…</ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      </>,
    )
    fireEvent.contextMenu(screen.getByText("target"))
  }

  it("goes to the named element when nothing else opened", async () => {
    renderWithFinalFocus()
    await screen.findByRole("menu")
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    await vi.waitFor(() => expect(document.activeElement).toBe(screen.getByText("elsewhere")))
  })

  it("stays where a dialog opened over the menu took it, as a dropdown's does", async () => {
    renderWithFinalFocus()
    await screen.findByRole("menu")
    const dialog = document.createElement("div")
    dialog.setAttribute("role", "dialog")
    const field = document.createElement("input")
    dialog.append(field)
    document.body.append(dialog)
    field.focus()
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    await vi.waitFor(() => expect(screen.queryByRole("menu")).toBeNull())
    expect(document.activeElement).toBe(field)
  })
})
