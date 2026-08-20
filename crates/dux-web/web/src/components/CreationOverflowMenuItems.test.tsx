// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

let ghAvailable = true
vi.mock("@/lib/store", () => ({
  openNewAgentPicker: vi.fn(),
  openAddProject: vi.fn(),
  openAddProjectForInit: vi.fn(),
  openCreateAgentFromPr: vi.fn(),
  createStandaloneTerminal: vi.fn(),
  useDux: () => ({ bootstrap: { gh_available: ghAvailable } }),
}))

import { CreationOverflowMenuItems } from "@/components/CreationOverflowMenuItems"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Button } from "@/components/ui/button"

// The component is a menu BODY, so it needs a real menu around it: base-ui's
// GroupLabel throws outside a Menu.Group and a Group outside a menu.
function Host() {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button aria-label="open">⋯</Button>} />
      <DropdownMenuContent>
        <CreationOverflowMenuItems />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

const openMenu = async () => {
  render(<Host />)
  fireEvent.click(screen.getByRole("button", { name: "open" }))
  return await screen.findByRole("menu")
}

// The menu's own children, in order, by what each one is.
function rowSlots(menu: HTMLElement): string[] {
  return Array.from(menu.children).map(
    (el) => el.getAttribute("data-slot") ?? "",
  )
}

beforeEach(() => {
  ghAvailable = true
  vi.clearAllMocks()
})
afterEach(() => cleanup())

describe("CreationOverflowMenuItems", () => {
  it("labels the three groups and lists every creation variant under them", async () => {
    const menu = await openMenu()
    expect(
      Array.from(menu.querySelectorAll('[data-slot="dropdown-menu-label"]')).map(
        (el) => el.textContent,
      ),
    ).toEqual(["Agents", "Terminals", "Projects"])
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "New agent from PR…",
      "New agent from existing worktree…",
      "New standalone agent…",
      "New standalone terminal",
      "Add project…",
      "Initialize a repository…",
    ])
  })

  // base-ui's GroupLabel THROWS outside a Menu.Group, so the parent is not
  // decoration: every label must sit inside one.
  it("wraps every label in its own group", async () => {
    const menu = await openMenu()
    const groups = menu.querySelectorAll('[data-slot="dropdown-menu-group"]')
    expect(groups).toHaveLength(3)
    for (const group of groups) {
      expect(
        group.querySelector('[data-slot="dropdown-menu-label"]'),
      ).toBeTruthy()
    }
  })

  it("rules BETWEEN the groups and never at an edge", async () => {
    const menu = await openMenu()
    const slots = rowSlots(menu)
    expect(
      slots.filter((s) => s === "dropdown-menu-separator"),
    ).toHaveLength(2)
    // A dangling rule at either edge (or two in a row) is what a gate hiding a
    // whole group would produce if the renderer drew rules blindly.
    expect(slots[0]).toBe("dropdown-menu-group")
    expect(slots[slots.length - 1]).toBe("dropdown-menu-group")
    for (let i = 1; i < slots.length; i++) {
      expect(
        slots[i] === "dropdown-menu-separator" &&
          slots[i - 1] === "dropdown-menu-separator",
      ).toBe(false)
    }
  })

  it("keeps a labeled Agents group when gh is unavailable", async () => {
    // The from-PR variant is the only gated row, so the group it lives in
    // shrinks but never empties. A heading with nothing under it would be a
    // dangling word.
    ghAvailable = false
    const menu = await openMenu()
    expect(
      Array.from(menu.querySelectorAll('[data-slot="dropdown-menu-label"]')).map(
        (el) => el.textContent,
      ),
    ).toEqual(["Agents", "Terminals", "Projects"])
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "New agent from existing worktree…",
      "New standalone agent…",
      "New standalone terminal",
      "Add project…",
      "Initialize a repository…",
    ])
    expect(
      rowSlots(menu).filter((s) => s === "dropdown-menu-separator"),
    ).toHaveLength(2)
  })

  it("gives every row a leading icon", async () => {
    await openMenu()
    for (const item of screen.getAllByRole("menuitem")) {
      expect(item.querySelector("svg")).toBeTruthy()
    }
  })
})
