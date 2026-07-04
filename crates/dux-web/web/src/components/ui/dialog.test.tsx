// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render } from "@testing-library/react"

import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog"

afterEach(cleanup)

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
