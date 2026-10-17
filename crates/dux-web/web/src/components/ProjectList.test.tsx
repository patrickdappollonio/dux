// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

const browser = installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { Dialog, DialogContent } from "@/components/ui/dialog"
import { DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { ProjectList } from "./ProjectList"

// A menu that is not inside a dialog is non-modal on a computer, so a press
// outside it reaches whatever was pressed. These pin what the list does with
// that press: jsdom does no hit-testing, so each test fires the events a real
// press produces, in order.

const settle = () => new Promise((r) => setTimeout(r, 60))

function renderList() {
  return render(
    <Dialog open historyEntry={false}>
      <DialogContent>
        <ProjectList
          title="Projects"
          description="All projects"
          rows={[{ id: "p1", name: "demo-api", label: null }]}
          menu={() => <DropdownMenuItem>Rename</DropdownMenuItem>}
        />
      </DialogContent>
    </Dialog>,
  )
}

const nameButton = () => screen.getByText("demo-api").closest("button")!

beforeEach(() => {
  vi.stubGlobal("innerWidth", 1280)
})

afterEach(() => {
  cleanup()
  void browser
})

describe("a project row's menu on a computer", () => {
  it("is closed by pressing the row's own name, not reopened by it", async () => {
    renderList()
    fireEvent.click(nameButton())
    await screen.findByRole("menu")
    // The press: pointer down closes the menu as an outside press, then the
    // click lands on the button.
    fireEvent.pointerDown(nameButton())
    fireEvent.mouseDown(nameButton())
    fireEvent.pointerUp(nameButton())
    fireEvent.mouseUp(nameButton())
    fireEvent.click(nameButton())
    await settle()
    expect(screen.queryByRole("menu")).toBeNull()
  })

  it("leaves focus on the control an outside press landed on", async () => {
    renderList()
    fireEvent.click(nameButton())
    await screen.findByRole("menu")
    const search = screen.getByLabelText("Search projects")
    fireEvent.pointerDown(search)
    fireEvent.mouseDown(search)
    search.focus()
    fireEvent.pointerUp(search)
    fireEvent.mouseUp(search)
    fireEvent.click(search)
    await settle()
    expect(screen.queryByRole("menu")).toBeNull()
    expect(document.activeElement).toBe(search)
  })
})
