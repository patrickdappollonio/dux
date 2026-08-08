// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "./dropdown-menu"

// The responsive presentation contract of the shared menu primitive: at md+
// every menu is the anchored popup it has always been, and under md it is a
// full-width bottom sheet (backdrop, internal scroll, drill-down submenus).
// The split is driven by useIsMobile, whose snapshot reads window.innerWidth,
// so these tests flip the presentation by resizing jsdom's window.

// useIsMobile's resize subscription wants matchMedia, which jsdom lacks; the
// hook degrades to no subscription, and the snapshot still reads innerWidth.
// The stub here only exercises the "matchMedia exists" subscribe path too.
function stubMatchMedia() {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}

function setViewportWidth(width: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  })
}

const settle = () => new Promise((r) => setTimeout(r, 40))

function renderMenu() {
  return render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <DropdownMenuItem>Alpha</DropdownMenuItem>
        <DropdownMenuItem>Beta</DropdownMenuItem>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>More</DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            <DropdownMenuItem>Deep item</DropdownMenuItem>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      </DropdownMenuContent>
    </DropdownMenu>,
  )
}

async function openRoot() {
  fireEvent.click(screen.getByText("open"))
  await screen.findByRole("menu")
}

const popupEl = () =>
  document.querySelector('[data-slot="dropdown-menu-content"]') as HTMLElement
const backdropEl = () =>
  document.querySelector('[data-slot="dropdown-menu-backdrop"]')
const subContentEl = () =>
  document.querySelector('[data-slot="dropdown-menu-sub-content"]')
const backRowEl = () =>
  document.querySelector('[data-slot="dropdown-menu-back"]')

beforeEach(() => {
  stubMatchMedia()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("DropdownMenuContent on desktop (innerWidth 1280)", () => {
  beforeEach(() => setViewportWidth(1280))

  it("renders the anchored popup with no sheet chrome", async () => {
    renderMenu()
    await openRoot()
    const popup = popupEl()
    // The anchored popup's signature classes, untouched by the mobile work.
    expect(popup.className).toContain("origin-(--transform-origin)")
    expect(popup.className).toContain("max-h-(--available-height)")
    expect(popup.className).not.toContain("max-h-[85dvh]")
    // No backdrop on desktop: the popup is anchored, not a sheet.
    expect(backdropEl()).toBeNull()
    // The positioner keeps base-ui's computed placement (no bottom pin).
    const positioner = popup.parentElement as HTMLElement
    expect(positioner.style.bottom).not.toBe("0px")
  })

  it("renders submenus as nested popups without a Back row", async () => {
    renderMenu()
    await openRoot()
    fireEvent.click(screen.getByText("More"))
    await settle()
    expect(subContentEl()).not.toBeNull()
    expect(backRowEl()).toBeNull()
    expect(screen.getByText("Deep item")).toBeTruthy()
  })
})

describe("DropdownMenuContent on mobile (innerWidth 500)", () => {
  beforeEach(() => setViewportWidth(500))

  it("renders as a full-width bottom sheet over a backdrop", async () => {
    renderMenu()
    await openRoot()
    const popup = popupEl()
    // Full width, bottom-slide, top-rounded: the sheet look.
    expect(popup.className).toContain("w-full")
    expect(popup.className).toContain("rounded-t-2xl")
    expect(popup.className).toContain("data-open:slide-in-from-bottom")
    // motion-reduce drops the slide (the whole enter/exit animation).
    expect(popup.className).toContain("motion-reduce:animate-none!")
    // The backdrop covers the uncovered top gap; tapping it is an outside
    // press, which is what dismisses (no click handler needed on it).
    expect(backdropEl()).not.toBeNull()
    // The positioner is pinned to the bottom edge via the supported style
    // prop override, not left to the anchored placement.
    const positioner = popup.parentElement as HTMLElement
    expect(positioner.style.position).toBe("fixed")
    expect(positioner.style.bottom).toBe("0px")
    expect(positioner.style.top).toBe("auto")
    expect(positioner.style.left).toBe("0px")
    expect(positioner.style.right).toBe("0px")
    expect(positioner.style.transform).toBe("none")
  })

  it("caps the sheet height and scrolls internally", async () => {
    renderMenu()
    await openRoot()
    const popup = popupEl()
    expect(popup.className).toContain("max-h-[85dvh]")
    expect(popup.className).toContain("overflow-y-auto")
    expect(popup.className).toContain("overscroll-contain")
  })

  it("keeps menu semantics: items are menuitems and Escape closes", async () => {
    renderMenu()
    await openRoot()
    expect(
      screen.getAllByRole("menuitem").map((el) => el.textContent),
    ).toContain("Alpha")
    fireEvent.keyDown(popupEl(), { key: "Escape" })
    await settle()
    expect(screen.queryByRole("menu")).toBeNull()
  })

  it("drills into a submenu as a stacked sheet with a Back row on top", async () => {
    renderMenu()
    await openRoot()
    fireEvent.click(screen.getByText("More"))
    await settle()
    const sub = subContentEl() as HTMLElement
    expect(sub).not.toBeNull()
    // The sub-sheet is the same full-width sheet presentation.
    expect(sub.className).toContain("w-full")
    expect(sub.className).toContain("rounded-t-2xl")
    // The Back row leads the sheet and is a real menu item.
    const back = backRowEl() as HTMLElement
    expect(back).not.toBeNull()
    expect(back.getAttribute("role")).toBe("menuitem")
    expect(sub.firstElementChild).toBe(back)
    expect(screen.getByText("Deep item")).toBeTruthy()
  })

  it("returns to the parent sheet when Back is tapped", async () => {
    renderMenu()
    await openRoot()
    fireEvent.click(screen.getByText("More"))
    await settle()
    fireEvent.click(backRowEl() as HTMLElement)
    await settle()
    // The submenu closed; the parent sheet is still up.
    expect(subContentEl()).toBeNull()
    expect(screen.getByText("Alpha")).toBeTruthy()
  })

  it("closes the whole tree when a submenu item is selected", async () => {
    renderMenu()
    await openRoot()
    fireEvent.click(screen.getByText("More"))
    await settle()
    fireEvent.click(screen.getByText("Deep item"))
    await settle()
    expect(screen.queryByRole("menu")).toBeNull()
  })
})
