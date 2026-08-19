// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

import type { Terminal } from "@xterm/xterm"

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
  resizes: [number, number][] = []
  /// Whether a resize frame actually goes on the wire. A socket mid-reconnect
  /// answers false, which is the third health check `takeOver` makes.
  wire = true
  connect() {
    this.connects++
  }
  sendResize(rows: number, cols: number) {
    if (!this.wire) return false
    this.resizes.push([rows, cols])
    return true
  }
}

function setup(opts: { kind?: "agent" | "terminal" } = {}) {
  const term = { rows: 24, cols: 80 } as unknown as Terminal
  const pty = new PtyFake()
  const focuses: number[] = []
  const view = renderHook(() =>
    useTerminalOwnership({
      id: "p1",
      kind: opts.kind ?? "agent",
      conn: "open",
      termRef: { current: term },
      ptyRef: { current: pty as unknown as PtySocket },
      focusTypingSurface: () => focuses.push(1),
    }),
  )
  return { view, pty, focuses }
}

beforeEach(() => {
  ledger.length = 0
  resetPtyOwnerEpochs()
})
afterEach(() => {
  vi.restoreAllMocks()
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

describe("taking over", () => {
  it("flips the verdict, sends the claim, and refocuses", () => {
    const { view, pty, focuses } = setup()
    act(() => {
      view.result.current.connId.write("mine")
      notifyPtyOwner("p1", "theirs", 1, undefined)
    })
    expect(view.result.current.isOwner).toBe(false)
    act(() => view.result.current.takeOver())
    expect(view.result.current.isOwner).toBe(true)
    expect(pty.resizes).toEqual([[24, 80]])
    expect(pty.connects).toBe(0)
    expect(focuses).toHaveLength(1)
    expect(view.result.current.takeoverLabel).toBeNull()
  })

  it("PARKS the claim and reopens the socket when the id is not known yet", () => {
    const { view, pty } = setup()
    act(() => view.result.current.takeOver())
    expect(view.result.current.pendingClaimRef.current).toBe(true)
    expect(pty.connects).toBe(1)
    expect(pty.resizes).toEqual([])
  })

  it("PARKS it when the socket could not carry the frame, even though it looked open", () => {
    const { view, pty } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    pty.wire = false
    act(() => view.result.current.takeOver())
    expect(view.result.current.pendingClaimRef.current).toBe(true)
    expect(pty.connects).toBe(1)
  })

  it("PARKS it when the socket is closed", () => {
    const { view, pty } = setup()
    act(() => {
      view.result.current.connId.write("mine")
    })
    pty.isOpen = false
    act(() => view.result.current.takeOver())
    expect(view.result.current.pendingClaimRef.current).toBe(true)
    expect(pty.connects).toBe(1)
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
    const term = { rows: 24, cols: 80 } as unknown as Terminal
    const pty = new PtyFake()
    const view = renderHook(
      ({ conn }: { conn: "open" | "connecting" }) =>
        useTerminalOwnership({
          id: "p1",
          kind: "agent",
          conn,
          termRef: { current: term },
          ptyRef: { current: pty as unknown as PtySocket },
          focusTypingSurface: () => {},
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
