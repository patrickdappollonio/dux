// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import { toast } from "sonner"
import { afterEach, beforeAll, describe, expect, it } from "vitest"

import {
  TOAST_SWIPE_DIRECTIONS,
  TOAST_SWIPE_DIRECTIONS_TOP,
  Toaster,
  toastPlacement,
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
  // Nor a PointerEvent constructor. This one carries the pointer type, which is
  // what the Toaster reads, including on a release it replays itself.
  if (typeof globalThis.PointerEvent !== "function") {
    class PointerEventStandIn extends MouseEvent {
      readonly pointerType: string
      readonly pointerId: number
      constructor(type: string, init: PointerEventInit = {}) {
        super(type, init)
        this.pointerType = init.pointerType ?? ""
        this.pointerId = init.pointerId ?? 0
      }
    }
    globalThis.PointerEvent = PointerEventStandIn as typeof PointerEvent
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

function pointerEvent(type: string, pointerType: string, x = 0, y = 0, pointerId = 1): PointerEvent {
  return new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: x,
    clientY: y,
    pointerType,
    pointerId,
  })
}

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
//
// A finger by default, the pointer a swipe is for; a mouse drag selects instead.
async function swipe(el: HTMLElement, dx: number, dy: number, pointerType = "touch") {
  const at = (type: string, x: number, y: number) =>
    act(() => {
      el.dispatchEvent(pointerEvent(type, pointerType, x, y))
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

// A toast's text comes from producers that do not know how long it will run (a
// list of skipped paths once ran to 2,500 characters), so the toast caps its
// own height and scrolls its text inside rather than covering the screen.
describe("Toaster height cap", () => {
  const originalWidth = window.innerWidth

  afterEach(() => {
    window.innerWidth = originalWidth
  })

  // A phone lets the text run to 75% of the visible screen less the toast's own
  // padding and border (16px + 1px on each side), so the whole toast stays
  // within 75%. A computer keeps the short cap.
  it.each([
    { shell: "a phone", width: 390, cap: "max-h-[calc(75dvh-34px)]", not: "max-h-[min(12rem,40vh)]" },
    { shell: "a computer", width: 1280, cap: "max-h-[min(12rem,40vh)]", not: "max-h-[calc(75dvh-34px)]" },
  ])("caps the text's height on $shell and scrolls it inside the toast, leaving a finger's drag to the swipe", async ({ width, cap, not }) => {
    window.innerWidth = width
    render(<Toaster />)
    act(() => {
      toast.warning("a very long message")
    })
    const text = await screen.findByText("a very long message")
    const title = text.closest("[data-title]") as HTMLElement
    expect(title).toBeTruthy()
    expect(title.className).toContain("overflow-y-auto")
    const classes = title.className.split(/\s+/)
    expect(classes).toContain(cap)
    expect(classes).not.toContain(not)
    // A scroll container hands every touch pan to the browser, which cancels
    // the pointer stream sonner's swipe reads, whatever the toast itself says
    // (measured in Chromium). jsdom computes no touch-action, so the class that
    // turns the panning back off is what can be checked here.
    expect(title.className.split(/\s+/)).toContain("touch-none")
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

describe("Toaster text selection", () => {
  function titleEl(): HTMLElement {
    const el = document.querySelector("[data-sonner-toast] [data-title]")
    if (!el) throw new Error("no toast title rendered")
    return el as HTMLElement
  }

  it("marks every toast as selectable text, against the page's blanket user-select: none", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("copy ~/.config/dux/worktrees/demo")
    })
    await screen.findByText("copy ~/.config/dux/worktrees/demo")
    expect(toastEl().className.split(/\s+/)).toContain("allow-select")
  })

  it("does not swipe a toast away under a mouse drag, which is selecting its text", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("drag across me")
    })
    await screen.findByText("drag across me")
    await swipe(toastEl(), 120, 0, "mouse")
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })

  it("holds the countdown while a selection lies inside a toast, and resumes once it is gone", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("select me before I go", { duration: 150 })
    })
    await screen.findByText("select me before I go")
    const title = titleEl()
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse"))
    })
    act(() => {
      window.getSelection()?.selectAllChildren(title)
      title.dispatchEvent(pointerEvent("pointerup", "mouse"))
    })
    const section = document.querySelector("[data-sonner-toaster]")?.closest("section")
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
    expect(section?.hasAttribute("data-dux-pressed")).toBe(true)

    act(() => {
      window.getSelection()?.removeAllRanges()
      document.dispatchEvent(new Event("selectionchange"))
    })
    // The release reaches sonner in a task of its own.
    await waitFor(() => {
      expect(section?.hasAttribute("data-dux-pressed")).toBe(false)
    })
    await waitFor(() => {
      expect(document.querySelector("[data-sonner-toast]")).toBeNull()
    })
  })

  it("hands a held release over to the stack once the selected toast is gone", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("outlives the other", { duration: 150 })
      toast.warning("removed while selected", { id: "removed", duration: 60_000 })
    })
    await screen.findByText("outlives the other")
    const title = await screen.findByText("removed while selected")
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse"))
    })
    act(() => {
      window.getSelection()?.selectAllChildren(title)
      title.dispatchEvent(pointerEvent("pointerup", "mouse"))
    })
    act(() => {
      toast.dismiss("removed")
    })
    // No selectionchange: a browser need not send one when the selected nodes
    // leave the document.
    await waitFor(() => {
      expect(screen.queryByText("removed while selected")).toBeNull()
    })
    await waitFor(() => {
      expect(screen.queryByText("outlives the other")).toBeNull()
    })
  })

  it("leaves a finger's stack to sonner when its held release is handed over", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("long-pressed", { duration: 150 })
      toast.warning("beside it", { duration: 150 })
    })
    const title = await screen.findByText("long-pressed")
    act(() => {
      title.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }))
    })
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "touch"))
    })
    act(() => {
      title.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }))
    })
    act(() => {
      window.getSelection()?.selectAllChildren(title)
      title.dispatchEvent(pointerEvent("pointerup", "touch"))
    })
    act(() => {
      window.getSelection()?.removeAllRanges()
      document.dispatchEvent(new Event("selectionchange"))
    })
    const section = document.querySelector("[data-sonner-toaster]")?.closest("section")
    // The release reaches sonner in a task of its own.
    await waitFor(() => {
      expect(section?.hasAttribute("data-dux-pressed")).toBe(false)
    })
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(toastEl().getAttribute("data-expanded")).toBe("true")
    // sonner let go of the press: the next time the pointer leaves, it listens,
    // and the countdown runs out.
    act(() => {
      title.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }))
    })
    await waitFor(() => {
      expect(screen.queryByText("long-pressed")).toBeNull()
    })
  })

  it("lets a mouse move over a toast reach the rest of the page without moving the toast", async () => {
    render(<Toaster />)
    act(() => {
      toast.error("moved over")
    })
    const title = await screen.findByText("moved over")
    let seen = 0
    const count = () => seen++
    document.addEventListener("pointermove", count)
    try {
      act(() => {
        title.dispatchEvent(pointerEvent("pointerdown", "mouse"))
      })
      for (const x of [4, 60, 120]) {
        act(() => {
          title.dispatchEvent(pointerEvent("pointermove", "mouse", x, 0))
        })
      }
      expect(seen).toBe(3)
      expect(["", "0px"]).toContain(toastEl().style.getPropertyValue("--swipe-amount-x"))
      // A page listener that stops a move after sonner has had it.
      const swallow = (e: Event) => e.stopPropagation()
      document.body.addEventListener("pointermove", swallow)
      act(() => {
        title.dispatchEvent(pointerEvent("pointermove", "mouse", 160, 0))
      })
      document.body.removeEventListener("pointermove", swallow)
    } finally {
      document.removeEventListener("pointermove", count)
    }
    // Read at the release itself, before React hands it to sonner.
    let offsetAtRelease = "unread"
    title.addEventListener("pointerup", () => {
      offsetAtRelease = toastEl().style.getPropertyValue("--swipe-amount-x")
    })
    act(() => {
      title.dispatchEvent(pointerEvent("pointerup", "mouse", 120, 0))
    })
    expect(["", "0px"]).toContain(offsetAtRelease)
    expect(toastEl().getAttribute("data-removed")).not.toBe("true")
  })

  it("resumes a stack the selecting drag carried the pointer off", async () => {
    // sonner ignores the pointer leaving the stack while a press is down, and
    // nothing after the release tells it the pointer is gone. Two toasts,
    // because sonner collapses a stack whose toast count drops to one.
    render(<Toaster />)
    act(() => {
      toast.warning("left behind", { duration: 150 })
      toast.warning("dragged off the edge", { duration: 150 })
    })
    await screen.findByText("dragged off the edge")
    await screen.findByText("left behind")
    const title = screen.getByText("dragged off the edge")
    act(() => {
      title.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }))
    })
    expect(toastEl().getAttribute("data-expanded")).toBe("true")
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse"))
    })
    act(() => {
      title.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }))
    })
    act(() => {
      window.getSelection()?.selectAllChildren(title)
      title.dispatchEvent(pointerEvent("pointerup", "mouse"))
    })
    act(() => {
      window.getSelection()?.removeAllRanges()
      document.dispatchEvent(new Event("selectionchange"))
    })
    await waitFor(() => {
      expect(document.querySelector('[data-sonner-toast][data-removed="true"]')).not.toBeNull()
    })
  })
})

describe("Toaster presses that end without a release", () => {
  function pressAndEnd(end: (title: HTMLElement) => void) {
    const title = document.querySelector("[data-sonner-toast] [data-title]") as HTMLElement
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse"))
    })
    act(() => end(title))
  }

  it("resumes the countdown after a cancelled press", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("cancelled under me", { duration: 150 })
    })
    await screen.findByText("cancelled under me")
    pressAndEnd((title) => title.dispatchEvent(pointerEvent("pointercancel", "mouse")))
    const section = document.querySelector("[data-sonner-toaster]")?.closest("section")
    // The release reaches sonner in a task of its own.
    await waitFor(() => {
      expect(section?.hasAttribute("data-dux-pressed")).toBe(false)
    })
    await waitFor(() => {
      expect(document.querySelector("[data-sonner-toast]")).toBeNull()
    })
  })

  it("resumes the countdown after a press that became a native drag", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("dragged out of me", { duration: 150 })
    })
    await screen.findByText("dragged out of me")
    pressAndEnd((title) => {
      title.dispatchEvent(new Event("dragstart", { bubbles: true }))
      title.dispatchEvent(new Event("dragend", { bubbles: true }))
    })
    await waitFor(() => {
      expect(document.querySelector("[data-sonner-toast]")).toBeNull()
    })
  })

  it("keeps the countdown held after a native drag while the selection is still in the toast", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("dragged but still selected", { duration: 150 })
    })
    const title = await screen.findByText("dragged but still selected")
    act(() => {
      window.getSelection()?.selectAllChildren(title)
    })
    pressAndEnd((t) => {
      t.dispatchEvent(new Event("dragstart", { bubbles: true }))
      t.dispatchEvent(new Event("dragend", { bubbles: true }))
    })
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(screen.queryByText("dragged but still selected")).not.toBeNull()
    act(() => {
      window.getSelection()?.removeAllRanges()
      document.dispatchEvent(new Event("selectionchange"))
    })
    await waitFor(() => {
      expect(screen.queryByText("dragged but still selected")).toBeNull()
    })
  })

  it("ends a press whose toast was removed before the release landed outside", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("still counting", { duration: 150 })
      toast.warning("pressed then removed", { id: "pressed", duration: 60_000 })
    })
    await screen.findByText("still counting")
    const title = await screen.findByText("pressed then removed")
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse", 0, 0, 3))
    })
    act(() => {
      toast.dismiss("pressed")
    })
    await waitFor(() => {
      expect(screen.queryByText("pressed then removed")).toBeNull()
    })
    act(() => {
      document.body.dispatchEvent(pointerEvent("pointerup", "mouse", 0, 0, 3))
    })
    await waitFor(() => {
      expect(screen.queryByText("still counting")).toBeNull()
    })
  })
})

describe("Toaster releases it sends itself", () => {
  it("hands a held release over only once a press elsewhere has ended, under the original pointer", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("held across a drag", { duration: 150 })
    })
    const title = await screen.findByText("held across a drag")
    act(() => {
      title.dispatchEvent(pointerEvent("pointerdown", "mouse", 0, 0, 7))
    })
    act(() => {
      window.getSelection()?.selectAllChildren(title)
      title.dispatchEvent(pointerEvent("pointerup", "mouse", 0, 0, 7))
    })
    const heard: { during: boolean; pointerId: number }[] = []
    let pressing = false
    const listen = (e: Event) => heard.push({ during: pressing, pointerId: (e as PointerEvent).pointerId })
    window.addEventListener("pointerup", listen, true)
    try {
      // A press on the page outside the toasts, such as a divider drag.
      act(() => {
        pressing = true
        document.body.dispatchEvent(pointerEvent("pointerdown", "mouse", 0, 0, 1))
      })
      act(() => {
        document.body.dispatchEvent(pointerEvent("pointermove", "mouse", 40, 0, 1))
      })
      act(() => {
        pressing = false
        document.body.dispatchEvent(pointerEvent("pointerup", "mouse", 40, 0, 1))
      })
      await waitFor(() => {
        expect(screen.queryByText("held across a drag")).toBeNull()
      })
    } finally {
      window.removeEventListener("pointerup", listen, true)
    }
    expect(heard.filter((h) => h.during)).toEqual([])
    expect(heard.map((h) => h.pointerId)).toContain(7)
  })

  it("lets a release over a toast through for a press that began elsewhere", async () => {
    render(<Toaster />)
    act(() => {
      toast.warning("released over", { duration: 60_000 })
    })
    const title = await screen.findByText("released over")
    let reachedPage = false
    const listen = () => {
      reachedPage = true
    }
    document.addEventListener("pointerup", listen)
    try {
      act(() => {
        document.body.dispatchEvent(pointerEvent("pointerdown", "mouse"))
      })
      act(() => {
        window.getSelection()?.selectAllChildren(title)
        title.dispatchEvent(pointerEvent("pointerup", "mouse"))
      })
    } finally {
      document.removeEventListener("pointerup", listen)
      window.getSelection()?.removeAllRanges()
    }
    expect(reachedPage).toBe(true)
  })
})

describe("Toaster placement", () => {
  function toasterEl(): HTMLElement {
    const el = document.querySelector("[data-sonner-toaster]")
    if (!el) throw new Error("no toaster rendered")
    return el as HTMLElement
  }

  const originalWidth = window.innerWidth

  function renderAtWidth(width: number) {
    window.innerWidth = width
    render(<Toaster />)
  }

  afterEach(() => {
    window.innerWidth = originalWidth
  })

  it("keeps the computer at the bottom, offsets and all", () => {
    const placement = toastPlacement(false)
    expect(placement.position).toBe("bottom-center")
    expect(placement.offset).toEqual({
      bottom: "calc(env(safe-area-inset-bottom) + 2.5rem)",
    })
    expect(placement.swipeDirections).toBe(TOAST_SWIPE_DIRECTIONS)
    // Nothing is said about sonner's own sub-600px variables from this shell.
    expect(placement.mobileOffset).toBeUndefined()
  })

  it("anchors the phone at the top, where the typing surfaces are not", () => {
    const placement = toastPlacement(true)
    expect(placement.position).toBe("top-center")
    expect(placement.swipeDirections).toBe(TOAST_SWIPE_DIRECTIONS_TOP)
    expect(TOAST_SWIPE_DIRECTIONS_TOP).toContain("top")
    // Dragging a top toast DOWN pulls it over the terminal, so it is not an exit.
    expect(TOAST_SWIPE_DIRECTIONS_TOP).not.toContain("bottom")
  })

  it("clears the notch at the top, on both of sonner's own breakpoints", () => {
    const placement = toastPlacement(true)
    const top = "calc(env(safe-area-inset-top) + 1rem)"
    expect(placement.offset).toEqual({ top })
    // sonner swaps to its mobile variables at 600px, which is not the app's
    // 768px shell breakpoint: both have to say the same thing.
    expect(placement.mobileOffset).toEqual({ top })
  })

  // sonner renders nothing at all until something is in the store, so every
  // placement read needs a toast up first.
  async function raiseOne() {
    act(() => {
      toast.success("placed", { duration: Infinity })
    })
    await screen.findByText("placed")
  }

  it("renders bottom-anchored on a desktop window", async () => {
    renderAtWidth(1280)
    await raiseOne()
    expect(toasterEl().getAttribute("data-y-position")).toBe("bottom")
  })

  it("renders top-anchored on a phone window", async () => {
    renderAtWidth(390)
    await raiseOne()
    expect(toasterEl().getAttribute("data-y-position")).toBe("top")
    expect(toasterEl().style.getPropertyValue("--offset-top")).toBe(
      "calc(env(safe-area-inset-top) + 1rem)",
    )
    expect(toasterEl().style.getPropertyValue("--mobile-offset-top")).toBe(
      "calc(env(safe-area-inset-top) + 1rem)",
    )
  })

  it("dismisses a phone toast on an upward swipe", async () => {
    renderAtWidth(390)
    act(() => {
      toast.error("swipe me up")
    })
    await screen.findByText("swipe me up")
    await swipe(toastEl(), 0, -120)
    await waitFor(() => {
      expect(toastEl().getAttribute("data-removed")).toBe("true")
    })
  })

  it("keeps a phone toast on a downward swipe, which pulls it over the terminal", async () => {
    renderAtWidth(390)
    act(() => {
      toast.error("stay put")
    })
    await screen.findByText("stay put")
    await swipe(toastEl(), 0, 120)
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
