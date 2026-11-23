// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { DropdownMenu, DropdownMenuContent, DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { REVEAL_ROW } from "@/lib/hoverReveal"

import { EllipsisMenuTrigger } from "./EllipsisMenuTrigger"

afterEach(cleanup)

const renderTrigger = (props: Partial<React.ComponentProps<typeof EllipsisMenuTrigger>> = {}) =>
  render(
    <div className={REVEAL_ROW} data-testid="row">
      <DropdownMenu>
        <EllipsisMenuTrigger placement="row" aria-label="Row actions" {...props} />
        <DropdownMenuContent>
          <DropdownMenuItem>Open</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>,
  )

describe("EllipsisMenuTrigger", () => {
  it("opens its menu, under the name it was given, with horizontal dots", () => {
    renderTrigger()
    const trigger = screen.getByRole("button", { name: "Row actions" })
    expect(trigger.querySelector("svg.lucide-ellipsis")).not.toBeNull()
    fireEvent.click(trigger)
    expect(screen.getByRole("menuitem", { name: "Open" })).toBeTruthy()
  })

  it("takes no room while idle, and is always there under a coarse pointer, when it reveals on hover", () => {
    renderTrigger({ reveal: "hover-or-focus" })
    const wrapper = screen.getByRole("button", { name: "Row actions" }).parentElement!
    const classes = wrapper.className.split(/\s+/)
    expect(classes).toContain("max-w-0")
    expect(classes).toContain("opacity-0")
    expect(classes).toContain("has-[[data-popup-open]]:max-w-(--reveal-cap)")
    expect(classes).toContain("pointer-coarse:max-w-none")
    expect(classes).toContain("pointer-coarse:opacity-100")
  })

  it("is not wrapped when it is always painted", () => {
    renderTrigger()
    expect(screen.getByRole("button", { name: "Row actions" }).parentElement).toBe(
      screen.getByTestId("row"),
    )
  })

  it("keeps a pill's press off the pill, and a busy one takes none", () => {
    const pillClick = vi.fn()
    const { unmount } = render(
      <div onClick={pillClick}>
        <DropdownMenu>
          <EllipsisMenuTrigger placement="tab-pill" aria-label="Tab actions" />
          <DropdownMenuContent />
        </DropdownMenu>
      </div>,
    )
    fireEvent.click(screen.getByRole("button", { name: "Tab actions" }))
    expect(pillClick).not.toHaveBeenCalled()
    unmount()

    renderTrigger({ busy: true })
    const busy = screen.getByRole("button", { name: "Row actions" }) as HTMLButtonElement
    expect(busy.getAttribute("aria-busy")).toBe("true")
    expect(busy.disabled).toBe(true)
    expect(busy.querySelector("svg.lucide-ellipsis")).toBeNull()
  })
})
