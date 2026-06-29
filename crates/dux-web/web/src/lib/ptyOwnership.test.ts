import { afterEach, describe, expect, it, vi } from "vitest"

import {
  isForeground,
  isOwnerAfterHandover,
  notifyPtyOwner,
  onPtyOwner,
  resetPtyOwnerEpochs,
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
  // Clear the per-pty epoch high-water marks so a case that records epochs cannot
  // make a later case wrongly drop a handover as "stale".
  resetPtyOwnerEpochs()
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

describe("pty.owner epoch dedup", () => {
  it("ignores an out-of-order (older) handover so a stale owner cannot win", () => {
    // The crux of the fix: the server assigns a monotonic epoch under its owners
    // lock, but the broadcast is emitted after the lock releases and can be
    // reordered. The map ends on owner=A (epoch 2), yet a client could receive the
    // owner=B (epoch 1) broadcast LAST. Keeping only the highest epoch per pty
    // makes the stale owner=B arrival a no-op, so the client stays on owner=A.
    const seen: Array<string | undefined> = []
    track(
      onPtyOwner((id, owner) => {
        if (id === "session-1") seen.push(owner)
      }),
    )

    notifyPtyOwner("session-1", "conn-A", 2) // newer claim, applied
    notifyPtyOwner("session-1", "conn-B", 1) // older broadcast arrives late
    notifyPtyOwner("session-1", "conn-B", 2) // same epoch as applied: also ignored

    expect(seen).toEqual(["conn-A"])
  })

  it("delivers a strictly-newer epoch and isolates dedup per pty id", () => {
    const seen: Array<[string, string | undefined]> = []
    track(onPtyOwner((id, owner) => seen.push([id, owner])))

    notifyPtyOwner("a", "conn-a1", 5)
    notifyPtyOwner("a", "conn-a2", 6) // strictly newer -> delivered
    notifyPtyOwner("a", "conn-a-stale", 4) // older -> ignored
    notifyPtyOwner("b", "conn-b1", 1) // different pty: its own counter -> delivered

    expect(seen).toEqual([
      ["a", "conn-a1"],
      ["a", "conn-a2"],
      ["b", "conn-b1"],
    ])
  })

  it("always delivers a handover with no epoch (mixed-version degrade)", () => {
    const seen: Array<string | undefined> = []
    track(
      onPtyOwner((id, owner) => {
        if (id === "session-1") seen.push(owner)
      }),
    )
    notifyPtyOwner("session-1", "conn-1") // no epoch
    notifyPtyOwner("session-1", "conn-2") // no epoch, still delivered
    expect(seen).toEqual(["conn-1", "conn-2"])
  })

  it("treats a handover arriving while own conn id is null as non-owner without crashing", () => {
    // On reconnect a `pty.owner` over /ws/events can land before this client's new
    // `connected` frame sets its id. With the id still null the ownership decision
    // must safely resolve to non-owner (observe), never throw.
    let owner = true
    let myConnId: string | null = null
    track(
      onPtyOwner((id, ownerId) => {
        if (id !== "session-1") return
        owner = isOwnerAfterHandover(ownerId, myConnId)
      }),
    )

    expect(() => notifyPtyOwner("session-1", "conn-self", 1)).not.toThrow()
    expect(owner).toBe(false)

    // Once the `connected` frame sets our id, a newer handover resolves correctly.
    myConnId = "conn-self"
    notifyPtyOwner("session-1", "conn-self", 2)
    expect(owner).toBe(true)
  })

  it("resetPtyOwnerEpochs clears high-water marks so a post-restart epoch is not dropped", () => {
    const seen: Array<string | undefined> = []
    track(
      onPtyOwner((id, owner) => {
        if (id === "session-1") seen.push(owner)
      }),
    )
    notifyPtyOwner("session-1", "conn-old", 9)
    // Server restarts: its epoch counter restarts at 1. Without a reset this would
    // be ignored as <= 9; the reconnect reset makes it deliver again.
    resetPtyOwnerEpochs()
    notifyPtyOwner("session-1", "conn-new", 1)
    expect(seen).toEqual(["conn-old", "conn-new"])
  })
})
