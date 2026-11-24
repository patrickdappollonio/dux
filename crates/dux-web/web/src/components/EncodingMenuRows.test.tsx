// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"
import { separatorProblems } from "@/test/menuLayout"

installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { EncodingMenuRows } from "@/components/EncodingMenuRows"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useMenuReason } from "@/hooks/use-menu-reason"
import { encodingBusyReason } from "@/lib/editorEncoding"
import type { LaneOp } from "@/lib/editorLane"

afterEach(cleanup)

const READ_ONLY = "This file is read-only. Reopen or convert it from the encoding menu."

function Menu({
  busy,
  readOnly,
  canSave = true,
}: {
  busy: LaneOp | null
  readOnly: boolean
  canSave?: boolean
}) {
  const busyReason = useMenuReason(busy === null ? null : encodingBusyReason(busy))
  const readOnlyReason = useMenuReason(readOnly ? READ_ONLY : null)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <EncodingMenuRows
          busy={busyReason}
          readOnly={readOnlyReason}
          fileBusy={busy !== null}
          writeBlocked={readOnly}
          canSave={canSave}
          current={{ encoding: "UTF-8", bom: false }}
          onReopen={() => {}}
          onSave={() => {}}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

async function open() {
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

const row = (text: string) => screen.getByText(text).closest('[role="menuitem"]') as HTMLElement
const lines = () => Array.from(document.querySelectorAll('[data-slot="menu-reason"]'))
const said = (el: HTMLElement) =>
  (el.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter(Boolean)
    .map((id) => document.getElementById(id)?.textContent)

// What the menu holds, top to bottom.
function order(menu: HTMLElement): string[] {
  return Array.from(
    menu.querySelectorAll(
      '[data-slot="menu-reason"], [data-slot="menu-reason-separator"], [role="menuitem"]',
    ),
  ).map((el) =>
    el.getAttribute("data-slot") === "menu-reason-separator"
      ? "---"
      : (el.textContent ?? ""),
  )
}

describe("the encoding menu's reasons", () => {
  it("says each lane operation's own sentence", () => {
    expect(encodingBusyReason("save")).toBe(
      "This file is being saved. Choose an encoding once the save has finished.",
    )
    expect(encodingBusyReason("convert")).toBe(encodingBusyReason("save"))
    expect(encodingBusyReason("reopen")).toBe(
      "This file is being reopened in another encoding. Choose an encoding once it has finished.",
    )
    expect(encodingBusyReason("reload")).toBe(
      "This file is being reloaded from disk. Choose an encoding once it has finished.",
    )
    expect(encodingBusyReason("freshness")).toBe(
      "This file is being checked against the disk. Choose an encoding once it has finished.",
    )
  })

  it("busy only: the reason heads the menu above a separator and both rows name it", async () => {
    render(<Menu busy="save" readOnly={false} />)
    const menu = await open()
    const busy = encodingBusyReason("save")
    expect(order(menu)).toEqual([busy, "---", "Reopen with encoding", "Save with encoding"])
    expect(said(row("Reopen with encoding"))).toEqual([busy])
    expect(said(row("Save with encoding"))).toEqual([busy])
    expect(separatorProblems(order(menu))).toEqual([])
  })

  it("read-only only: the reason sits beneath Save with encoding alone", async () => {
    render(<Menu busy={null} readOnly />)
    const menu = await open()
    expect(order(menu)).toEqual(["Reopen with encoding", "Save with encoding", READ_ONLY])
    expect(said(row("Reopen with encoding"))).toEqual([])
    expect(said(row("Save with encoding"))).toEqual([READ_ONLY])
  })

  it("both: the busy line heads the menu, the read-only line stays beneath its row, each said once", async () => {
    render(<Menu busy="reopen" readOnly />)
    const menu = await open()
    const busy = encodingBusyReason("reopen")
    expect(order(menu)).toEqual([
      busy,
      "---",
      "Reopen with encoding",
      "Save with encoding",
      READ_ONLY,
    ])
    expect(lines()).toHaveLength(2)
    expect(said(row("Reopen with encoding"))).toEqual([busy])
    expect(said(row("Save with encoding"))).toEqual([busy, READ_ONLY])
    expect(separatorProblems(order(menu))).toEqual([])
  })

  it("the lone Reopen row: busy heads the menu, no read-only line", async () => {
    render(<Menu busy="reload" readOnly canSave={false} />)
    const menu = await open()
    const busy = encodingBusyReason("reload")
    expect(order(menu)).toEqual([busy, "---", "Reopen with encoding"])
    expect(said(row("Reopen with encoding"))).toEqual([busy])
    expect(separatorProblems(order(menu))).toEqual([])
  })

  it("idle: just the rows, no line and no separator", async () => {
    render(<Menu busy={null} readOnly={false} />)
    const menu = await open()
    expect(order(menu)).toEqual(["Reopen with encoding", "Save with encoding"])
    expect(said(row("Save with encoding"))).toEqual([])
  })
})
