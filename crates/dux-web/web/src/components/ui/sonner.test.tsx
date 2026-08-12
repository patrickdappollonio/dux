// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import { toast } from "sonner"
import { afterEach, beforeAll, describe, expect, it } from "vitest"

import {
  TOAST_SWIPE_DIRECTIONS,
  Toaster,
  VISIBLE_TOASTS_DESKTOP,
  VISIBLE_TOASTS_MOBILE,
} from "./sonner"

// jsdom has no Pointer Events implementation: `setPointerCapture` is missing on
// Element, and sonner calls it on every pointerdown. Stub it so the swipe
// handlers run exactly as they do in a browser.
beforeAll(() => {
  if (!Element.prototype.setPointerCapture) {
    Element.prototype.setPointerCapture = () => {}
    Element.prototype.releasePointerCapture = () => {}
    Element.prototype.hasPointerCapture = () => false
  }
})

afterEach(() => {
  act(() => {
    toast.dismiss()
  })
  // vitest runs with `globals: false` here, so @testing-library registers no
  // auto-cleanup: without this the previous test's Toaster (and its toasts)
  // stay in the document and the next test's queries read the stale one.
  cleanup()
})

// A human-plausible drag duration. 6px over this long is well under sonner's
// 0.11 px/ms flick velocity; 120px over it is well over the distance threshold.
const SWIPE_HOLD_MS = 250

function toastEl(): HTMLElement {
  const el = document.querySelector("[data-sonner-toast]")
  if (!el) throw new Error("no toast rendered")
  return el as HTMLElement
}

function iconSvg(): SVGElement {
  const el = document.querySelector("[data-icon] svg")
  if (!el) throw new Error("no toast icon rendered")
  return el as unknown as SVGElement
}

// Drive a pointer drag across the toast. `dx`/`dy` are the total offsets from
// the press point.
//
// Each event goes in its OWN act() because sonner locks the swipe axis into
// React state on the first move and only reads that state on the NEXT move.
// Batching the whole gesture into one act() leaves the axis null for the entire
// drag, so no swipe amount is ever recorded and nothing is dismissed. A real
// browser delivers pointermove across frames, which is what this reproduces.
async function swipe(el: HTMLElement, dx: number, dy: number) {
  // jsdom has no PointerEvent constructor; MouseEvent carries every field
  // sonner reads (button, clientX/clientY) and React dispatches by type name.
  const opts = { bubbles: true, cancelable: true, button: 0 }
  const at = (type: string, x: number, y: number) =>
    act(() => {
      el.dispatchEvent(new MouseEvent(type, { ...opts, clientX: x, clientY: y }))
    })
  at("pointerdown", 0, 0)
  // A small first move locks the axis, then the full drag is measured.
  at("pointermove", Math.sign(dx) * 4, Math.sign(dy) * 4)
  at("pointermove", dx, dy)
  // Real elapsed time matters: sonner ALSO dismisses on velocity (distance over
  // the wall-clock duration of the drag), so a gesture delivered in zero time
  // would dismiss on any distance at all and the threshold would go untested.
  await new Promise((resolve) => setTimeout(resolve, SWIPE_HOLD_MS))
  at("pointerup", dx, dy)
}

describe("Toaster severity styling", () => {
  it("colors the error icon with the destructive token", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("Everything is on fire")
    })
    await screen.findByText("Everything is on fire")
    expect(iconSvg().getAttribute("class")).toContain("text-destructive")
  })

  it("gives every tone a DIFFERENT icon color, so severity reads at a glance", async () => {
    const seen = new Map<string, string>()
    for (const tone of ["success", "info", "warning", "error"] as const) {
      render(<Toaster />)
      act(() => {
        toast[tone](`a ${tone} message`)
      })
      await screen.findByText(`a ${tone} message`)
      const cls = iconSvg().getAttribute("class") ?? ""
      // The color class is whatever text-* utility the icon carries.
      const color = cls
        .split(/\s+/)
        .filter((c) => c.startsWith("text-"))
        .join(" ")
      expect(color, `${tone} icon has no color class`).not.toBe("")
      seen.set(tone, color)
      act(() => {
        toast.dismiss()
      })
      cleanup()
    }
    expect(new Set(seen.values()).size).toBe(seen.size)
  })

  it("keeps the icons shape-distinct, so color is never the only signal", async () => {
    const shapes = new Set<string>()
    for (const tone of ["success", "info", "warning", "error"] as const) {
      render(<Toaster />)
      act(() => {
        toast[tone](`shape ${tone}`)
      })
      await screen.findByText(`shape ${tone}`)
      const cls = iconSvg().getAttribute("class") ?? ""
      const lucide = cls.split(/\s+/).find((c) => c.startsWith("lucide-") && c !== "lucide")
      expect(lucide, `${tone} icon is not a lucide shape`).toBeTruthy()
      shapes.add(lucide as string)
      act(() => {
        toast.dismiss()
      })
      cleanup()
    }
    expect(shapes.size).toBe(4)
  })
})

describe("Toaster swipe to dismiss", () => {
  it("declares the horizontal directions sonner would not infer from bottom-center", () => {
    // sonner derives its default directions by splitting the position string,
    // so "bottom-center" yields ["bottom", "center"] and a sideways swipe is
    // inert. Naming the directions is what makes left/right work.
    expect(TOAST_SWIPE_DIRECTIONS).toContain("left")
    expect(TOAST_SWIPE_DIRECTIONS).toContain("right")
    expect(TOAST_SWIPE_DIRECTIONS).toContain("bottom")
  })

  it("dismisses on a swipe to the left", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("swipe me left")
    })
    await screen.findByText("swipe me left")
    await swipe(toastEl(), -120, 0)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })

  it("dismisses on a swipe to the right", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("swipe me right")
    })
    await screen.findByText("swipe me right")
    await swipe(toastEl(), 120, 0)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })

  it("still dismisses on a downward swipe, the gesture a bottom toast invites", async () => {
    render(<Toaster />)
    act(() => {
      toast.success("swipe me down")
    })
    await screen.findByText("swipe me down")
    await swipe(toastEl(), 0, 120)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })

  it("PINS sonner's limitation: a loading toast has no close button and cannot be swiped", async () => {
    // Measured, not assumed. sonner renders the close button only for
    // `closeButton && !toast.jsx && toastType !== 'loading'`, and its
    // pointerdown handler returns early while `disabled` (which is exactly
    // `toastType === 'loading'`). So a busy toast has NO manual exit at all,
    // by either route, and it is the store's own timer that has to retire it.
    // If a sonner upgrade ever loosens this, this test fails and the store's
    // busy timer can be reconsidered.
    render(<Toaster />)
    act(() => {
      toast.loading("working forever", { duration: 50 })
    })
    await screen.findByText("working forever")
    expect(document.querySelector("[data-close-button]")).toBeNull()
    await swipe(toastEl(), -120, 0)
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })

  it("PINS sonner's limitation: a loading toast ignores its `duration` entirely", async () => {
    // sonner's auto-close effect bails on `toast.type === 'loading'` before it
    // ever starts a timer, so passing a finite duration to `toast.loading` is
    // inert. This is why `showStatusToast` schedules the busy dismissal itself.
    render(<Toaster />)
    act(() => {
      toast.loading("still working", { duration: 10 })
    })
    await screen.findByText("still working")
    await new Promise((resolve) => setTimeout(resolve, 80))
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
    expect(screen.getByText("still working")).toBeTruthy()
  })

  it("ignores a nudge below the swipe threshold, so a stray tap does not dismiss", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("hold still")
    })
    await screen.findByText("hold still")
    await swipe(toastEl(), -6, 0)
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })
})

describe("Toaster stacking depth", () => {
  // `useIsMobile` reads window.innerWidth through useSyncExternalStore, and
  // jsdom lets us set it. 768 is the breakpoint the whole app shares.
  function renderAtWidth(width: number) {
    window.innerWidth = width
    render(<Toaster />)
  }

  async function raise(n: number) {
    for (let i = 0; i < n; i++) {
      act(() => {
        toast.success(`message ${i}`, { duration: Infinity })
      })
    }
    await screen.findByText("message 0")
  }

  it("shows five at once on a desktop window, where sonner would have shown three", async () => {
    expect(VISIBLE_TOASTS_DESKTOP).toBe(5)
    renderAtWidth(1280)
    await raise(6)
    // The sixth is queued: sonner keeps it mounted but marks it not visible.
    const visible = [...document.querySelectorAll("[data-sonner-toast]")].filter(
      (el) => el.getAttribute("data-visible") === "true",
    )
    expect(visible.length).toBe(VISIBLE_TOASTS_DESKTOP)
  })

  it("keeps three on a phone, where the toasts sit over the terminal", async () => {
    expect(VISIBLE_TOASTS_MOBILE).toBe(3)
    renderAtWidth(390)
    await raise(6)
    const visible = [...document.querySelectorAll("[data-sonner-toast]")].filter(
      (el) => el.getAttribute("data-visible") === "true",
    )
    expect(visible.length).toBe(VISIBLE_TOASTS_MOBILE)
  })
})
