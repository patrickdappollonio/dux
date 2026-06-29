import { afterEach, describe, expect, it, vi } from "vitest"

import {
  PtyOwnershipTracker,
  SELF_CLAIM_GRACE_MS,
  isForeground,
  notifyPtyOwner,
  onPtyOwner,
} from "./ptyOwnership"

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("isForeground", () => {
  // The terminal view seeds its initial ownership from this: a visible mount
  // claims (sends its size) and becomes the owner; a hidden mount attaches as a
  // silent observer and sends nothing.
  it("is true when the tab is visible (a visible mount claims / sends size)", () => {
    vi.stubGlobal("document", { visibilityState: "visible" })
    expect(isForeground()).toBe(true)
  })

  it("is false when the tab is hidden (a hidden mount observes / sends no size)", () => {
    vi.stubGlobal("document", { visibilityState: "hidden" })
    expect(isForeground()).toBe(false)
  })

  it("defaults to foreground with no document so a claim is never suppressed", () => {
    vi.stubGlobal("document", undefined)
    expect(isForeground()).toBe(true)
  })
})

describe("PtyOwnershipTracker", () => {
  // A controllable clock so the grace window is exercised deterministically.
  function trackerAt(clock: { t: number }) {
    return new PtyOwnershipTracker(() => clock.t)
  }

  it("treats our own claim's echo as not a demotion, then demotes a later takeover", () => {
    const clock = { t: 0 }
    const tracker = trackerAt(clock)

    // We claim (foreground attach / take-over) and the server echoes our own
    // `pty.owner` back almost immediately: that is NOT a demotion.
    tracker.noteLocalClaim()
    clock.t = 50
    expect(tracker.isDemotion()).toBe(false)

    // A later `pty.owner` (another device took over) IS a demotion.
    clock.t = 200
    expect(tracker.isDemotion()).toBe(true)
  })

  it("resolves a takeover that lands close after our own claim (count, not just time)", () => {
    const clock = { t: 0 }
    const tracker = trackerAt(clock)

    // We claim at t=0; another device claims at t=500. Both echoes land inside the
    // grace window, but in broadcast order: ours first (consumed), theirs second.
    tracker.noteLocalClaim()
    clock.t = 60
    expect(tracker.isDemotion()).toBe(false) // our own echo, consumed
    clock.t = 560
    expect(tracker.isDemotion()).toBe(true) // their takeover, within grace but unmatched
  })

  it("does not let a stale (never-arriving) echo absorb a much-later takeover", () => {
    const clock = { t: 0 }
    const tracker = trackerAt(clock)

    tracker.noteLocalClaim()
    // Our echo never arrives; a genuine takeover lands well past the grace window.
    clock.t = SELF_CLAIM_GRACE_MS + 1
    expect(tracker.isDemotion()).toBe(true)
  })

  it("with no local claim, any owner event is a demotion", () => {
    const tracker = new PtyOwnershipTracker(() => 1000)
    expect(tracker.isDemotion()).toBe(true)
  })
})

describe("pty.owner fan-out", () => {
  it("delivers to registered listeners and stops after unsubscribe", () => {
    const seen: string[] = []
    const off = onPtyOwner((id) => seen.push(id))

    notifyPtyOwner("session-1")
    notifyPtyOwner("term-9")
    expect(seen).toEqual(["session-1", "term-9"])

    off()
    notifyPtyOwner("session-1")
    expect(seen).toEqual(["session-1", "term-9"])
  })

  it("isolates listeners: a listener only reacts to its own pty id", () => {
    const a: string[] = []
    const b: string[] = []
    const offA = onPtyOwner((id) => {
      if (id === "a") a.push(id)
    })
    const offB = onPtyOwner((id) => {
      if (id === "b") b.push(id)
    })

    notifyPtyOwner("a")
    notifyPtyOwner("b")
    notifyPtyOwner("a")

    expect(a).toEqual(["a", "a"])
    expect(b).toEqual(["b"])
    offA()
    offB()
  })
})
