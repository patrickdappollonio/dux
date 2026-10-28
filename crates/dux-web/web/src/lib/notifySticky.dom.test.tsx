// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest"

import { Toaster } from "@/components/ui/sonner"

import {
  notifyBusy,
  notifyError,
  notifySuccess,
  notifyWarning,
  setStatusClearSeconds,
} from "./notify"

// A sticky notification waits for the user instead of for a clock. That is only
// acceptable if the user can actually get rid of it, so this file proves BOTH
// exits still work on an `Infinity` toast, against the real sonner rather than a
// spy.
//
// It is not obvious that they do. sonner has one mechanism that removes both a
// toast's close button and its swipe, and it would be easy to assume the
// mechanism is "this toast has no timer". It is not: the gate is
// `toastType === 'loading'` in both places, which is why a busy toast has no
// exit at all (pinned in `components/ui/sonner.test.tsx`) while an Infinity
// final keeps everything. Measured here so a sonner upgrade that changes its
// mind fails loudly, because the alternative is a toast the user cannot remove.

beforeAll(() => {
  // jsdom ships no Pointer Events; sonner calls setPointerCapture on every
  // pointerdown. Same stub as the Toaster's own test.
  if (!Element.prototype.setPointerCapture) {
    Element.prototype.setPointerCapture = () => {}
    Element.prototype.releasePointerCapture = () => {}
    Element.prototype.hasPointerCapture = () => false
  }
})

afterEach(() => {
  setStatusClearSeconds(undefined)
  cleanup()
})

const SWIPE_HOLD_MS = 250

function toastEl(): HTMLElement {
  const el = document.querySelector("[data-sonner-toast]")
  if (!el) throw new Error("no toast rendered")
  return el as HTMLElement
}

// Drive a finger drag across the toast (a mouse drag selects text instead).
// Each event goes in its OWN act() because sonner locks the swipe axis into
// React state on the first move and reads it on the NEXT one.
async function swipe(el: HTMLElement, dx: number, dy: number) {
  const opts = { bubbles: true, cancelable: true, button: 0 }
  const at = (type: string, x: number, y: number) =>
    act(() => {
      const ev = new MouseEvent(type, { ...opts, clientX: x, clientY: y })
      Object.defineProperty(ev, "pointerType", { value: "touch" })
      el.dispatchEvent(ev)
    })
  at("pointerdown", 0, 0)
  at("pointermove", Math.sign(dx) * 4, Math.sign(dy) * 4)
  at("pointermove", dx, dy)
  // Real elapsed time matters: sonner also dismisses on velocity.
  await new Promise((resolve) => setTimeout(resolve, SWIPE_HOLD_MS))
  at("pointerup", dx, dy)
}

describe("a sticky notification", () => {
  it("stays put where the same tone without it would have gone", async () => {
    // One second of window, so an ordinary error would be long gone.
    setStatusClearSeconds(0.25)
    render(<Toaster />)
    act(() => {
      notifyError("saved but never delivered", { sticky: true })
    })
    await screen.findByText("saved but never delivered")
    await new Promise((resolve) => setTimeout(resolve, 1500))
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })

  it("keeps its close button, which sonner withholds only from a spinner", async () => {
    render(<Toaster />)
    act(() => {
      notifySuccess("waiting for you", { sticky: true })
    })
    await screen.findByText("waiting for you")
    expect(document.querySelector("[data-close-button]")).not.toBeNull()
  })

  it("can still be swiped away", async () => {
    render(<Toaster />)
    act(() => {
      notifyError("swipe me even though I am sticky", { sticky: true })
    })
    await screen.findByText("swipe me even though I am sticky")
    await swipe(toastEl(), -120, 0)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })
})

describe("a sticky notification that REPLACED a spinner on the same id", () => {
  // This is the only shape production actually raises. The file-drop path
  // raises `notifyBusy` on a per-drop id before every upload and then raises
  // the report on that same id, so the sticky warning is always a `loading`
  // toast being taken over, never a fresh one.
  //
  // It is worth its own test because sonner gates the close button AND the
  // swipe on the same thing, `toastType === 'loading'`, and a handoff is
  // exactly the case where one could plausibly be left behind. A sticky toast
  // with neither exit would be unremovable for the life of the page.
  async function busyThenSticky(message: string) {
    render(<Toaster />)
    act(() => {
      notifyBusy("Uploading shot.png...", { id: "drop-1" })
    })
    await screen.findByText("Uploading shot.png...")
    // The spinner has no exits at all, which is what makes the handoff matter.
    expect(document.querySelector("[data-close-button]")).toBeNull()
    act(() => {
      notifyWarning(message, { id: "drop-1", sticky: true })
    })
    await screen.findByText(message)
  }

  it("gets the close button the spinner it replaced was refused", async () => {
    await busyThenSticky("Saved shot.png, but the path was not sent.")
    expect(document.querySelector("[data-close-button]")).not.toBeNull()
  })

  it("can be swiped away, which the spinner it replaced could not", async () => {
    await busyThenSticky("Saved shot.png, but the path was not sent.")
    await swipe(toastEl(), -120, 0)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })

  it("still waits, where the same handoff without sticky would have cleared", async () => {
    // Be exact about what this measures: the sticky duration surviving the
    // handoff, on a window short enough that an ordinary warning would be gone.
    // It does NOT measure the leak guard, which fires a minute out and is
    // pinned against the clock in `notify.test.ts` instead.
    setStatusClearSeconds(0.25)
    await busyThenSticky("Saved shot.png, but the path was not sent.")
    await new Promise((resolve) => setTimeout(resolve, 1500))
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })
})

describe("an ordinary notification", () => {
  it("retires on the configured window without anyone touching it", async () => {
    setStatusClearSeconds(0.25)
    render(<Toaster />)
    act(() => {
      notifySuccess("all done")
    })
    await screen.findByText("all done")
    await waitFor(
      () => {
        expect(toastEl().getAttribute("data-removed")).toBe("true")
      },
      { timeout: 2000 },
    )
  })

  // THE `term-copy` REPRODUCTION, at the mechanism. An id is a REPLACEMENT
  // instruction: sonner resets a toast's remaining time only when its DURATION
  // changes, while re-running its close timer on every re-raise, so repeating a
  // raise on one fixed id keeps restarting the countdown and the toast never
  // finishes. Copy-on-select fired on every drag with a fixed `term-copy` id,
  // which is how "Copied to clipboard" came to sit on screen for 90 seconds
  // across 30 copies.
  it("retires after a burst of repeats, because repeats share no id and so no clock", async () => {
    setStatusClearSeconds(0.25)
    render(<Toaster />)
    for (let i = 0; i < 5; i++) {
      act(() => {
        notifySuccess("Copied to clipboard")
      })
      await new Promise((resolve) => setTimeout(resolve, 120))
    }
    await waitFor(
      () => {
        for (const el of document.querySelectorAll("[data-sonner-toast]")) {
          expect(el.getAttribute("data-removed")).toBe("true")
        }
      },
      { timeout: 2500 },
    )
  })

  it("is pinned open by a fixed id under the same burst, which is the bug the id removal fixes", async () => {
    // The counterexample, so the test above is measuring something. Same
    // cadence, same window, one shared id: the countdown never gets to finish.
    setStatusClearSeconds(0.25)
    render(<Toaster />)
    for (let i = 0; i < 5; i++) {
      act(() => {
        notifySuccess("Copied to clipboard", { id: "term-copy" })
      })
      await new Promise((resolve) => setTimeout(resolve, 120))
    }
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })
})

describe("a notification with an action button", () => {
  it("leaves the toast to the press's handler, which replaces it on the same id", async () => {
    render(<Toaster />)
    let pressed = 0
    act(() => {
      notifyWarning("the copy was blocked", {
        id: "copy-1",
        sticky: true,
        action: { label: "Copy", onClick: () => (pressed += 1) },
      })
    })
    await screen.findByText("the copy was blocked")
    act(() => {
      screen.getByRole("button", { name: "Copy" }).click()
    })
    expect(pressed).toBe(1)
    // The press alone must not close the toast: only its handler knows
    // whether the press worked.
    await new Promise((resolve) => setTimeout(resolve, 300))
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")

    act(() => {
      notifySuccess("copied it", { id: "copy-1" })
    })
    await screen.findByText("copied it")
    expect(screen.queryByRole("button", { name: "Copy" })).toBeNull()
  })
})

describe("the countdown bar", () => {
  // sonner MERGES a raise into the toast already on its id, so a bar left over
  // from the toast being replaced would keep draining on one that never goes.
  it("drains only while the toast on an id is going to dismiss itself", async () => {
    render(<Toaster />)
    const bar = () => toastEl().querySelector(".dux-toast-drain")
    act(() => {
      notifySuccess("counting down", { id: "k" })
    })
    await screen.findByText("counting down")
    expect(bar()).not.toBeNull()

    act(() => {
      notifyWarning("waiting for you", { id: "k", sticky: true })
    })
    await screen.findByText("waiting for you")
    expect(bar()).toBeNull()

    act(() => {
      notifyBusy("working on it", { id: "k" })
    })
    await screen.findByText("working on it")
    expect(bar()).toBeNull()

    // A base window of 0 turns auto-clear off for every final tone.
    setStatusClearSeconds(0)
    act(() => {
      notifyError("no window at all", { id: "k" })
    })
    await screen.findByText("no window at all")
    expect(bar()).toBeNull()
  })

  // The bar starts over on every replacement, so sonner's timer has to as well.
  // sonner only resets a toast's remaining time when its duration changes, so a
  // toast that was paused once and then replaced with the same duration would
  // resume from the remainder left at that pause and vanish early.
  it("restarts the full window when a once-paused toast is replaced on its id", () => {
    // Fake timers and a fake clock, so the boundaries below are exact: sonner
    // measures a pause with `Date` and delivers every raise on a timeout.
    vi.useFakeTimers()
    try {
      const advance = (ms: number) =>
        act(() => {
          vi.advanceTimersByTime(ms)
        })
      setStatusClearSeconds(6)
      render(<Toaster />)
      act(() => {
        notifySuccess("first copy", { id: "same" })
      })
      advance(0)
      screen.getByText("first copy")
      advance(3000)
      // Hovering expands the stack, which is what pauses sonner's timers, with
      // 3000ms of the 6000ms window left.
      const stack = document.querySelector("[data-sonner-toaster]") as HTMLElement
      act(() => {
        fireEvent.mouseEnter(stack)
      })
      advance(500)
      act(() => {
        fireEvent.mouseLeave(stack)
      })
      act(() => {
        notifySuccess("second copy", { id: "same" })
      })
      advance(0)
      screen.getByText("second copy")
      // Well past the old remainder, just short of a fresh window.
      advance(5900)
      expect(toastEl().getAttribute("data-removed")).not.toBe("true")
      advance(200)
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    } finally {
      vi.useRealTimers()
    }
  })
})
