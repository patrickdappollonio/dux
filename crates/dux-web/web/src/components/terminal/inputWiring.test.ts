// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import type { Terminal } from "@xterm/xterm"

import { focusTypingSurfaceIn } from "./inputSurface"
import type { LiveSettings } from "./liveValues"
import {
  registerComposeFocusGuard,
  suspendTerminalTabStop,
} from "./inputWiring"

// The focus-routing rule is imported from the input surface for real (the
// fallback case below is the whole point of not mocking it), and that module
// pulls in the store, which touches `localStorage` at import time. This
// environment does not provide one, so the store gets the smallest stand-in
// that satisfies the bindings; nothing here reads a draft.
vi.mock("@/lib/notify", () => ({ notifyError: () => {} }))
vi.mock("@/lib/store", () => ({
  useDux: () => ({}),
  composeDraft: () => "",
  peekComposeDraft: () => "",
  setComposeDraft: () => {},
}))

// A stand-in for xterm's hidden textarea inside the pane's container: the guard
// listens for the bubbling `focusin`, so anything focusable in there will do.
function pane() {
  const container = document.createElement("div")
  const inner = document.createElement("textarea")
  container.appendChild(inner)
  document.body.appendChild(container)
  return { container, inner }
}

/// The message box, which lives OUTSIDE the pane's container.
function messageBox() {
  const box = document.createElement("textarea")
  document.body.appendChild(box)
  return box
}

/// Everything inside `root` a Tab press could land on.
function tabbableInside(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>("*")).filter(
    (element) => element.tabIndex >= 0,
  )
}

/// Let the guard's deferred redirect run.
async function settle(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
}

afterEach(() => {
  document.body.innerHTML = ""
})

describe("registerComposeFocusGuard", () => {
  // THE MESSAGE BOX IS THE TYPING SURFACE WHEN IT IS ON. A finger never had
  // another way in, but a mouse can click straight into the terminal, which is
  // reachable the moment the box can be turned on for a fine pointer.
  it("hands focus to the message box when the terminal takes it", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    registerComposeFocusGuard({
      container,
      composeActive: () => true,
      focusTypingSurface,
    })

    inner.focus()
    await settle()

    expect(focusTypingSurface).toHaveBeenCalledTimes(1)
  })

  // A CLICK INTO THE TERMINAL COMES FROM THE BOX, so the event that has to be
  // redirected is exactly the one whose `relatedTarget` is the message box. It
  // is indistinguishable from a Shift-Tab out of the box, which is why the
  // keyboard trap is answered by the TAB ORDER (see `suspendTerminalTabStop`)
  // rather than by reading `relatedTarget` here: a guard that skipped this
  // event would hand the keyboard back to xterm on every click.
  it("still redirects a click that came from the message box itself", async () => {
    const { container, inner } = pane()
    const box = messageBox()
    box.focus()
    const focusTypingSurface = vi.fn()
    registerComposeFocusGuard({
      container,
      composeActive: () => true,
      focusTypingSurface,
    })

    inner.focus()
    await settle()

    expect(focusTypingSurface).toHaveBeenCalledTimes(1)
  })

  // THE X11 PRIMARY SELECTION, which is a real Linux regression and not a
  // theory: xterm publishes a terminal selection to the middle-click clipboard
  // by stuffing it into its hidden textarea and running `focus()` then
  // `select()` back to back, synchronously. A redirect that fired between the
  // two moved focus away before the selection was taken, and middle-click paste
  // into other applications quietly stopped working while the box was up.
  it("lets xterm finish selecting before it moves focus", async () => {
    const { container, inner } = pane()
    const box = messageBox()
    const focusedWhenSelected: (Element | null)[] = []
    const select = inner.select.bind(inner)
    inner.select = () => {
      focusedWhenSelected.push(document.activeElement)
      select()
    }
    registerComposeFocusGuard({
      container,
      composeActive: () => true,
      focusTypingSurface: () => box.focus(),
    })

    inner.value = "a terminal selection"
    inner.focus()
    inner.select()

    expect(focusedWhenSelected).toEqual([inner])
    await settle()
    expect(document.activeElement).toBe(box)
  })

  it("leaves the terminal alone while typing goes straight into it", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    registerComposeFocusGuard({
      container,
      composeActive: () => false,
      focusTypingSurface,
    })

    inner.focus()
    await settle()

    expect(focusTypingSurface).not.toHaveBeenCalled()
  })

  // Read at event time, not at registration: the Direct toggle flips the answer
  // under a wiring that outlives it.
  it("follows a live flip of the surface", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    let active = false
    registerComposeFocusGuard({
      container,
      composeActive: () => active,
      focusTypingSurface,
    })

    inner.focus()
    await settle()
    expect(focusTypingSurface).not.toHaveBeenCalled()

    active = true
    inner.blur()
    inner.focus()
    await settle()
    expect(focusTypingSurface).toHaveBeenCalledTimes(1)
  })

  // The redirect is deferred, so the surface can go away between the focus and
  // the move. Asked again on the way out rather than acted on blind.
  it("drops a deferred redirect the Direct toggle has already cancelled", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    let active = true
    registerComposeFocusGuard({
      container,
      composeActive: () => active,
      focusTypingSurface,
    })

    inner.focus()
    active = false
    await settle()

    expect(focusTypingSurface).not.toHaveBeenCalled()
  })

  it("stops listening once disposed, so an unmounted pane cannot steal focus", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    const guard = registerComposeFocusGuard({
      container,
      composeActive: () => true,
      focusTypingSurface,
    })

    guard.dispose()
    inner.focus()
    await settle()

    expect(focusTypingSurface).not.toHaveBeenCalled()
  })

  it("drops a redirect already queued when the pane is disposed", async () => {
    const { container, inner } = pane()
    const focusTypingSurface = vi.fn()
    const guard = registerComposeFocusGuard({
      container,
      composeActive: () => true,
      focusTypingSurface,
    })

    inner.focus()
    guard.dispose()
    await settle()

    expect(focusTypingSurface).not.toHaveBeenCalled()
  })

  // THE REAL ROUTING RULE, not a mock of it. A pane with no message box mounted
  // (a watcher, or the commit before the box renders) resolves to xterm's own
  // textarea, which is the element the guard just saw take focus. Focusing an
  // already-focused element fires no `focusin`, so the guard cannot re-enter:
  // the fallback settles instead of bouncing between the two surfaces.
  it("falls back to the terminal without looping when no box is mounted", async () => {
    const { container, inner } = pane()
    const term = { focus: () => inner.focus() } as unknown as Terminal
    const live = {
      current: { composeActive: true },
    } as unknown as LiveSettings
    const refs = {
      live,
      composeInputRef: { current: null },
      termRef: { current: term },
    }
    const focusTypingSurface = vi.fn(() => focusTypingSurfaceIn(refs))
    registerComposeFocusGuard({
      container,
      composeActive: () => live.current.composeActive,
      focusTypingSurface,
    })

    inner.focus()
    await settle()

    expect(focusTypingSurface).toHaveBeenCalledTimes(1)
    expect(document.activeElement).toBe(inner)
  })
})

// THE KEYBOARD WAY OUT OF THE PANE. The guard sends every focus that lands in
// the container to the message box, so while it is armed xterm's hidden
// textarea is a trap: Shift-Tab out of the box landed on it and bounced
// straight back, and backward navigation out of the pane was impossible.
describe("suspendTerminalTabStop", () => {
  it("leaves nothing in the pane for Shift-Tab to land on", () => {
    const { container, inner } = pane()
    expect(tabbableInside(container)).toEqual([inner])

    const restore = suspendTerminalTabStop(inner)

    expect(tabbableInside(container)).toEqual([])
    restore()
    expect(tabbableInside(container)).toEqual([inner])
  })

  it("puts back whatever tab index the terminal had", () => {
    const { inner } = pane()
    inner.tabIndex = 3

    const restore = suspendTerminalTabStop(inner)
    expect(inner.tabIndex).toBe(-1)

    restore()
    expect(inner.tabIndex).toBe(3)
  })

  // The terminal is created by an effect of its own, so the pane can ask for
  // this before xterm has a textarea to take out of the order.
  it("tolerates a terminal that has not been created yet", () => {
    expect(() => suspendTerminalTabStop(null)()).not.toThrow()
  })
})
