// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

const browser = installBrowserHistory()
const offline = vi.fn(() => Promise.reject(new Error("offline test")))
vi.stubGlobal("fetch", offline)
await import("@/lib/store")

import { Modal } from "@/components/modal"
import { DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { ProjectRows } from "./ProjectList"
import { useProjectSearch } from "./projectSearch"

// A menu that is not inside a dialog is non-modal on a computer, so a press
// outside it reaches whatever was pressed. These pin what the list does with
// that press: jsdom does no hit-testing, so each test fires the events a real
// press produces, in order.

const settle = () => new Promise((r) => setTimeout(r, 60))

const ROWS = [{ id: "p1", name: "demo-api", label: null }]

// The rows as the Projects list puts them in its modal, with nothing to pick.
function List() {
  const search = useProjectSearch(ROWS, undefined)
  return (
    <Modal
      kind="picker"
      open
      historyEntry={false}
      onClose={() => {}}
      title="Projects"
      search={{
        value: search.query,
        onChange: search.setQuery,
        label: "Search projects",
        fieldProps: search.pick.fieldProps,
      }}
    >
      <ProjectRows
        search={search}
        heading="Choose a project"
        menu={() => <DropdownMenuItem>Rename</DropdownMenuItem>}
      />
    </Modal>
  )
}

function renderList() {
  return render(<List />)
}

const nameButton = () => screen.getByText("demo-api").closest("button")!

beforeEach(() => {
  vi.stubGlobal("innerWidth", 1280)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.stubGlobal("fetch", offline)
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

  it("lights the part of a project's name the search matched", () => {
    renderList()
    fireEvent.change(screen.getByLabelText("Search projects"), {
      target: { value: "API" },
    })
    expect(screen.getByText("api").tagName).toBe("MARK")
  })

  it("leaves focus on the control an outside press landed on", async () => {
    renderList()
    fireEvent.click(nameButton())
    await screen.findByRole("menu")
    const search = screen.getByLabelText("Search projects")
    fireEvent.pointerDown(search)
    // A browser runs the microtasks the pointer down queued before the mouse
    // down that focuses the field.
    await act(async () => {})
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
