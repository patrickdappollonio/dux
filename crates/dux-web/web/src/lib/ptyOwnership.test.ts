import { afterEach, describe, expect, it, vi } from "vitest"

import {
  appliedPtyOwnerEpoch,
  handshakeSuperseded,
  isForeground,
  isOwnerAfterHandover,
  notifyPtyOwner,
  onPtyOwner,
  resetPtyOwnerEpochs,
  seedVerdictFromConnected,
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

// THE HANDSHAKE SEED. A plain claim is refused SILENTLY server-side now, so a
// foregrounded arrival's optimistic guess would never be corrected: it would
// render typing surfaces over a pty whose keystrokes are all dropped, with no
// card. This is the correction, and the ORDER of its rules is its whole content.
describe("seedVerdictFromConnected", () => {
  const base = { myConnId: "mine", foreground: true, takeoverArmed: false }

  it("makes a foregrounded arrival a WATCHER of a pty somebody else drives", () => {
    expect(seedVerdictFromConnected({ ...base, owner: "theirs" })).toBe(false)
  })

  it("still lets the foreground guess claim an UNOWNED pty", () => {
    expect(seedVerdictFromConnected({ ...base, owner: null })).toBe(true)
    expect(
      seedVerdictFromConnected({ ...base, owner: null, foreground: false }),
    ).toBe(false)
  })

  // An ABSENT owner key is an older server that still grants any claim. Reading
  // it as "unowned" would be a guess dressed as an answer; reading it as "owned"
  // would strand every pane on that server as a permanent watcher.
  it("falls back to the foreground guess when the server did not answer", () => {
    expect(seedVerdictFromConnected({ ...base, owner: undefined })).toBe(true)
    expect(
      seedVerdictFromConnected({ ...base, owner: undefined, foreground: false }),
    ).toBe(false)
  })

  // The take-over bounce's own handshake still reports the OLD owner: the claim
  // rides the first resize of the new connection, so ownership lags by one
  // replay parse. Demoting here would flash the card back over a pane the user
  // has just taken, and the flagged frame would then be refused by the client's
  // own owner gate before it ever reached the wire.
  it("an ARMED take-over outranks every other rule", () => {
    expect(
      seedVerdictFromConnected({
        ...base,
        owner: "theirs",
        takeoverArmed: true,
        foreground: false,
      }),
    ).toBe(true)
  })

  it("recognises its own id as ownership", () => {
    expect(
      seedVerdictFromConnected({ ...base, owner: "mine", foreground: false }),
    ).toBe(true)
  })

  // THE CROSS-SOCKET RACE. The handshake rides the PTY socket; `pty.owner`
  // rides the events socket; nothing orders the two TCP connections. A fresh
  // `pty.owner{owner:B, epoch:1}` can be applied BEFORE a stale
  // `connected{owner:null, owner_epoch:0}` lands, and the stale-null direction
  // emits no correcting event, ever: seeding from it would wedge this client
  // as a phantom owner with every plain resize refused silently. The epoch
  // comparison makes the seed defer to the strictly newer applied verdict.
  describe("deferring to a strictly newer applied pty.owner", () => {
    it("keeps the demotion when a stale null handshake follows a newer owner", () => {
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: null,
          handshakeEpoch: 0,
          appliedEpoch: 1,
          priorVerdict: false,
        }),
      ).toBe(false)
    })

    it("keeps a true prior verdict too: it defers, it does not demote", () => {
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: "theirs",
          handshakeEpoch: 3,
          appliedEpoch: 5,
          priorVerdict: true,
        }),
      ).toBe(true)
    })

    it("seeds normally when the applied epoch is equal or older", () => {
      // Equal: the handshake snapshot was taken at (or after) that claim.
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: null,
          handshakeEpoch: 1,
          appliedEpoch: 1,
          priorVerdict: false,
        }),
      ).toBe(true)
      // Older applied: the handshake is the fresher answer.
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: null,
          handshakeEpoch: 2,
          appliedEpoch: 1,
          priorVerdict: false,
        }),
      ).toBe(true)
    })

    it("seeds normally when no pty.owner has been applied yet", () => {
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: null,
          handshakeEpoch: 0,
          appliedEpoch: undefined,
          priorVerdict: false,
        }),
      ).toBe(true)
    })

    it("leaves the old-server path unchanged: no epoch, foreground fallback", () => {
      // An old server omits `owner` and `owner_epoch` together, so the defer
      // rule can never fire and the legacy foreground guess decides, even when
      // a handover with an epoch was applied earlier in the session.
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: undefined,
          handshakeEpoch: undefined,
          appliedEpoch: 4,
          priorVerdict: false,
        }),
      ).toBe(true)
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: undefined,
          foreground: false,
          handshakeEpoch: undefined,
          appliedEpoch: 4,
          priorVerdict: true,
        }),
      ).toBe(false)
    })

    it("an armed take-over still outranks the defer rule", () => {
      expect(
        seedVerdictFromConnected({
          ...base,
          owner: "theirs",
          takeoverArmed: true,
          foreground: false,
          handshakeEpoch: 0,
          appliedEpoch: 9,
          priorVerdict: false,
        }),
      ).toBe(true)
    })
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

  it("passes the device (4th) argument through to listeners", () => {
    // The take-over modal names the other device from this `device` arg (the raw
    // User-Agent the server stamps on the handover), so it must survive the fan-out.
    const seen: Array<string | undefined> = []
    track(onPtyOwner((_id, _owner, device) => seen.push(device)))

    notifyPtyOwner("session-1", "conn-other", 1, "Mozilla/5.0 Chrome/120")
    notifyPtyOwner("session-1", "conn-self", 2) // no device -> undefined

    expect(seen).toEqual(["Mozilla/5.0 Chrome/120", undefined])
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

  it("exposes the applied high-water mark to the handshake seed, per pty", () => {
    expect(appliedPtyOwnerEpoch("session-1")).toBeUndefined()
    notifyPtyOwner("session-1", "conn-A", 3)
    expect(appliedPtyOwnerEpoch("session-1")).toBe(3)
    expect(appliedPtyOwnerEpoch("other")).toBeUndefined()
    // An epochless (mixed-version) delivery records nothing.
    notifyPtyOwner("session-1", "conn-B")
    expect(appliedPtyOwnerEpoch("session-1")).toBe(3)
    // And the ONE staleness comparison reads it: strictly newer applied only.
    expect(handshakeSuperseded(2, 3)).toBe(true)
    expect(handshakeSuperseded(3, 3)).toBe(false)
    expect(handshakeSuperseded(4, 3)).toBe(false)
    expect(handshakeSuperseded(undefined, 3)).toBe(false)
    expect(handshakeSuperseded(2, undefined)).toBe(false)
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
