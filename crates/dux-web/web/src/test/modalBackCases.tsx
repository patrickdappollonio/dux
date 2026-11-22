// The browser's Back over a `Modal`, as shared cases, so each runs on a history
// whose traversals land at once and on one whose traversals land on a later
// task (see `installBrowserHistory`). The file that calls this installs the
// history and imports the store first.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { Modal } from "@/components/modal"
import type { ModalAction } from "@/components/modal"
import { registerModal } from "@/lib/modalHistory"
import { chip } from "@/lib/prose"

import type { BrowserHistory } from "./browserHistory"

const save: ModalAction = { label: "Save", tone: "primary", onSelect: () => {} }

function editing({
  changes,
  onClose,
  setAside,
}: {
  changes: number
  onClose: () => void
  setAside?: () => void
}) {
  return (
    <Modal
      kind="form"
      open
      onClose={onClose}
      title="Sample"
      submit={save}
      unsavedEdits={{
        holder: "The sample has",
        changes,
        leaves: ["the file ", chip("notes.md")],
        setAside,
      }}
    />
  )
}

function saving({ busy, onClose }: { busy: boolean; onClose: () => void }) {
  return <Modal kind="form" open busy={busy} onClose={onClose} title="Sample" submit={save} />
}

const asking = () => screen.queryByText("Discard unsaved changes?")

export function describeModalBack(browser: BrowserHistory): void {
  describe("the browser's Back over a modal", () => {
    beforeEach(() => {
      vi.stubGlobal("innerWidth", 1280)
      vi.stubGlobal("matchMedia", (query: string) => ({
        matches: false,
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
      }))
    })
    // A close takes the modal's entry away by a traversal, which on a history
    // that runs them later is still under way when the test ends.
    afterEach(async () => {
      cleanup()
      vi.unstubAllGlobals()
      await new Promise((resolve) => setTimeout(resolve, 20))
    })

    it("closes a modal as its Cancel does, and leaves one that opened by itself alone", async () => {
      const onClose = vi.fn()
      const before = browser.index
      const view = render(<Modal kind="report" open onClose={onClose} title="Sample" />)
      await waitFor(() => expect(browser.index).toBe(before + 1))
      act(() => browser.back())
      await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
      expect(onClose).toHaveBeenCalledWith("back")
      view.unmount()

      // One that opened by itself takes no entry, so the one after it takes
      // the first.
      await waitFor(() => expect(browser.index).toBe(before))
      const after = browser.index
      render(<Modal kind="report" open onClose={onClose} title="Itself" historyEntry={false} />)
      render(<Modal kind="report" open onClose={onClose} title="By hand" />)
      await waitFor(() => expect(browser.index).toBe(after + 1))
      expect(browser.length).toBe(after + 2)
    })

    // Back asks as Cancel does, and the form keeps its history entry while it
    // asks, so no later Back can carry the page away from under it.
    it("asks once on Back and stays on its history entry", async () => {
      const onClose = vi.fn()
      const before = browser.index
      render(editing({ changes: 1, onClose }))
      await waitFor(() => expect(browser.index).toBe(before + 1))
      act(() => browser.back())
      await waitFor(() => expect(asking()).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
      await waitFor(() => expect(asking()).toBeNull())
      await waitFor(() => expect(browser.index).toBe(before + 1))
      expect(onClose).not.toHaveBeenCalled()

      act(() => browser.back())
      await waitFor(() => expect(asking()).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: "Discard changes" }))
      expect(onClose).toHaveBeenCalledWith("back")
    })

    // A Back gives the page no user activation, and an entry pushed without
    // one is skipped by the next Back: the question then has no entry of its
    // own.
    it("never lets Back leave the page while the question it raised is up", async () => {
      const onClose = vi.fn()
      const before = browser.index
      render(editing({ changes: 1, onClose }))
      await waitFor(() => expect(browser.index).toBe(before + 1))
      vi.stubGlobal("navigator", { userActivation: { isActive: false } })
      act(() => browser.back())
      await waitFor(() => expect(asking()).not.toBeNull())
      await waitFor(() => expect(browser.index).toBe(before + 1))
      // The next Back abandons the question, as its Keep editing does.
      act(() => browser.back())
      await waitFor(() => expect(asking()).toBeNull())
      await waitFor(() => expect(browser.index).toBe(before + 1))
      expect(onClose).not.toHaveBeenCalled()
    })

    // Macros and Preferences: the edits are kept for the next opening, so Back
    // has nothing to ask about.
    it("sets the edits aside on Back when the caller can, and asks on Cancel still", async () => {
      const onClose = vi.fn()
      const setAside = vi.fn()
      const before = browser.index
      render(editing({ changes: 1, onClose, setAside }))
      await waitFor(() => expect(browser.index).toBe(before + 1))
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
      await waitFor(() => expect(asking()).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
      await waitFor(() => expect(asking()).toBeNull())
      expect(setAside).not.toHaveBeenCalled()
      await waitFor(() => expect(browser.index).toBe(before + 1))
      act(() => browser.back())
      await waitFor(() => expect(onClose).toHaveBeenCalledWith("back"))
      expect(setAside).toHaveBeenCalledTimes(1)
      expect(asking()).toBeNull()
    })

    it("holds a busy modal's history entry through a Back, and closes once it is free", async () => {
      const onClose = vi.fn()
      const before = browser.index
      const view = render(saving({ busy: true, onClose }))
      await waitFor(() => expect(browser.index).toBe(before + 1))
      act(() => browser.back())
      await waitFor(() => expect(browser.index).toBe(before + 1))
      expect(onClose).not.toHaveBeenCalled()
      view.rerender(saving({ busy: false, onClose }))
      await waitFor(() => expect(onClose).toHaveBeenCalledWith("back"))
    })

    // A dialog that closes on Back sits under a form that asks first; a jump
    // two entries back closes the one and asks in the other, which keeps an
    // entry of its own so the next Back is its too.
    it("keeps an entry for a modal that asks, under which a plain dialog closed", async () => {
      const before = browser.index
      let plainClosed = 0
      const release = registerModal(() => {
        plainClosed += 1
        release()
      })
      await waitFor(() => expect(browser.index).toBe(before + 1))
      const onClose = vi.fn()
      render(editing({ changes: 1, onClose }))
      await waitFor(() => expect(browser.index).toBe(before + 2))
      act(() => browser.go(-2))
      await waitFor(() => expect(asking()).not.toBeNull())
      expect(plainClosed).toBe(1)
      fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
      await waitFor(() => expect(asking()).toBeNull())
      await waitFor(() => expect(browser.index).toBe(before + 1))

      act(() => browser.back())
      await waitFor(() => expect(asking()).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
      await waitFor(() => expect(asking()).toBeNull())
      await waitFor(() => expect(browser.index).toBe(before + 1))
      expect(onClose).not.toHaveBeenCalled()

      act(() => browser.back())
      await waitFor(() => expect(asking()).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: "Discard changes" }))
      expect(onClose).toHaveBeenCalledWith("back")
    })
  })
}
