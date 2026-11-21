// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { useState } from "react"

import { installBrowserHistory } from "@/test/browserHistory"

installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { Dialog, DialogContent, DialogTitle } from "./dialog"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu"
import { Popover, PopoverContent, PopoverTrigger } from "./popover"

afterEach(cleanup)

// A browser plays the popup's exit animation before base-ui unmounts it and
// hands focus back, so the dialog opened beside it has taken focus by then.
// jsdom has no animations; a closing popup is given one that ends a moment later.
beforeEach(() => {
  Element.prototype.getAnimations = function (this: Element) {
    if (!this.hasAttribute("data-ending-style") && !this.hasAttribute("data-closed")) return []
    const finished = new Promise((resolve) => setTimeout(resolve, 30))
    return [{ finished } as unknown as Animation]
  }
})
afterEach(() => {
  delete (Element.prototype as Partial<Element>).getAnimations
})

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 80))
  })
}

// A dialog opened from inside a popup that names where focus goes when it
// closes: the dialog keeps the keyboard, whatever the popup asked for.
function Harness({ kind }: { kind: "popover" | "menu" }) {
  const [open, setOpen] = useState(false)
  const [dialog, setDialog] = useState(false)
  const elsewhere = () => document.getElementById("elsewhere")
  const openDialog = () => {
    setOpen(false)
    setDialog(true)
  }
  return (
    <>
      <button id="elsewhere">Terminal</button>
      {kind === "popover" ? (
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger>Pick</PopoverTrigger>
          <PopoverContent finalFocus={elsewhere}>
            <button onClick={openDialog}>Edit</button>
          </PopoverContent>
        </Popover>
      ) : (
        <DropdownMenu open={open} onOpenChange={setOpen}>
          <DropdownMenuTrigger>Pick</DropdownMenuTrigger>
          <DropdownMenuContent finalFocus={elsewhere}>
            <DropdownMenuItem onClick={openDialog}>Edit</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <Dialog open={dialog} onOpenChange={setDialog}>
        <DialogContent>
          <DialogTitle>Macros</DialogTitle>
          <input aria-label="Name" autoFocus />
        </DialogContent>
      </Dialog>
    </>
  )
}

describe("a popup that closes on its way to a dialog", () => {
  it.each(["popover", "menu"] as const)("leaves focus in the dialog (%s)", async (kind) => {
    render(<Harness kind={kind} />)
    fireEvent.click(screen.getByText("Pick"))
    await settle()
    const edit = screen.getByText("Edit")
    edit.focus()
    fireEvent.click(edit)
    await settle()
    expect(document.activeElement).toBe(screen.getByLabelText("Name"))
  })

  it("still hands focus where it names when nothing else took it", async () => {
    function Plain() {
      const [open, setOpen] = useState(false)
      return (
        <>
          <button id="elsewhere">Terminal</button>
          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger>Pick</PopoverTrigger>
            <PopoverContent finalFocus={() => document.getElementById("elsewhere")}>
              <button onClick={() => setOpen(false)}>Run</button>
            </PopoverContent>
          </Popover>
        </>
      )
    }
    render(<Plain />)
    fireEvent.click(screen.getByText("Pick"))
    await settle()
    const run = screen.getByText("Run")
    run.focus()
    fireEvent.click(run)
    await settle()
    expect(document.activeElement).toBe(screen.getByText("Terminal"))
  })
})
