// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { PtySocket } from "@/lib/ptySocket"

import { channel } from "./channels"
import { VIEWER_HEAL_DEBOUNCE_MS } from "./constants"
import { gridsDiverge, shouldHealByReattaching, useViewerGrid } from "./viewerGrid"

describe("gridsDiverge", () => {
  it("is true only when both grids are known and they differ", () => {
    expect(gridsDiverge({ rows: 24, cols: 80 }, { rows: 40, cols: 120 })).toBe(
      true,
    )
    expect(gridsDiverge({ rows: 24, cols: 80 }, { rows: 24, cols: 120 })).toBe(
      true,
    )
    expect(gridsDiverge({ rows: 24, cols: 80 }, { rows: 40, cols: 80 })).toBe(
      true,
    )
    expect(gridsDiverge({ rows: 24, cols: 80 }, { rows: 24, cols: 80 })).toBe(
      false,
    )
  })

  it("treats an unknown grid on either side as nothing to claim", () => {
    // An old server reports no grid, and the local grid is unknown before the
    // first fit. Both are "we do not know", and a badge shown on a guess is
    // worse than no badge: the user cannot falsify it.
    expect(gridsDiverge(null, { rows: 40, cols: 120 })).toBe(false)
    expect(gridsDiverge({ rows: 24, cols: 80 }, null)).toBe(false)
    expect(gridsDiverge(null, null)).toBe(false)
  })
})

describe("shouldHealByReattaching", () => {
  // A watcher hearing a real change with nothing else in flight: the one case
  // that heals. Every field below is flipped from this baseline one at a time.
  const healing = {
    isOwner: false,
    takeoverArmed: false,
    bounceInFlight: false,
    hasSocket: true,
    fromHandshake: false,
    changed: true,
  }

  it("heals a watcher on a real change", () => {
    expect(shouldHealByReattaching(healing)).toBe(true)
  })

  it("never heals the owner", () => {
    // The owner's own resize is echoed back to it; bouncing on it would
    // reconnect the person doing the dragging on every window drag.
    expect(shouldHealByReattaching({ ...healing, isOwner: true })).toBe(false)
  })

  it("stands down while a take-over is armed", () => {
    // Take-over is itself a bounce carrying an intent. A second `connect()` on
    // top of it closes a socket that is still opening, and the claim rides
    // nothing.
    expect(shouldHealByReattaching({ ...healing, takeoverArmed: true })).toBe(
      false,
    )
  })

  it("stands down while a bounce is already in flight", () => {
    expect(shouldHealByReattaching({ ...healing, bounceInFlight: true })).toBe(
      false,
    )
  })

  it("does nothing for a pane with no socket", () => {
    // A dormant tab is never mounted (subscribing force-launches it), so there
    // is nothing to reconnect and nothing to heal.
    expect(shouldHealByReattaching({ ...healing, hasSocket: false })).toBe(false)
  })

  it("never heals on the handshake's own grid", () => {
    // A fresh attach has just rebuilt its buffer from the server's repaint;
    // bouncing on the grid that attach reported would loop forever.
    expect(shouldHealByReattaching({ ...healing, fromHandshake: true })).toBe(
      false,
    )
  })

  it("ignores a re-announcement of the grid it already knew", () => {
    expect(shouldHealByReattaching({ ...healing, changed: false })).toBe(false)
  })
})

describe("useViewerGrid", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  // A minimal socket double: the machine only ever calls `connect()` on it.
  function fakePty(): { connect: () => void; connects: () => number } {
    let connects = 0
    return {
      connect: () => {
        connects += 1
      },
      connects: () => connects,
    }
  }

  function mount() {
    const pty = fakePty()
    const ptyRef = { current: pty as unknown as PtySocket }
    const ownership = channel(false)
    let armed = false
    const takeoverIntent = {
      read: () => armed,
      arm: () => {
        armed = true
      },
      clear: () => {
        armed = false
      },
    }
    const view = renderHook(() =>
      useViewerGrid({
        ptyRef,
        ownership,
        takeoverIntent,
        setReconnecting: () => {},
      }),
    )
    return { pty, view, ownership }
  }

  it("clears an armed heal when the socket opens, so a reconnect that lands first is not bounced again", () => {
    // The heal exists to force a fresh attach. If some OTHER reconnect (a
    // network blip, a take-over) opens the socket while the timer is armed,
    // that open has already rebuilt the buffer, and the armed timer surviving
    // it would fire a redundant bounce at the just-healed socket.
    const { pty, view } = mount()
    // Arm via a real change after an attach: handshake grid first, then a
    // different announced grid.
    act(() => {
      view.result.current.noteRemoteGrid({ rows: 24, cols: 80 }, true)
      view.result.current.noteRemoteGrid({ rows: 30, cols: 100 }, false)
    })
    // An unrelated reconnect lands before the debounce elapses: the socket
    // opens and its handshake re-announces the grid it attached at.
    act(() => {
      view.result.current.noteSocketOpen()
      view.result.current.noteRemoteGrid({ rows: 30, cols: 100 }, true)
    })
    act(() => {
      vi.advanceTimersByTime(VIEWER_HEAL_DEBOUNCE_MS * 2)
    })
    expect(pty.connects()).toBe(0)
  })

  it("still bounces once when nothing intervenes before the debounce elapses", () => {
    // The counterpart that proves the clear above is not simply disabling the
    // machine: left alone, the armed heal fires exactly one connect().
    const { pty, view } = mount()
    act(() => {
      view.result.current.noteRemoteGrid({ rows: 24, cols: 80 }, true)
      view.result.current.noteRemoteGrid({ rows: 30, cols: 100 }, false)
    })
    act(() => {
      vi.advanceTimersByTime(VIEWER_HEAL_DEBOUNCE_MS * 2)
    })
    expect(pty.connects()).toBe(1)
  })

  it("stands down at firing time when the client became the owner meanwhile", () => {
    // The firing-time re-check routes through the same decision table as
    // arming (shouldHealByReattaching), so the guards cannot drift apart.
    const { pty, view, ownership } = mount()
    act(() => {
      view.result.current.noteRemoteGrid({ rows: 24, cols: 80 }, true)
      view.result.current.noteRemoteGrid({ rows: 30, cols: 100 }, false)
    })
    // A handover makes this client the owner inside the debounce window: the
    // armed timer still fires, but the live guard read stands it down.
    ownership.write(true)
    act(() => {
      vi.advanceTimersByTime(VIEWER_HEAL_DEBOUNCE_MS * 2)
    })
    expect(pty.connects()).toBe(0)
  })
})
