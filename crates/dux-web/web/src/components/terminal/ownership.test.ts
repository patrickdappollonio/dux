// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"
import type { PtySocket } from "@/lib/ptySocket"

import { useTerminalOwnership } from "./ownership"

// The ledger is the machine's one external side effect, so it is recorded rather than
// stubbed away: "does this pane publish a verdict, and which" is half of what
// the four states mean.
const { ledger } = vi.hoisted(() => ({ ledger: [] as [string, string][] }))
// The store is replaced outright rather than spread over: importing it for real
// touches localStorage at module scope, and the machine wants exactly one
// function out of it.
vi.mock("@/lib/store", () => ({
  noteAgentPtyOwnership: (id: string, verdict: string) => {
    ledger.push([id, verdict])
  },
}))

class PtyFake {
  isOpen = true
  connects = 0
  connect() {
    this.connects++
  }
}

/// `document.visibilityState`, which is what `isForeground` reads. The initial
/// guess and the freed-pty auto-claim both turn on it.
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
}

function setup(opts: { kind?: "agent" | "terminal" } = {}) {
  const pty = new PtyFake()
  const focuses: number[] = []
  const reconnecting: boolean[] = []
  // The lifecycle's send port for the freed-pty claim, recorded rather than
  // performed: this hook's contract is "ask the coordinator", not "send".
  const freedClaims: number[] = []
  const claimFreedPtyRef = { current: () => freedClaims.push(1) } as {
    current: (() => void) | null
  }
  const view = renderHook(() =>
    useTerminalOwnership({
      id: "p1",
      kind: opts.kind ?? "agent",
      conn: "open",
      ptyRef: { current: pty as unknown as PtySocket },
      focusTypingSurface: () => focuses.push(1),
      setReconnecting: (v) => reconnecting.push(v),
      claimFreedPtyRef,
    }),
  )
  return { view, pty, focuses, reconnecting, freedClaims, claimFreedPtyRef }
}

beforeEach(() => {
  ledger.length = 0
  resetPtyOwnerEpochs()
  setVisibility("visible")
})
afterEach(() => {
  vi.restoreAllMocks()
  setVisibility("visible")
})

describe("the initial verdict", () => {
  it("claims on attach when the document is foregrounded", () => {
    const { view } = setup()
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.ownership.read()).toBe(true)
    expect(ledger).toEqual([["p1", "mine"]])
  })

  it("publishes nothing at all for a companion terminal", () => {
    setup({ kind: "terminal" })
    expect(ledger).toEqual([])
  })
})

describe("a handover", () => {
  it("demotes this client when the claimer's id is somebody else's", () => {
    const { view } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.isOwner).toBe(false)
    // The channel flips synchronously, so an in-flight keystroke is gated at
    // once rather than after the re-render.
    expect(view.result.current.ownership.read()).toBe(false)
    expect(view.result.current.takeoverLabel).not.toBeNull()
    expect(ledger.at(-1)).toEqual(["p1", "elsewhere"])
  })

  it("keeps this client the owner when the claimer's id is its own", () => {
    const { view } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    act(() => {
      notifyPtyOwner("p1", "mine", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("reads a NULL id as 'not us', so a pre-connected handover is safe", () => {
    const { view } = setup()
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, undefined)
    })
    expect(view.result.current.isOwner).toBe(false)
  })

  it("ignores a handover for another pty entirely", () => {
    const { view } = setup()
    act(() => {
      notifyPtyOwner("other", "theirs", 1, undefined)
    })
    expect(view.result.current.isOwner).toBe(true)
  })
})

// SITE 4. The handshake is now what decides, and the foreground guess survives
// only for an UNOWNED pty. Without this a phone opening a desktop-driven agent
// would sit under typing surfaces whose every keystroke the server drops, with
// no card, forever: a refused claim is silent by design.
describe("seeding the verdict from the connected handshake", () => {
  it("demotes a foregrounded pane that joined a pty somebody else drives", () => {
    const { view } = setup()
    expect(view.result.current.isOwner).toBe(true) // the pre-handshake guess
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownership.read()).toBe(false)
    expect(view.result.current.ownerPresent).toBe(true)
  })

  it("keeps a foregrounded pane the owner of an UNOWNED pty", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", null))
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.ownerPresent).toBe(false)
  })

  it("leaves a BACKGROUNDED pane a watcher of an unowned pty", () => {
    setVisibility("hidden")
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", null))
    expect(view.result.current.isOwner).toBe(false)
    // And it says so honestly rather than naming a device.
    expect(view.result.current.ownerPresent).toBe(false)
  })

  it("falls back to the foreground guess against a server that omits the owner", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", undefined))
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.ownerPresent).toBe(true)
  })

  // THE CROSS-SOCKET RACE, replayed exactly. The handshake rides the PTY socket
  // and `pty.owner` rides the events socket; nothing orders the two TCP
  // connections. Here the fresh `pty.owner{owner:B, epoch:1}` is applied FIRST
  // and the stale `connected{owner:null, owner_epoch:0}` lands second. Without
  // the epoch deferral the seed would read null+foreground as "claim it" and
  // wedge this pane as a phantom owner forever: a plain resize is refused
  // silently, and the stale-null direction emits no correcting event.
  it("does NOT become owner from a stale null handshake after a newer pty.owner", () => {
    const { view } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    // The newer handover is applied first: another device claimed, epoch 1.
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.isOwner).toBe(false)
    // The stale handshake, snapshotted before that claim, arrives second.
    act(() => view.result.current.seedFromConnected("mine", null, 0))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownership.read()).toBe(false)
    // ownerPresent is deferred on the same comparison: somebody IS driving.
    expect(view.result.current.ownerPresent).toBe(true)
    // And the demoting device's name survives the stale frame too.
    expect(view.result.current.takeoverLabel).not.toBeNull()
  })

  it("seeds normally from a handshake at or after the applied epoch", () => {
    const { view } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, undefined)
    })
    expect(view.result.current.isOwner).toBe(false)
    // The owner then released and this pane reconnected: the handshake's
    // snapshot (epoch 2, unowned) is strictly newer than the applied handover,
    // so the foreground guess claims exactly as a fresh attach would.
    act(() => view.result.current.seedFromConnected("mine2", null, 2))
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.ownerPresent).toBe(false)
  })

  // The bounce's own handshake still reports the OLD owner: the claim rides the
  // first resize of the new connection, so ownership lags by one replay parse.
  // Demoting here would flash the card back over a pane the user just took.
  it("does not demote a pane whose take-over is armed and mid-bounce", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    act(() => view.result.current.seedFromConnected("mine2", "theirs"))
    expect(view.result.current.isOwner).toBe(true)
  })
})

describe("taking over", () => {
  it("arms the intent, flips the verdict, bounces the socket, and refocuses", () => {
    const { view, pty, focuses, reconnecting } = setup()
    act(() => {
      view.result.current.connId.write("mine")
      notifyPtyOwner("p1", "theirs", 1, undefined)
    })
    expect(view.result.current.isOwner).toBe(false)

    act(() => view.result.current.takeOver())
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    // A FRESH ATTACH, always: nothing is written down the live socket, because
    // a live claim leaves this client's polluted viewer-era scrollback exactly
    // where it is.
    expect(pty.connects).toBe(1)
    // The bounce is visible. A deliberate `connect()` fires no `onReconnecting`
    // of its own, so the half-second window would otherwise read as dead.
    expect(reconnecting).toEqual([true])
    expect(focuses).toHaveLength(1)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("is a NO-OP while the bounce is still in flight", () => {
    const { view, pty } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    act(() => view.result.current.takeOver())
    act(() => view.result.current.takeOver())
    expect(pty.connects).toBe(1)
    expect(view.result.current.takeoverIntent.read()).toBe(true)
  })

  // The intent is state, not a queued frame, precisely so it survives the
  // socket churn a bounce is made of. It is spent by the lifecycle's confirmed
  // write, which this hook does not perform.
  it("keeps the intent armed across a socket bounce until something writes it", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    // The bounce's reconnect: a new id, and the handshake still naming the old
    // owner (the claim has not gone out yet).
    act(() => view.result.current.seedFromConnected("mine2", "theirs"))
    expect(view.result.current.takeoverIntent.read()).toBe(true)
  })

  it("retires an armed intent WITHOUT sending it when a handover demotes us", () => {
    const { view } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    // Somebody else's claim landed first; this client lost the race. Re-arming
    // is the user's decision, not a retry loop's.
    act(() => notifyPtyOwner("p1", "theirs", 2, undefined))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    expect(view.result.current.isOwner).toBe(false)
    // And the button works again, because the intent is no longer armed.
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
  })
})

// SITE 5. The driver's socket closed and the server broadcast an owner-cleared
// `pty.owner`. Ownership no longer follows focus, so without this the card
// would be a permanent lie about a browser tab that has gone.
describe("a freed pty", () => {
  it("is claimed by a mounted, FOREGROUNDED viewer", () => {
    const { view, freedClaims } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    expect(view.result.current.isOwner).toBe(false)

    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.isOwner).toBe(true)
    // Through the coordinator's port, never straight at the socket.
    expect(freedClaims).toHaveLength(1)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("leaves a BACKGROUNDED viewer watching, and says nobody is driving", () => {
    const { view, freedClaims } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    setVisibility("hidden")
    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(false)
    expect(freedClaims).toHaveLength(0)
  })

  it("does not send through a port the lifecycle has already torn down", () => {
    const { view, claimFreedPtyRef } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    claimFreedPtyRef.current = null
    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    // No throw, and the verdict still flips: the pane believes it owns an
    // unowned pty, and its next ordinary resize claims it.
    expect(view.result.current.isOwner).toBe(true)
  })
})

describe("the LOST state", () => {
  it("publishes no verdict at all once the socket has given up", () => {
    const { view } = setup()
    expect(ledger.at(-1)).toEqual(["p1", "mine"])
    act(() => view.result.current.setConnectionLost(true))
    expect(ledger.at(-1)).toEqual(["p1", "unknown"])
  })

  it("resumes publishing when the socket comes back", () => {
    const { view } = setup()
    act(() => view.result.current.setConnectionLost(true))
    act(() => view.result.current.setConnectionLost(false))
    expect(ledger.at(-1)).toEqual(["p1", "mine"])
  })

  it("hands the answer back to the spine on unmount", () => {
    const { view } = setup()
    view.unmount()
    expect(ledger.at(-1)).toEqual(["p1", "unknown"])
  })
})

describe("the other device's NAME across an events-socket outage", () => {
  it("is dropped whenever the events socket is not open, while the verdict stands", () => {
    const pty = new PtyFake()
    const view = renderHook(
      ({ conn }: { conn: "open" | "connecting" }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn,
          ptyRef: { current: pty as unknown as PtySocket },
          focusTypingSurface: () => {},
          setReconnecting: () => {},
          claimFreedPtyRef: { current: null },
        }),
      { initialProps: { conn: "open" as const } },
    )
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.takeoverLabel).not.toBeNull()
    view.rerender({ conn: "connecting" })
    // The generic copy is never wrong; the specific name might be.
    expect(view.result.current.takeoverLabel).toBeNull()
    expect(view.result.current.isOwner).toBe(false)
  })
})
