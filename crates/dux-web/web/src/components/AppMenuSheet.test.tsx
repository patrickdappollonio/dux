// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

const openCustomizeWebapp = vi.fn()
const sortAgents = vi.fn()
vi.mock("@/lib/store", () => ({
  openCustomizeWebapp: () => openCustomizeWebapp(),
  openConfigEditor: vi.fn(),
  openMacrosDialog: vi.fn(),
  openGlobalEnv: vi.fn(),
  openTaskManager: vi.fn(),
  sortAgents: (by: string) => sortAgents(by),
}))
vi.mock("@/lib/configApi", () => ({
  configApi: { reload: () => Promise.resolve() },
}))

import { AppMenuSheet } from "@/components/AppMenuSheet"
import { appMenuModel, findSubmenu } from "@/lib/appMenu"

const settle = () => new Promise((r) => setTimeout(r, 40))

afterEach(() => cleanup())

function Harness({ onOpenChange = vi.fn() }: { onOpenChange?: () => void }) {
  return <AppMenuSheet open onOpenChange={onOpenChange} />
}

describe("AppMenuSheet", () => {
  beforeEach(() => vi.clearAllMocks())

  // THE ANTI-DRIFT TEST. Both presentations render from one model, so the mobile
  // sheet's root list must match the desktop menu's top level exactly.
  it("renders the same top-level titles as the desktop menu", () => {
    render(<Harness />)
    const expected = appMenuModel()
      .filter((e) => e.kind !== "separator")
      .map((e) => (e.kind === "separator" ? "" : e.title))
    const rendered = screen.getAllByRole("menuitem").map((e) => e.textContent)
    expect(rendered).toHaveLength(expected.length)
    for (const title of expected) {
      expect(rendered.some((t) => t?.includes(title))).toBe(true)
    }
  })

  it("shows the root title and no back arrow at the root", () => {
    render(<Harness />)
    expect(screen.getByText("Menu")).toBeTruthy()
    expect(screen.queryByLabelText("Back")).toBeNull()
  })

  it("drills down into Configuration and shows a back arrow", async () => {
    render(<Harness />)
    fireEvent.click(screen.getByText("Configuration"))
    await settle()
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByText("Edit config file…")).toBeTruthy()
    // The root's entries are gone: this is a drill-down, not an expansion.
    expect(screen.queryByText("Preferences…")).toBeNull()
  })

  it("gives the back button the 44px primary hub-control size, not the 40px dense floor", async () => {
    render(<Harness />)
    fireEvent.click(screen.getByText("Configuration"))
    await settle()
    const back = screen.getByLabelText("Back")
    expect(back.className).toContain("size-11")
    expect(back.className).not.toContain("size-10")
  })

  it("shows the submenu title as the drilled-down header", async () => {
    render(<Harness />)
    fireEvent.click(screen.getByText("Sort agents by"))
    await settle()
    const sub = findSubmenu(appMenuModel(), "sort-agents")!
    expect(screen.getAllByText(sub.title).length).toBeGreaterThan(0)
    expect(screen.getByText("Recently updated")).toBeTruthy()
  })

  it("returns to the root list from the back arrow", async () => {
    render(<Harness />)
    fireEvent.click(screen.getByText("Configuration"))
    await settle()
    fireEvent.click(screen.getByLabelText("Back"))
    await settle()
    expect(screen.getByText("Preferences…")).toBeTruthy()
    expect(screen.queryByLabelText("Back")).toBeNull()
  })

  it("resets to the root when reopened", async () => {
    const { rerender } = render(<AppMenuSheet open onOpenChange={vi.fn()} />)
    fireEvent.click(screen.getByText("Configuration"))
    await settle()
    expect(screen.getByLabelText("Back")).toBeTruthy()

    rerender(<AppMenuSheet open={false} onOpenChange={vi.fn()} />)
    await settle()
    rerender(<AppMenuSheet open onOpenChange={vi.fn()} />)
    await settle()
    expect(screen.getByText("Preferences…")).toBeTruthy()
    expect(screen.queryByLabelText("Back")).toBeNull()
  })

  it("gives every row at least a 44px touch target", () => {
    render(<Harness />)
    for (const row of screen.getAllByRole("menuitem")) {
      expect(row.className, row.textContent ?? "").toContain("min-h-11")
    }
  })

  it("runs the item action and closes the sheet", () => {
    const onOpenChange = vi.fn()
    render(<AppMenuSheet open onOpenChange={onOpenChange} />)
    fireEvent.click(screen.getByText("Preferences…"))
    expect(openCustomizeWebapp).toHaveBeenCalledOnce()
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  it("runs a drilled-down item and closes the sheet", async () => {
    const onOpenChange = vi.fn()
    render(<AppMenuSheet open onOpenChange={onOpenChange} />)
    fireEvent.click(screen.getByText("Sort agents by"))
    await settle()
    fireEvent.click(screen.getByText("Created"))
    expect(sortAgents).toHaveBeenCalledWith("created")
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  // The sheet is a hand-rolled list inside a dialog, so unlike the desktop
  // flyout (where base-ui supplies it) we own the ARIA.
  it("supplies menu semantics by hand", async () => {
    render(<Harness />)
    expect(screen.getByRole("menu")).toBeTruthy()
    const submenuRow = screen.getByText("Configuration").closest("button")!
    expect(submenuRow.getAttribute("aria-haspopup")).toBe("menu")
    expect(submenuRow.getAttribute("aria-expanded")).toBe("false")
    const plainRow = screen.getByText("Preferences…").closest("button")!
    expect(plainRow.getAttribute("aria-haspopup")).toBeNull()
  })

  it("renders every row with an icon", () => {
    render(<Harness />)
    for (const row of screen.getAllByRole("menuitem")) {
      expect(row.querySelector("svg"), row.textContent ?? "").toBeTruthy()
    }
  })
})
