// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"
import { noteServerRunProbe } from "@/lib/serverRun"
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
/// guess and the self-succession rule both turn on it.
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
}

function setup(
  opts: {
    kind?: "agent" | "terminal"
    conn?: "open" | "connecting"
    spineInputOwner?: string | null
  } = {},
) {
  const pty = new PtyFake()
  const reconnecting: boolean[] = []
  // Rendered through props rather than a captured constant so a test can move
  // the SPINE under a mounted pane, which is the only way the spine correction
  // ever happens in the app.
  const view = renderHook(
    ({ spineInputOwner }: { spineInputOwner?: string | null }) =>
      useTerminalOwnership({
        id: "p1",
        kind: opts.kind ?? "agent",
        conn: opts.conn ?? "open",
        spineInputOwner,
        ptyRef: { current: pty as unknown as PtySocket },
        setReconnecting: (v) => reconnecting.push(v),
      }),
    { initialProps: { spineInputOwner: opts.spineInputOwner } },
  )
  const setSpineInputOwner = (spineInputOwner: string | null | undefined) => {
    act(() => view.rerender({ spineInputOwner }))
  }
  return { view, pty, reconnecting, setSpineInputOwner }
}

beforeEach(() => {
  ledger.length = 0
  resetPtyOwnerEpochs()
  setVisibility("visible")
  // The run-identity fan-out is module state shared by every test in this file,
  // and self-succession now depends on it. Start each test from the state a
  // freshly loaded tab is in: the run is confirmed to be the one that served it.
  noteServerRunProbe("same")
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

  // THE MERE-ATTACH NAME, the user-reported regression. A watcher that simply
  // attaches hears no `pty.owner` broadcast (attaching never steals and a
  // refused claim emits nothing), so the handshake's `owner_device` is its only
  // source of a specific card title.
  it("names the driving device from the handshake alone, with no pty.owner event", () => {
    const { view } = setup()
    act(() =>
      view.result.current.seedFromConnected(
        "mine",
        "theirs",
        1,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      ),
    )
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
  })

  // A name planted while the EVENTS socket is down could never be corrected
  // (pty.owner broadcasts are live-only, and they are the one channel that
  // updates a name), so the handshake's name half is gated on that socket
  // being open. The VERDICT half still seeds: the generic title is never
  // wrong, a stale specific name is.
  it("seeds the verdict but not the name while the events socket is down", () => {
    const { view } = setup({ conn: "connecting" })
    act(() =>
      view.result.current.seedFromConnected(
        "mine",
        "theirs",
        1,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      ),
    )
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(true)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  // An old server sends no `owner_device` (it omits `owner` too), and an owner
  // that presented no User-Agent has nothing to be named by: both fall back to
  // the generic title rather than a guess.
  it("leaves the title generic when the handshake carries no device", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs", 1))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverLabel).toBeNull()
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
  // FLIPPED. The press used to focus the typing surface at once, which on a
  // phone raises the soft keyboard over a pane that is a whole reconnect and one
  // replay parse away from having anything to type into. Focus is the pane's now,
  // and it waits for the handshake AND the replay.
  it("arms the intent, flips the verdict and bounces the socket, WITHOUT focusing", () => {
    const { view, pty, reconnecting } = setup()
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
// `pty.owner`. LOSING OWNERSHIP IS STICKY: the broadcast re-titles the card and
// claims nothing, whatever this pane's visibility is. Sitting on an open card
// must never win the pty back, because that passive path is exactly what let an
// idle desktop beat the returning owner to its own pty.
describe("a freed pty", () => {
  it("does NOT claim, even for a mounted, FOREGROUNDED viewer", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    expect(view.result.current.isOwner).toBe(false)

    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownership.read()).toBe(false)
    // Only the card's copy moves: nobody is driving, and this pane is still a
    // watcher until its human says otherwise.
    expect(view.result.current.ownerPresent).toBe(false)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("leaves a BACKGROUNDED viewer watching, and says nobody is driving", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    setVisibility("hidden")
    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(false)
  })

  it("demotes a pane that believed it was the owner, because the server says nobody is", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", null))
    expect(view.result.current.isOwner).toBe(true)
    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(false)
  })

  // FLIPPED. The freed exemption is gone. It used to park an armed intent
  // through the old owner's disconnect so a mid-bounce take-over could still
  // claim flagged; the bounce's own handshake finds the pty UNOWNED and seeds a
  // PLAIN claim that reaches exactly the same outcome, and keeping a flag alive
  // past its socket is the whole class of bug the rule closes.
  it("DROPS an armed take-over intent, and the next unowned handshake claims plain", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    // The old owner disconnects mid-bounce and the server broadcasts
    // owner-cleared.
    act(() => notifyPtyOwner("p1", undefined, 2, undefined))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    // The bounce's handshake then finds the pty unowned, and a foregrounded page
    // claims it plainly. Same outcome, no parked flag.
    act(() => view.result.current.seedFromConnected("mine2", null, 3))
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })

  it("lets a SECOND press bounce again once the first bounce's socket closed", () => {
    const { view, pty } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    expect(pty.connects).toBe(1)
    // A second press while the bounce is still in flight is a no-op guard: the
    // intent is armed and the frame carrying it has not gone out.
    act(() => view.result.current.takeOver())
    expect(pty.connects).toBe(1)
    // The bounce FAILED: the socket dropped. The intent dies with it, so the
    // button works again rather than silently doing nothing.
    act(() => view.result.current.notePtyConn("closed"))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    act(() => view.result.current.takeOver())
    expect(pty.connects).toBe(2)
  })

  it("clears an armed intent only for a genuine lost race: an event naming ANOTHER owner", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    // Somebody else's claim landed first. This is the demotion that retires
    // the intent without sending it; re-arming is the user's decision.
    act(() => notifyPtyOwner("p1", "conn-other", 2, undefined))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    expect(view.result.current.isOwner).toBe(false)
  })
})

// SELF-SUCCESSION. The server's liveness reap is send-failure based and takes
// tens of seconds; a blipped client is back in about one, with a fresh
// connection id. Its handshake therefore names its OWN dead previous id as the
// owner, and a plain id comparison would demote the returning driver to a
// watcher of its own ghost, with no later event to correct it (the reap's
// release finds a different owner by then and broadcasts nothing).
describe("self-succession after a blipped socket", () => {
  it("claims back a pty the handshake says our own DEAD connection owns", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    // The drop: the lifecycle retires the id, and the machine remembers it.
    act(() => view.result.current.connId.write(null))
    // The reconnect's handshake, a second later: the server has not reaped
    // conn-a yet, so it still names it as the driver.
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.isOwner).toBe(true)
    // Flagged, because the server grants a flagged claim against ANY owner and
    // the owner being displaced here is our own ghost. Nothing is stolen.
    expect(view.result.current.takeoverIntent.read()).toBe(true)
  })

  it("does NOT self-succeed while the page is backgrounded", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    setVisibility("hidden")
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    // The backgrounded-owner contract stands: it returns as a watcher and its
    // human presses Take over.
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })

  it("stays a watcher when the handshake names somebody ELSE", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-other"))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })

  it("does not resurrect a ghost the events socket has already superseded", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    // Another device took the pty while this one was away, and that handover
    // reached this client first.
    act(() => notifyPtyOwner("p1", "conn-other", 5, undefined))
    // The reconnect's handshake was snapshotted before it and names our ghost.
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a", 3))
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })

  // AND IT KEEPS THE NAME THE NEWER EVENT WROTE. The seed runs from the PTY
  // socket, whose handlers were wired on the mount render, so the prior name has
  // to be read through a ref: passing the render closure's value pinned it to
  // null forever and quietly downgraded "Open on Chrome on macOS" to the generic
  // title on every superseded handshake.
  it("keeps the device name a superseding handover wrote", () => {
    const { view } = setup()
    // CAPTURED AT MOUNT, deliberately. The lifecycle wires `pty.onConnected`
    // once, from the render its attach effect ran on, so the seed the socket
    // actually calls is this closure and not the newest one. Calling the fresh
    // one would hide the bug entirely.
    const seedFromMountRender = view.result.current.seedFromConnected
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() =>
      notifyPtyOwner("p1", "conn-other", 5, "Mozilla/5.0 (Macintosh) Chrome/1"),
    )
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
    act(() => seedFromMountRender("conn-b", "conn-a", 3))
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
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
  // FLIPPED. Losing the events socket used to WIPE the name while
  // `ownerPresent` stayed true, so a flapping spine downgraded a perfectly good
  // "Open on Chrome on macOS" to the generic "Active on another device" and back
  // again. The wipe was defending against a name going stale with no correction
  // coming; the correction now exists (the spine's own `input_owner`), so the
  // name is kept and only ever replaced by a newer fact.
  it("is KEPT while the events socket is down, because the generic copy is worse, not safer", () => {
    const pty = new PtyFake()
    const view = renderHook(
      ({ conn }: { conn: "open" | "connecting" }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn,
          ptyRef: { current: pty as unknown as PtySocket },
          setReconnecting: () => {},
        }),
      { initialProps: { conn: "open" as const } },
    )
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
    view.rerender({ conn: "connecting" })
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
    expect(view.result.current.isOwner).toBe(false)
  })

  it("is corrected by the refetched spine once the events socket is back", () => {
    const pty = new PtyFake()
    const view = renderHook(
      ({
        conn,
        spineInputOwner,
      }: {
        conn: "open" | "connecting"
        spineInputOwner?: string | null
      }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn,
          spineInputOwner,
          ptyRef: { current: pty as unknown as PtySocket },
          setReconnecting: () => {},
        }),
      { initialProps: { conn: "open" as const, spineInputOwner: "theirs" } },
    )
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
    // The outage. The name stands.
    view.rerender({ conn: "connecting", spineInputOwner: "theirs" })
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
    // Back, and the refetched spine says a DIFFERENT connection drives it now.
    // The name describes a device that stopped driving, so it goes.
    view.rerender({ conn: "open", spineInputOwner: "somebody-else" })
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("keeps the name when the spine confirms the same driver", () => {
    const pty = new PtyFake()
    const view = renderHook(
      ({ spineInputOwner }: { spineInputOwner?: string | null }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn: "open",
          spineInputOwner,
          ptyRef: { current: pty as unknown as PtySocket },
          setReconnecting: () => {},
        }),
      { initialProps: { spineInputOwner: undefined } },
    )
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    view.rerender({ spineInputOwner: "theirs" })
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
  })

  it("takes an UNANSWERED spine as no evidence rather than as a correction", () => {
    const pty = new PtyFake()
    const view = renderHook(
      ({ spineInputOwner }: { spineInputOwner?: string | null }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn: "open",
          spineInputOwner,
          ptyRef: { current: pty as unknown as PtySocket },
          setReconnecting: () => {},
        }),
      { initialProps: { spineInputOwner: undefined } },
    )
    act(() => {
      notifyPtyOwner("p1", "theirs", 1, "Mozilla/5.0 (Macintosh) Chrome/1")
    })
    // An older server omits the field entirely; that is not a statement that
    // nobody drives.
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
  })
})

// THE TWO PROPERTIES THE RULE EXISTS FOR. An automatic reconnect is a plain
// attach and never a take-over: no retry, resume or heal bounce carries the
// flag, and only a press on the button does.
describe("an automatic reconnect never claims", () => {
  it("lands a reconnect against another driver as a WATCHER, with the card up", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    // Another device took over while we were attached.
    act(() =>
      notifyPtyOwner("p1", "conn-other", 2, "Mozilla/5.0 (Macintosh) Chrome/1"),
    )
    expect(view.result.current.isOwner).toBe(false)
    // The socket drops and the ordinary retry path brings it back. Nothing about
    // that is a claim.
    act(() => view.result.current.notePtyConn("closed"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.notePtyConn("connecting"))
    act(() => view.result.current.notePtyConn("open"))
    act(() =>
      view.result.current.seedFromConnected(
        "conn-b",
        "conn-other",
        3,
        "Mozilla/5.0 (Macintosh) Chrome/1",
      ),
    )
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    // The card is up, and the handshake's own owner snapshot names the driver.
    expect(view.result.current.ownerPresent).toBe(true)
    expect(view.result.current.takeoverLabel).toBe("Chrome on macOS")
  })

  it("spends a PRESSED intent on exactly one flagged resize, and never on a later reconnect", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    // A press names no expected owner: a press may take from anyone.
    expect(view.result.current.takeoverIntent.expectedOwner()).toBeUndefined()
    // The bounce's first resize carries the flag; the lifecycle clears the intent
    // on the confirmed write.
    act(() => view.result.current.takeoverIntent.clear())
    // Much later, the network drops and reconnects on its own. Nothing re-arms.
    act(() => view.result.current.notePtyConn("closed"))
    act(() => view.result.current.notePtyConn("connecting"))
    act(() => view.result.current.notePtyConn("open"))
    act(() => view.result.current.seedFromConnected("mine2", null, 4))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })

  it("drops an armed intent on ANY close of the socket it was armed for", () => {
    const { view } = setup()
    act(() => view.result.current.seedFromConnected("mine", "theirs"))
    act(() => view.result.current.takeOver())
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    act(() => view.result.current.notePtyConn("closed"))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
  })
})

describe("self-succession names the ghost it expects to displace", () => {
  it("sends the dead connection id as the expected owner", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    expect(view.result.current.takeoverIntent.expectedOwner()).toBe("conn-a")
  })

  it("recognises ANY id this pane has held, not merely the most recent", () => {
    const { view } = setup()
    // Two reconnects in a row on a flapping radio.
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.connId.write("conn-b"))
    act(() => view.result.current.connId.write(null))
    // The server has still not reaped conn-a, so its handshake names it.
    act(() => view.result.current.seedFromConnected("conn-c", "conn-a"))
    expect(view.result.current.isOwner).toBe(true)
    expect(view.result.current.takeoverIntent.expectedOwner()).toBe("conn-a")
  })

  // A RESTARTED SERVER RESTARTS THE ID COUNTER, so an id this pane held against
  // the previous run can be handed to somebody else's connection on the new one.
  // Self-succeeding on it would take a pty this pane never owned.
  it("forgets its ghosts when the server run has actually changed", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => noteServerRunProbe("changed"))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    expect(view.result.current.isOwner).toBe(false)
  })

  // THE ORDINARY MOBILE DROP, and the case self-succession exists for. A drop
  // takes BOTH sockets, so the events socket reconnects (resetting the epoch
  // high-water marks) strictly before the pty socket's handshake arrives. While
  // the ghosts rode that reset, the returning driver's own dead id was already
  // forgotten by the time its handshake named it, and the driver landed on the
  // take-over card: measured, three returns in four.
  it("keeps its ghosts across an ordinary events reconnect", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => resetPtyOwnerEpochs())
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    expect(view.result.current.takeoverIntent.expectedOwner()).toBe("conn-a")
    expect(view.result.current.isOwner).toBe(true)
  })

  // A PROBE THAT COULD NOT ANSWER REFUSES THE SUCCESSION, and this is the one
  // place the ergonomics lose to the rule. An unknown answer is not evidence of
  // a change, but it is not evidence of sameness either, and a restarted server
  // mints its connection ids from zero again: another device's fresh id can
  // equal one of this pane's stale ghosts, and succeeding on it would take a pty
  // this pane never owned with no press at all. So the pane lands as a watcher
  // and the user pays one tap.
  it("refuses self-succession while the run identity is unconfirmed", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => noteServerRunProbe("unknown"))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.takeoverIntent.read()).toBe(false)
    expect(view.result.current.takeoverIntent.expectedOwner()).toBe(undefined)
    expect(view.result.current.isOwner).toBe(false)
  })

  // And the succession comes back the moment the probe answers: the ghosts were
  // never wrong, only unproven.
  it("self-succeeds again once a probe confirms the run has not moved", () => {
    const { view } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => noteServerRunProbe("unknown"))
    act(() => noteServerRunProbe("same"))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.takeoverIntent.read()).toBe(true)
    expect(view.result.current.takeoverIntent.expectedOwner()).toBe("conn-a")
    expect(view.result.current.isOwner).toBe(true)
  })

  // THE REFUSAL IS SILENT, and the replaced version of this test did not
  // exercise it. It hand-delivered a `pty.owner` broadcast, which is the
  // CORRECTION channel: it would have passed with `expected_owner` never sent
  // and with the server granting every flagged claim. A refusal emits nothing
  // at all (the transfer changes nothing, so there is nothing to broadcast), so
  // the only thing that can ever tell this pane it lost is the spine's own
  // `input_owner`, refetched on every events open.
  it("lands as a WATCHER when the server silently refuses the succession", () => {
    const { view, setSpineInputOwner } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    // Optimistic, and until the spine says otherwise it is the right guess.
    expect(view.result.current.isOwner).toBe(true)
    // Somebody else legitimately claimed the pty in the gap, so the server
    // refuses the transfer inside its own critical section and says nothing.
    // The next spine read names them, and that is the whole answer.
    setSpineInputOwner("conn-other")
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(true)
  })

  // The other direction, and the reason the demotion is not simply "the spine
  // disagrees". A succession that will be GRANTED is exactly the case where the
  // pty is still recorded to this pane's own dead connection, so a spine read
  // naming that ghost must leave the returning driver alone.
  it("is NOT demoted by a spine read that still names this pane's own ghost", () => {
    const { view, setSpineInputOwner } = setup()
    act(() => view.result.current.connId.write("conn-a"))
    act(() => view.result.current.connId.write(null))
    act(() => view.result.current.seedFromConnected("conn-b", "conn-a"))
    expect(view.result.current.isOwner).toBe(true)
    setSpineInputOwner("conn-a")
    expect(view.result.current.isOwner).toBe(true)
  })
})

describe("the spine corrects a phantom owner", () => {
  it("demotes a pane the spine says another connection drives", () => {
    const { view, setSpineInputOwner } = setup()
    act(() => view.result.current.seedFromConnected("mine", null))
    expect(view.result.current.isOwner).toBe(true)
    setSpineInputOwner("conn-other")
    expect(view.result.current.isOwner).toBe(false)
    expect(view.result.current.ownerPresent).toBe(true)
  })

  it("leaves a genuinely in-flight PRESS alone, because a press cannot be refused", () => {
    const { view, setSpineInputOwner } = setup()
    act(() => view.result.current.seedFromConnected("mine", "conn-other"))
    expect(view.result.current.isOwner).toBe(false)
    act(() => view.result.current.takeOver())
    expect(view.result.current.isOwner).toBe(true)
    // A spine document rendered before the grant landed still names the device
    // that was driving. That is staleness, not refusal.
    setSpineInputOwner("conn-other")
    expect(view.result.current.isOwner).toBe(true)
  })

  it("says nothing when the spine has not answered, or says nobody drives", () => {
    const { view, setSpineInputOwner } = setup()
    act(() => view.result.current.seedFromConnected("mine", null))
    setSpineInputOwner(undefined)
    expect(view.result.current.isOwner).toBe(true)
    setSpineInputOwner(null)
    expect(view.result.current.isOwner).toBe(true)
  })

  it("never demotes on a spine read that names this pane itself", () => {
    const { view, setSpineInputOwner } = setup()
    // The lifecycle records the id before it seeds; mirror that, because the
    // correction compares the spine against this pane's OWN connection.
    act(() => view.result.current.connId.write("mine"))
    act(() => view.result.current.seedFromConnected("mine", null))
    setSpineInputOwner("mine")
    expect(view.result.current.isOwner).toBe(true)
  })
})
