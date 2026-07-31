// @vitest-environment jsdom
//
// The client half of the reconnect-repaint contract.
//
// `TerminalPane` resets xterm before applying a reconnect replay (so the replayed
// scrollback replaces the stale buffer instead of stacking on it). `reset()`
// clears every private MODE along with the cells, and the child process only ever
// emitted its modes once, at its own startup, so nothing on the live byte stream
// puts them back. That is why `dux_core::pty::mode_restore_sequence` appends a
// full mode assignment to every repaint.
//
// These tests pin what that block has to achieve on the client, using a REAL
// xterm rather than a stub, because the thing under test is xterm's own mode
// bookkeeping. The touch-scroll forward path in `TerminalPane.onTouchMove` is
// gated on exactly the two values asserted here:
//
//     const altScreen = term.buffer.active.type !== "normal"
//     const forwardWheel = altScreen && isOwner && term.modes.mouseTrackingMode !== "none"
//     if (altScreen && !forwardWheel) return
//
// so on a full-screen agent a lost mouse mode is not a degraded scroll, it is no
// scroll at all: the handler returns before it touches the finger delta.
//
// Honest scope note: these assertions pass against the FIXED server and would
// also pass against any other producer of the same bytes. They cannot fail for
// the unfixed server, because the unfixed bytes are what the "without the
// restore block" case below feeds in deliberately. The test that actually fails
// without the fix is the Rust one, `reconnect_repaint_restores_private_modes_*`
// in crates/dux-core/src/pty.rs; this file exists so a future change to the mode
// block (or an xterm upgrade that renames a mode) is caught on this side too.
import { afterEach, describe, expect, it } from "vitest"
import { Terminal } from "@xterm/xterm"

function drain(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve))
}

// The alt-screen shape of a repaint: enter the alternate buffer, clear, paint,
// place the cursor. This is what the server sent before the mode block existed.
const ALT_REPAINT_CELLS_ONLY =
  "\x1b[?1049h\x1b[2J\x1b[H\x1b[0m\x1b[39;49magent ui\x1b[0m\x1b[1;9H"

// The tail `mode_restore_sequence` now appends, for a child on the alt screen
// with button-event tracking in SGR encoding, bracketed paste on and the cursor
// hidden. Both polarities are always emitted, so this is a full assignment.
// Copied verbatim from what dux-core actually produces for that child (checked
// against `mode_restore_sequence` byte for byte), not hand-written: note that the tracking disables (1000l, 1003l) come BEFORE the enable (1002h),
// because xterm collapses the three into one active protocol and a later disable
// would clear it. The last two before the keypad byte are the ANSI (non-private,
// no `?`) pair: IRM insert mode (4) and LNM line-feed/new-line mode (20).
//
// These strings are hand-fed rather than generated, so nothing here fails if the
// Rust side changes shape; keep them true by hand. There is deliberately no
// cross-language pin.
const MODE_RESTORE_TAIL =
  "\x1b[?1l\x1b[?7h\x1b[?25l" +
  "\x1b[?1000l\x1b[?1003l\x1b[?1002h\x1b[?1004l\x1b[?1005l\x1b[?1006h\x1b[?1007h" +
  "\x1b[?2004h\x1b[4l\x1b[20l\x1b>"

// The main-screen tail for a shell with bracketed paste on and autowrap off.
const MAIN_MODE_RESTORE_TAIL =
  "\x1b[?1l\x1b[?7l\x1b[?25h" +
  "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1007h" +
  "\x1b[?2004h\x1b[4l\x1b[20l\x1b>"

// The same main-screen tail for a child that IS sitting in insert mode.
const MAIN_MODE_RESTORE_TAIL_INSERT_ON = MAIN_MODE_RESTORE_TAIL.replace(
  "\x1b[4l",
  "\x1b[4h",
)

// Mirrors `TerminalPane.onTouchMove`'s gate. A finger drag on the alt screen is
// forwarded to the app as SGR wheel reports only when this returns "forward";
// "ignore" is the dead gesture the user reported.
function touchScrollTarget(term: Terminal, isOwner: boolean) {
  const altScreen = term.buffer.active.type !== "normal"
  const forwardWheel =
    altScreen && isOwner && term.modes.mouseTrackingMode !== "none"
  if (!altScreen) return "local"
  return forwardWheel ? "forward" : "ignore"
}

describe("reconnect repaint, client side", () => {
  let terms: Terminal[] = []
  const make = () => {
    const t = new Terminal({ cols: 40, rows: 6 })
    terms.push(t)
    return t
  }
  afterEach(() => {
    for (const t of terms) t.dispose()
    terms = []
  })

  it("leaves a reset terminal with no mouse tracking when the repaint carries only cells", async () => {
    const term = make()
    // The state a live client is in before the drop: the child enabled tracking
    // at startup and xterm has been tracking it ever since.
    await drain(term, "\x1b[?1049h\x1b[?1002h\x1b[?1006hagent ui")
    expect(term.modes.mouseTrackingMode).not.toBe("none")
    expect(touchScrollTarget(term, true)).toBe("forward")

    // The reconnect: reset, then apply a cells-only replay.
    term.reset()
    await drain(term, ALT_REPAINT_CELLS_ONLY)

    expect(term.buffer.active.type).toBe("alternate")
    expect(term.modes.mouseTrackingMode).toBe("none")
    expect(touchScrollTarget(term, true)).toBe("ignore")
  })

  it("restores mouse tracking, so a touch drag is forwarded again after a reconnect", async () => {
    const term = make()
    await drain(term, "\x1b[?1049h\x1b[?1002h\x1b[?1006hagent ui")

    term.reset()
    await drain(term, ALT_REPAINT_CELLS_ONLY + MODE_RESTORE_TAIL)

    expect(term.buffer.active.type).toBe("alternate")
    expect(term.modes.mouseTrackingMode).not.toBe("none")
    expect(term.modes.bracketedPasteMode).toBe(true)
    expect(touchScrollTarget(term, true)).toBe("forward")
  })

  it("gives a first-open terminal the same state a reconnect gets", async () => {
    // The hole in the original diagnosis: a hard refresh builds a brand-new
    // terminal whose modes also default to none, so cells-only leaves the two
    // paths in the SAME (broken) state and the refresh could only ever be fixed
    // by the child redrawing. With the mode block both paths land identically
    // and correctly, with no dependency on the child re-emitting anything.
    const fresh = make()
    await drain(fresh, ALT_REPAINT_CELLS_ONLY + MODE_RESTORE_TAIL)

    const reconnected = make()
    await drain(reconnected, "\x1b[?1049h\x1b[?1002h\x1b[?1006holder output")
    reconnected.reset()
    await drain(reconnected, ALT_REPAINT_CELLS_ONLY + MODE_RESTORE_TAIL)

    expect(reconnected.modes.mouseTrackingMode).toBe(
      fresh.modes.mouseTrackingMode,
    )
    expect(reconnected.buffer.active.type).toBe(fresh.buffer.active.type)
    expect(touchScrollTarget(reconnected, true)).toBe(
      touchScrollTarget(fresh, true),
    )
    expect(touchScrollTarget(fresh, true)).toBe("forward")
  })

  it("still ignores the drag for a read-only viewer", async () => {
    // Ownership is the other half of the gate and the mode restore must not
    // change it: a non-owner has nothing to forward to.
    const term = make()
    await drain(term, ALT_REPAINT_CELLS_ONLY + MODE_RESTORE_TAIL)
    expect(touchScrollTarget(term, false)).toBe("ignore")
  })

  it("restores insert mode, so typed characters push the rest of the line right", async () => {
    // The ANSI half of the block. Insert mode is the one whose loss is visible
    // immediately: with it lost the client OVERWRITES at the cursor where the
    // program expects each character to shove the rest of the line along.
    const term = make()
    term.reset()
    await drain(term, "abcdef\x1b[1;1H" + MAIN_MODE_RESTORE_TAIL_INSERT_ON)
    expect(term.modes.insertMode).toBe(true)

    await drain(term, "XY")
    expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe(
      "XYabcdef",
    )
  })

  it("clears a stale insert mode a client arrived carrying", async () => {
    // Both polarities are always emitted, so a client re-used from a program
    // that was in insert mode comes back overwriting again.
    const term = make()
    await drain(term, "\x1b[4h")
    expect(term.modes.insertMode).toBe(true)

    await drain(term, "\x1b[2J\x1b[Habcdef\x1b[1;1H" + MAIN_MODE_RESTORE_TAIL)
    expect(term.modes.insertMode).toBe(false)

    await drain(term, "XY")
    expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe(
      "XYcdef",
    )
  })

  it("keeps the local scroll path on the main screen", async () => {
    // The main-screen repaint restores autowrap to whatever the child has after
    // forcing it on to rebuild soft wraps. Either way the buffer stays normal,
    // so a drag scrolls xterm's own scrollback at full magnitude.
    const term = make()
    const mainRepaint =
      "\x1b[?1049l\x1b[?7h\x1b[2J\x1b[3J\x1b[H\x1b[0mprompt$ \x1b[0m\x1b[1;9H"
    term.reset()
    await drain(term, mainRepaint + MAIN_MODE_RESTORE_TAIL)

    expect(term.buffer.active.type).toBe("normal")
    expect(term.modes.wraparoundMode).toBe(false)
    expect(term.modes.bracketedPasteMode).toBe(true)
    expect(touchScrollTarget(term, true)).toBe("local")
  })
})
