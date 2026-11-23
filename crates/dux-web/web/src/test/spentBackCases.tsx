// Back over a modal that has no history entry of its own, as shared cases run
// on a history whose traversals land at once and on one whose traversals land
// on a later task. The file that calls this installs the history, pushes one
// page entry over the first, and imports the store.
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, screen, waitFor } from "@testing-library/react"

import { Modal } from "@/components/modal"

import type { BrowserHistory } from "./browserHistory"

export function describeSpentBack(browser: BrowserHistory): void {
  function Stepped({ onClose }: { onClose: () => void }) {
    const [editing, setEditing] = useState(true)
    return (
      <Modal
        kind="form"
        open
        onClose={onClose}
        title={editing ? "Edit macro" : "Macros"}
        step={editing ? "edit" : "list"}
        back={editing ? { label: "all macros", onSelect: () => setEditing(false) } : undefined}
        submit={{ label: "Save", tone: "primary", onSelect: () => {} }}
      />
    )
  }

  // Macros reopened by itself after a failed save: no user activation, so no
  // entry could be pushed for it.
  describe("Back from a step of a modal with no history entry", () => {
    afterEach(async () => {
      cleanup()
      Reflect.deleteProperty(navigator, "userActivation")
      await new Promise((resolve) => setTimeout(resolve, 20))
    })

    it("goes back a step and leaves the page where it was", async () => {
      Object.defineProperty(navigator, "userActivation", {
        configurable: true,
        value: { isActive: false },
      })
      const onClose = vi.fn()
      const page = browser.index
      render(<Stepped onClose={onClose} />)
      await new Promise((resolve) => setTimeout(resolve, 20))
      expect(browser.index).toBe(page)
      act(() => browser.back())
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Macros" })).not.toBeNull())
      await waitFor(() => expect(browser.index).toBe(page))
      expect(onClose).not.toHaveBeenCalled()
    })
  })
}
