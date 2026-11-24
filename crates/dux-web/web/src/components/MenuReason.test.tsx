// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"
import { menuLayout, separatorProblems } from "@/test/menuLayout"

// An open menu closes on Back through the store, so the real store is loaded
// over a history that has an entry for it.
installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { MenuGroupReason, MenuItemReason, MenuReasonsHead } from "@/components/MenuReason"
import { useMenuReason } from "@/hooks/use-menu-reason"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
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

// Where a reason sits is what it covers, read from the rendered order and not
// from classes: the whole menu is one line first, a group is a line directly
// above its first item, and one item's reason is directly beneath it.
describe("where a reason sits", () => {
  const WHOLE = "Everything here is read-only."
  const GROUP = "Nothing is staged."
  const ITEM = "No branch to push."
  const OTHER_GROUP = "No repository."

  function Placed({
    whole = null,
    group = null,
    other = null,
    item = null,
    leadingRow = true,
  }: {
    whole?: string | null
    group?: string | null
    other?: string | null
    item?: string | null
    leadingRow?: boolean
  }) {
    const wholeReason = useMenuReason(whole)
    const groupReason = useMenuReason(group)
    const otherReason = useMenuReason(other)
    const itemReason = useMenuReason(item)
    return (
      <DropdownMenu>
        <DropdownMenuTrigger>open</DropdownMenuTrigger>
        <DropdownMenuContent>
          <MenuReasonsHead reasons={[wholeReason]} />
          {leadingRow ? <DropdownMenuItem>Open</DropdownMenuItem> : null}
          <MenuGroupReason reasons={[groupReason]} apart={leadingRow} />
          <DropdownMenuItem disabled={group !== null} aria-describedby={groupReason.describedBy}>
            Commit
          </DropdownMenuItem>
          <DropdownMenuItem disabled={group !== null} aria-describedby={groupReason.describedBy}>
            Push
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <MenuGroupReason reasons={[otherReason]} />
          <DropdownMenuItem disabled={other !== null} aria-describedby={otherReason.describedBy}>
            Pull
          </DropdownMenuItem>
          <DropdownMenuItem disabled={item !== null} aria-describedby={itemReason.describedBy}>
            Refresh
          </DropdownMenuItem>
          <MenuItemReason reason={itemReason} />
          <DropdownMenuItem>Hide</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  async function layoutOf(ui: React.ReactElement) {
    render(ui)
    return menuLayout(await open())
  }

  it("puts a whole-menu reason first, set apart from the rows", async () => {
    const layout = await layoutOf(<Placed whole={WHOLE} />)
    expect(layout.slice(0, 3)).toEqual([`reason: ${WHOLE}`, "---", "Open"])
    expect(separatorProblems(layout)).toEqual([])
  })

  it("puts a group's reason directly above the first item it greys, beneath what is live", async () => {
    const layout = await layoutOf(<Placed group={GROUP} />)
    const at = layout.indexOf(`reason: ${GROUP}`)
    expect(layout.slice(at - 2, at + 3)).toEqual(["Open", "---", `reason: ${GROUP}`, "Commit", "Push"])
    expect(layout[0]).toBe("Open")
    expect(separatorProblems(layout)).toEqual([])
  })

  it("puts an item's reason directly beneath that item", async () => {
    const layout = await layoutOf(<Placed item={ITEM} />)
    const at = layout.indexOf("Refresh")
    expect(layout.slice(at, at + 3)).toEqual(["Refresh", `reason: ${ITEM}`, "Hide"])
  })

  it("puts two different sentences each over its own items", async () => {
    const layout = await layoutOf(<Placed group={GROUP} other={OTHER_GROUP} />)
    expect(layout).toEqual([
      "Open",
      "---",
      `reason: ${GROUP}`,
      "Commit",
      "Push",
      "---",
      `reason: ${OTHER_GROUP}`,
      "Pull",
      "Refresh",
      "Hide",
    ])
  })

  it("never starts on a rule when a group's reason is the first thing", async () => {
    const layout = await layoutOf(<Placed group={GROUP} leadingRow={false} />)
    expect(layout[0]).toBe(`reason: ${GROUP}`)
    expect(separatorProblems(layout)).toEqual([])
  })

  it("leaves no stray rule in any mix of reasons", async () => {
    const states: Parameters<typeof Placed>[0][] = []
    for (const whole of [null, WHOLE]) {
      for (const group of [null, GROUP]) {
        for (const other of [null, OTHER_GROUP]) {
          for (const item of [null, ITEM]) {
            for (const leadingRow of [true, false]) {
              states.push({ whole, group, other, item, leadingRow })
            }
          }
        }
      }
    }
    for (const state of states) {
      const { unmount } = render(<Placed {...state} />)
      const layout = menuLayout(await open())
      expect(separatorProblems(layout), JSON.stringify(state)).toEqual([])
      fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
      unmount()
    }
  })
})

// A reason is a handle and a line, and the two cannot come apart.
describe("a reason's handle and its line", () => {
  function Unplaced() {
    const reason = useMenuReason(SHARED)
    return (
      <DropdownMenu>
        <DropdownMenuTrigger>open</DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem disabled aria-describedby={reason.describedBy}>
            Commit…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  it("leaves an item describing nothing when its reason is never drawn", async () => {
    render(<Unplaced />)
    await open()
    expect(reasons()).toEqual([])
    expect(item("Commit…").hasAttribute("aria-describedby")).toBe(false)
  })

  function Twice() {
    const first = useMenuReason(SHARED)
    const second = useMenuReason(SHARED)
    return (
      <DropdownMenu>
        <DropdownMenuTrigger>open</DropdownMenuTrigger>
        <DropdownMenuContent>
          <MenuReasonsHead reasons={[first, second]} />
          <DropdownMenuItem disabled aria-describedby={first.describedBy}>
            Commit…
          </DropdownMenuItem>
          <DropdownMenuItem disabled aria-describedby={second.describedBy}>
            Push
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  it("says one sentence once however many handles carry it, and each item names that line", async () => {
    render(<Twice />)
    await open()
    expect(reasons().map((r) => r.textContent)).toEqual([SHARED])
    expect(description(item("Commit…"))).toBe(SHARED)
    expect(description(item("Push"))).toBe(SHARED)
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
