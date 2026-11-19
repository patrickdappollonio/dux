// @vitest-environment jsdom
import { useRef, useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { installBrowserHistory } from "@/test/browserHistory"

// The browser's Back reaches a dialog through the store, which gives every open
// dialog a history entry of its own, so the real store is loaded over a
// history that has one.
const browser = installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

afterEach(cleanup)

describe("the browser's Back", () => {
  // A dialog nobody controls keeps its open state inside base-ui, so this is
  // the case where the primitive has to know it is open by itself.
  it("closes a dialog opened from its own trigger", async () => {
    render(
      <Dialog>
        <DialogTrigger>open</DialogTrigger>
        <DialogContent>
          <DialogTitle>Plain</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    const before = browser.index
    fireEvent.click(screen.getByText("open"))
    await screen.findByText("Plain")
    await waitFor(() => {
      expect(browser.index).toBe(before + 1)
    })
    act(() => browser.back())
    await waitFor(() => {
      expect(screen.queryByText("Plain")).toBeNull()
    })
  })

  // A dialog that is busy refuses to close, the way Settings does while a
  // write is in flight. The Back still counts: the dialog closes as soon as it
  // is free to.
  it("closes a busy dialog once it is no longer busy", async () => {
    function Busy() {
      const [open, setOpen] = useState(true)
      const busyRef = useRef(true)
      const [, setBusy] = useState(true)
      return (
        <Dialog
          open={open}
          onOpenChange={(next) => {
            if (!next && !busyRef.current) setOpen(false)
          }}
        >
          <DialogContent>
            <DialogTitle>Saving</DialogTitle>
            <button
              type="button"
              onClick={() => {
                busyRef.current = false
                setBusy(false)
              }}
            >
              finish
            </button>
          </DialogContent>
        </Dialog>
      )
    }
    const before = browser.index
    render(<Busy />)
    await screen.findByText("Saving")
    await waitFor(() => {
      expect(browser.index).toBe(before + 1)
    })
    act(() => browser.back())
    expect(screen.getByText("Saving")).toBeTruthy()
    fireEvent.click(screen.getByText("finish"))
    await waitFor(() => {
      expect(screen.queryByText("Saving")).toBeNull()
    })
  })
})

function overlay() {
  return document.querySelector('[data-slot="dialog-overlay"]')
}

describe("DialogContent destructive backdrop", () => {
  it("keeps the modest blur on every overlay", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogTitle>Plain</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    const el = overlay()
    expect(el).not.toBeNull()
    expect(el?.className).toContain(
      "supports-backdrop-filter:backdrop-blur-sm",
    )
  })

  it("does NOT add backdrop-grayscale without the destructive prop", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogTitle>Plain</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    expect(overlay()?.className).not.toContain("backdrop-grayscale")
  })

  it("adds backdrop-grayscale when destructive is set", () => {
    render(
      <Dialog open>
        <DialogContent destructive>
          <DialogTitle>Danger</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    const el = overlay()
    expect(el?.className).toContain("backdrop-grayscale")
    // blur stays on the base overlay for the destructive variant too
    expect(el?.className).toContain(
      "supports-backdrop-filter:backdrop-blur-sm",
    )
  })

  it("does not spread the destructive prop onto the popup element", () => {
    render(
      <Dialog open>
        <DialogContent destructive>
          <DialogTitle>Danger</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    const popup = document.querySelector('[data-slot="dialog-content"]')
    expect(popup).not.toBeNull()
    expect(popup?.getAttribute("destructive")).toBeNull()
  })
})

describe("DialogFooter touch height", () => {
  // jsdom cannot evaluate media queries, so the footer's own classes are read
  // back and each one's descendant selector is tried against the rendered
  // buttons, a plain one and a link-styled one.
  it("lifts every footer button, a link rendered as one included, to 44px at phone width and on a coarse pointer at any width", () => {
    render(
      <DialogFooter>
        <button type="button">Cancel</button>
        <Button render={<a href="https://example.com">Docs</a>} />
      </DialogFooter>,
    )
    const footer = document.querySelector('[data-slot="dialog-footer"]') as HTMLElement
    const anchor = screen.getByText("Docs").closest("a") as HTMLElement
    const plain = screen.getByText("Cancel")
    for (const media of ["max-md", "pointer-coarse"]) {
      const rule = Array.from(footer.classList).find(
        (c) => c.startsWith(`${media}:[&_`) && c.endsWith("]:min-h-11"),
      )
      expect(
        rule,
        `${media}: a touch tablet at desktop width gets the desktop layout, so its footer buttons need the coarse-pointer minimum as well as the phone-width one`,
      ).toBeDefined()
      const selector = (rule as string)
        .slice(`${media}:[&_`.length, -"]:min-h-11".length)
        .replaceAll("_", " ")
      expect(plain.matches(selector), `${media}: a plain footer button`).toBe(true)
      expect(anchor.matches(selector), `${media}: a Button rendered as a link`).toBe(true)
    }
  })
})

describe("DialogContent close button touch size", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("gives the 28px corner close button a 7px margin per side at phone width and on a coarse pointer, a 40px tap area across its 1px border (a browser must measure 40px), without growing its visible box", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogTitle>Title</DialogTitle>
        </DialogContent>
      </Dialog>,
    )
    const classes = Array.from(
      document.querySelector('[data-slot="dialog-close"]')?.classList ?? [],
    )
    expect(
      classes,
      "the 28px close button is under the 40px touch floor: a 7px-per-side pseudo-element must cover phone width and a coarse pointer, and be off otherwise",
    ).toEqual(
      expect.arrayContaining([
        "after:hidden",
        "max-md:after:block",
        "pointer-coarse:after:block",
        "after:-inset-[7px]",
      ]),
    )
    expect(
      classes.filter((c) => /size-10|top-0\.5|right-0\.5/.test(c)),
      "growing the visible box pushes its focus ring into the popup's clipped edge, so the box stays 28px at 8px from the corner",
    ).toEqual([])
    expect(classes).toEqual(expect.arrayContaining(["top-2", "right-2", "size-7"]))
  })
})
