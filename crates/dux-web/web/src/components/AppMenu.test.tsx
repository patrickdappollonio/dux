// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

const openCustomizeWebapp = vi.fn()
vi.mock("@/lib/store", () => ({
  openCustomizeWebapp: () => openCustomizeWebapp(),
  openConfigEditor: vi.fn(),
  openMacrosDialog: vi.fn(),
  openGlobalEnv: vi.fn(),
  openKillRunning: vi.fn(),
  sortAgents: vi.fn(),
}))
vi.mock("@/lib/configApi", () => ({
  configApi: { reload: () => Promise.resolve() },
}))

import { AppMenu } from "@/components/AppMenu"
import { appMenuModel, type AppMenuEntry } from "@/lib/appMenu"

function walk(entries: AppMenuEntry[]): AppMenuEntry[] {
  return entries.flatMap((e) =>
    e.kind === "submenu" ? [e, ...walk(e.entries)] : [e],
  )
}

const settle = () => new Promise((r) => setTimeout(r, 40))

afterEach(() => cleanup())

describe("AppMenu", () => {
  beforeEach(() => vi.clearAllMocks())

  it("renders a cog trigger labelled Menu", () => {
    render(<AppMenu />)
    expect(screen.getByRole("button", { name: /^menu$/i })).toBeTruthy()
  })

  // The trigger is a real <button>, so a browser fires its click on Enter/Space
  // natively. jsdom does NOT synthesize that click, so asserting "Enter opens
  // it" here would test jsdom, not our menu. What IS ours to prove: the trigger
  // is tab-reachable, and base-ui's own keyboard affordance (ArrowDown) opens
  // the menu and moves focus into it.
  it("is reachable by keyboard", () => {
    render(<AppMenu />)
    const trigger = screen.getByRole("button", { name: /^menu$/i })
    trigger.focus()
    expect(document.activeElement).toBe(trigger)
    expect(trigger.tagName).toBe("BUTTON")
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu")
  })

  it("opens on ArrowDown and moves focus into the menu", async () => {
    render(<AppMenu />)
    const trigger = screen.getByRole("button", { name: /^menu$/i })
    trigger.focus()
    fireEvent.keyDown(trigger, { key: "ArrowDown" })
    expect(await screen.findByRole("menu")).toBeTruthy()
    await settle()
    expect(document.activeElement?.getAttribute("data-slot")).toBe(
      "dropdown-menu-item",
    )
  })

  // Base UI does NOT set aria-expanded=true on an OPEN menu trigger (it stays
  // "false" and the trigger gains data-popup-open instead). CLAUDE.md documents
  // this, and it was reconfirmed empirically against base-ui 1.5.0 before this
  // test was written. Assert what the primitive actually emits.
  it("marks the open trigger with data-popup-open, not aria-expanded", async () => {
    render(<AppMenu />)
    const trigger = screen.getByRole("button", { name: /^menu$/i })
    fireEvent.click(trigger)
    await screen.findByRole("menu")
    expect(trigger.hasAttribute("data-popup-open")).toBe(true)
    expect(trigger.getAttribute("aria-controls")).toBeTruthy()
  })

  it("renders one menuitem per model entry", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")

    // Drive the expectation from the model, never a hand-written list.
    const topLevel = appMenuModel().filter((e) => e.kind !== "separator")
    const rendered = screen.getAllByRole("menuitem").map((e) => e.textContent)
    expect(rendered).toHaveLength(topLevel.length)
    for (const entry of topLevel) {
      if (entry.kind === "separator") continue
      expect(rendered.some((t) => t?.includes(entry.title))).toBe(true)
    }
  })

  it("marks submenu triggers with aria-haspopup", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")
    const subTriggers = document.querySelectorAll(
      '[data-slot="dropdown-menu-sub-trigger"]',
    )
    const submenus = appMenuModel().filter((e) => e.kind === "submenu")
    expect(subTriggers).toHaveLength(submenus.length)
    for (const t of subTriggers) {
      expect(t.getAttribute("aria-haspopup")).toBe("menu")
      expect(t.getAttribute("role")).toBe("menuitem")
    }
  })

  // The menu is anchored to the app's RIGHT edge, so a submenu opening to the
  // right would run off-screen. base-ui's positioner collision-flips, but the
  // flip needs real layout (jsdom has none), so we state the intent explicitly
  // rather than relying on it. This asserts the intent reaches the positioner.
  it("opens submenus to the left", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")
    fireEvent.click(
      document.querySelector('[data-slot="dropdown-menu-sub-trigger"]')!,
    )
    await settle()
    const subContent = document.querySelector(
      '[data-slot="dropdown-menu-sub-content"]',
    )
    expect(subContent?.getAttribute("data-side")).toBe("left")
  })

  it("expands a submenu and shows its children", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")
    const sortTrigger = screen.getByText("Sort agents by")
    fireEvent.click(sortTrigger)
    await settle()
    expect(screen.getByText("Recently updated")).toBeTruthy()
    expect(screen.getByText("Created")).toBeTruthy()
    expect(screen.getByText("Name")).toBeTruthy()
  })

  it("closes on Escape and returns focus to the trigger", async () => {
    render(<AppMenu />)
    const trigger = screen.getByRole("button", { name: /^menu$/i })
    fireEvent.click(trigger)
    await screen.findByRole("menu")
    fireEvent.keyDown(document.activeElement ?? document.body, {
      key: "Escape",
    })
    await settle()
    expect(screen.queryByRole("menu")).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it("calls openCustomizeWebapp when Preferences is chosen", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")
    fireEvent.click(screen.getByText("Preferences…"))
    expect(openCustomizeWebapp).toHaveBeenCalledOnce()
  })

  it("renders every non-separator entry with an icon", async () => {
    render(<AppMenu />)
    fireEvent.click(screen.getByRole("button", { name: /^menu$/i }))
    await screen.findByRole("menu")
    for (const item of screen.getAllByRole("menuitem")) {
      expect(item.querySelector("svg"), item.textContent ?? "").toBeTruthy()
    }
  })

  it("does not bind a keyboard shortcut to open itself", async () => {
    render(<AppMenu />)
    for (const init of [
      { key: "k", ctrlKey: true },
      { key: "k", metaKey: true },
    ]) {
      fireEvent.keyDown(window, init)
      await settle()
      expect(screen.queryByRole("menu")).toBeNull()
    }
  })

  it("walks the whole model without a submenu losing entries", () => {
    // Guards the recursion contract the renderer depends on.
    expect(walk(appMenuModel()).length).toBeGreaterThan(appMenuModel().length)
  })
})
