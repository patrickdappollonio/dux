// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { closeModals, openModals } from "@/lib/modalHistory"

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
    fireEvent.click(screen.getByText("open"))
    await screen.findByText("Plain")
    act(() => closeModals(openModals().slice(-1).map((modal) => modal.id)))
    await waitFor(() => {
      expect(screen.queryByText("Plain")).toBeNull()
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
