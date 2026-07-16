// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

const openAddProject = vi.fn()
const openAddProjectForInit = vi.fn()
vi.mock("@/lib/store", () => ({
  openAddProject: () => openAddProject(),
  openAddProjectForInit: () => openAddProjectForInit(),
  useDux: () => ({}),
}))

import { AddProjectSplitButton } from "@/components/AddProjectSplitButton"

afterEach(() => cleanup())

describe("AddProjectSplitButton", () => {
  it("applies the ButtonGroup seam so the second segment's inner corners are squared", () => {
    // Proof the group seam actually applied: the seam behavior lives in the
    // group component's own child selectors, so a trimmed or bypassed vendored
    // component silently loses the joint. Assert the group carries the
    // horizontal not(:first-child) selectors and the ⋯ trigger is a non-first
    // child they target.
    const { container } = render(<AddProjectSplitButton />)
    const group = container.querySelector('[data-slot="button-group"]')
    expect(group).toBeTruthy()
    expect(group!.className).toContain(
      "[&>*:not(:first-child)]:rounded-l-none",
    )
    expect(group!.className).toContain("[&>*:not(:last-child)]:rounded-r-none")
    const trigger = screen.getByRole("button", {
      name: /more ways to add a project/i,
    })
    const children = Array.from(group!.children)
    expect(children.indexOf(trigger)).toBeGreaterThan(0)
  })

  it("reveals the open state via data-popup-open, not aria-expanded (the tenet trap)", async () => {
    render(<AddProjectSplitButton />)
    const trigger = screen.getByRole("button", {
      name: /more ways to add a project/i,
    })
    fireEvent.click(trigger)
    await screen.findByRole("menu")
    expect(trigger.hasAttribute("data-popup-open")).toBe(true)
    // Base UI keeps aria-expanded=false on an open menu trigger; styling keyed
    // off it would silently never match.
    expect(trigger.getAttribute("aria-expanded")).not.toBe("true")
  })

  it("offers both add variants and routes them to the store actions", async () => {
    render(<AddProjectSplitButton />)
    fireEvent.click(
      screen.getByRole("button", { name: /more ways to add a project/i }),
    )
    await screen.findByRole("menu")
    fireEvent.click(screen.getByText("Initialize a repository…"))
    expect(openAddProjectForInit).toHaveBeenCalled()

    fireEvent.click(
      screen.getByRole("button", { name: /more ways to add a project/i }),
    )
    await screen.findByRole("menu")
    fireEvent.click(screen.getByText("Add project…"))
    expect(openAddProject).toHaveBeenCalled()
  })

  it("keeps the one-click primary Add project segment", () => {
    render(<AddProjectSplitButton />)
    fireEvent.click(screen.getByRole("button", { name: /^add project$/i }))
    expect(openAddProject).toHaveBeenCalled()
  })
})
