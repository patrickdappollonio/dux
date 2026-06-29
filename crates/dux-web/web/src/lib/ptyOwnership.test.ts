import { afterEach, describe, expect, it, vi } from "vitest"

import {
  isForeground,
  isOwnerAfterHandover,
  notifyPtyOwner,
  onPtyOwner,
} from "./ptyOwnership"

// Every `onPtyOwner` registration in a test is tracked here and torn down in
// `afterEach` so the module-level listener set never leaks state across tests
// (a stray listener from one case would otherwise fire in another).
const registeredOffs: Array<() => void> = []
function track(off: () => void): () => void {
  registeredOffs.push(off)
  return off
}

afterEach(() => {
  vi.unstubAllGlobals()
  while (registeredOffs.length > 0) registeredOffs.pop()?.()
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

describe("isOwnerAfterHandover", () => {
  // Ownership is now decided by comparing the handover's claimer id against this
  // client's own PTY-socket connection id, not by a timing heuristic.
  it("is the owner when the handover's owner id is our own connection id", () => {
    expect(isOwnerAfterHandover("conn-7", "conn-7")).toBe(true)
  })

  it("is NOT the owner when the handover's owner id is another device's id", () => {
    // The race the id comparison fixes: a foreign claim demotes us to the
    // read-only placeholder definitively, regardless of broadcast ordering.
    expect(isOwnerAfterHandover("conn-9", "conn-7")).toBe(false)
  })

  it("is NOT the owner before our own `connected` frame has set our id", () => {
    expect(isOwnerAfterHandover("conn-7", null)).toBe(false)
  })

  it("is NOT the owner when the event carried no owner id", () => {
    expect(isOwnerAfterHandover(undefined, "conn-7")).toBe(false)
  })

  it("treats two distinct undefineds as non-ownership, not a match", () => {
    // null connId vs undefined owner must never coincidentally read as "us".
    expect(isOwnerAfterHandover(undefined, null)).toBe(false)
  })
})

describe("pty.owner fan-out", () => {
  it("delivers pty id + owner id to listeners and stops after unsubscribe", () => {
    const seen: Array<[string, string | undefined]> = []
    const off = track(onPtyOwner((id, owner) => seen.push([id, owner])))

    notifyPtyOwner("session-1", "conn-1")
    notifyPtyOwner("term-9", "conn-2")
    expect(seen).toEqual([
      ["session-1", "conn-1"],
      ["term-9", "conn-2"],
    ])

    off()
    notifyPtyOwner("session-1", "conn-3")
    expect(seen).toEqual([
      ["session-1", "conn-1"],
      ["term-9", "conn-2"],
    ])
  })

  it("isolates listeners: a listener only reacts to its own pty id", () => {
    const a: Array<string | undefined> = []
    const b: Array<string | undefined> = []
    track(
      onPtyOwner((id, owner) => {
        if (id === "a") a.push(owner)
      }),
    )
    track(
      onPtyOwner((id, owner) => {
        if (id === "b") b.push(owner)
      }),
    )

    notifyPtyOwner("a", "conn-a1")
    notifyPtyOwner("b", "conn-b1")
    notifyPtyOwner("a", "conn-a2")

    expect(a).toEqual(["conn-a1", "conn-a2"])
    expect(b).toEqual(["conn-b1"])
  })

  it("drives the ownership decision end to end (own claim vs foreign takeover)", () => {
    // The realistic wiring: a view holds its own connection id and flips ownership
    // by comparing each handover's owner id against it.
    const myConnId = "conn-self"
    let owner = true
    track(
      onPtyOwner((id, ownerId) => {
        if (id !== "session-1") return
        owner = isOwnerAfterHandover(ownerId, myConnId)
      }),
    )

    // Our own claim echoes back -> we stay the owner.
    notifyPtyOwner("session-1", "conn-self")
    expect(owner).toBe(true)

    // Another device takes over -> we are demoted to the placeholder.
    notifyPtyOwner("session-1", "conn-other")
    expect(owner).toBe(false)
  })
})
