// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// An open menu closes on Back through the store, so the real store is loaded
// over a history that has an entry for it.
installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { MenuItemReason, MenuReasonsHead } from "@/components/MenuReason"
import { useMenuReason } from "@/hooks/use-menu-reason"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { ContextMenu, ContextMenuTrigger } from "@/components/ui/context-menu"
import { FileTreeContextMenu } from "@/components/FileTreeContextMenu"

afterEach(cleanup)

const SHARED = "This agent is active on another device."
const OWN = "Nothing selected can be discarded."

function Menu({ shared, own }: { shared: string | null; own: string | null }) {
  const head = useMenuReason(shared)
  const ownReason = useMenuReason(own)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <MenuReasonsHead reasons={[head]} />
        <DropdownMenuItem disabled={shared !== null} aria-describedby={head.describedBy}>
          Commit…
        </DropdownMenuItem>
        <DropdownMenuItem disabled={shared !== null} aria-describedby={head.describedBy}>
          Push
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={own !== null}
          aria-describedby={ownReason.describedBy}
        >
          Discard…
        </DropdownMenuItem>
        <MenuItemReason reason={ownReason} />
        <DropdownMenuItem>Refresh</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

async function open() {
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

const item = (text: string) => screen.getByText(text).closest('[role="menuitem"]') as HTMLElement
const reasons = () => Array.from(document.querySelectorAll('[data-slot="menu-reason"]'))
const description = (el: HTMLElement) =>
  document.getElementById(el.getAttribute("aria-describedby") ?? "")?.textContent ?? null

describe("a disabled menu item's reason", () => {
  it("says a shared reason once, at the top, set apart, and every item it greys names it", async () => {
    render(<Menu shared={SHARED} own={null} />)
    const menu = await open()
    expect(reasons().map((r) => r.textContent)).toEqual([SHARED])
    expect(description(item("Commit…"))).toBe(SHARED)
    expect(description(item("Push"))).toBe(SHARED)
    // The first thing in the menu, then a separator before the rows.
    const head = reasons()[0]!.parentElement!
    expect(menu.firstElementChild).toBe(head)
    expect(head.nextElementSibling?.getAttribute("role")).toBe("separator")
    // Capped where the menu is anchored and sized to its widest row; the phone
    // sheet is the screen's width and the sentence spans it.
    expect(reasons()[0]!.className).toContain("md:max-w-64")
  })

  it("says a reason only one item has right beneath that item", async () => {
    render(<Menu shared={null} own={OWN} />)
    await open()
    expect(reasons().map((r) => r.textContent)).toEqual([OWN])
    const discard = item("Discard…")
    expect(description(discard)).toBe(OWN)
    expect(discard.nextElementSibling?.contains(reasons()[0]!)).toBe(true)
    expect(item("Commit…").hasAttribute("aria-describedby")).toBe(false)
  })

  it("adds nothing to a menu with no reason", async () => {
    render(<Menu shared={null} own={null} />)
    const menu = await open()
    expect(reasons()).toEqual([])
    expect(menu.querySelector('[role="separator"]')).toBeNull()
    expect(menu.firstElementChild).toBe(item("Commit…"))
  })
})

describe("a context menu that carries a reason", () => {
  function setViewportWidth(width: number) {
    Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width })
  }
  afterEach(() => setViewportWidth(1024))

  function renderTreeMenu() {
    const noop = () => {}
    render(
      <ContextMenu>
        <ContextMenuTrigger>row</ContextMenuTrigger>
        <FileTreeContextMenu
          variant="file"
          onNewFile={noop}
          onNewFolder={noop}
          onRename={noop}
          onMove={noop}
          onDelete={noop}
          onInfo={noop}
          writeBlocked={SHARED}
        />
      </ContextMenu>,
    )
    fireEvent.contextMenu(screen.getByText("row"))
  }

  it("is the bottom card on a phone, with the reason in it", async () => {
    setViewportWidth(500)
    renderTreeMenu()
    const menu = await screen.findByRole("menu")
    const positioner = menu.parentElement as HTMLElement
    expect(positioner.style.position).toBe("fixed")
    expect(positioner.style.bottom).toBe("0px")
    expect(menu.className).toContain("rounded-t-2xl")
    expect(document.querySelector('[data-slot="context-menu-backdrop"]')).not.toBeNull()
    expect(menu.contains(reasons()[0]!)).toBe(true)
    expect(description(item("Rename…"))).toBe(SHARED)
  })

  it("stays anchored where it was opened on a computer", async () => {
    setViewportWidth(1280)
    renderTreeMenu()
    const menu = await screen.findByRole("menu")
    expect((menu.parentElement as HTMLElement).style.bottom).not.toBe("0px")
    expect(document.querySelector('[data-slot="context-menu-backdrop"]')).toBeNull()
  })
})
