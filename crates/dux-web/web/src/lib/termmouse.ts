/**
 * Forwarding a TOUCH gesture to a mouse-reporting app in the PTY.
 *
 * # Why this is a replay and not an encoder
 *
 * A terminal mouse report is not one wire format. The app chooses BOTH a
 * tracking protocol and an encoding, with separate DEC private modes, and the
 * two are independent:
 *
 * | DECSET | meaning                          | kind     |
 * | ------ | -------------------------------- | -------- |
 * | `?9`   | X10: press only, no modifiers     | protocol |
 * | `?1000`| VT200: press + release            | protocol |
 * | `?1002`| cell-motion (drag while pressed)  | protocol |
 * | `?1003`| any-motion                        | protocol |
 * | `?1005`| UTF-8 coordinates                 | encoding |
 * | `?1006`| SGR                               | encoding |
 * | `?1015`| urxvt                             | encoding |
 * | `?1016`| SGR pixels                        | encoding |
 *
 * dux used to hand-encode a left click as SGR (`ESC [ < 0 ; col ; row M/m`)
 * unconditionally, gated only on `term.modes.mouseTrackingMode !== "none"` —
 * which reports the PROTOCOL and says nothing at all about the ENCODING. An app
 * that enabled `?1000` or `?1002` WITHOUT `?1006` therefore received SGR text it
 * does not parse, and an app on `?9` received a release report it must never be
 * sent. The cell was computed with a second, parallel arithmetic
 * (`container.clientWidth / term.cols`) that does not match xterm's own
 * (`getMouseReportCoords`, which measures against the `.xterm-screen`
 * element, subtracts its CSS padding and divides by the MEASURED cell size), so
 * it landed on a different cell than a desktop click at the same point. That is
 * not theoretical: MEASURED on a 390px phone viewport in `tools/preview-env`,
 * clicking and then tapping the same 21 points, 15 of them reported a DIFFERENT
 * cell, drifting up to two columns by the far side. The container is 374px wide
 * where xterm's screen is 361px (the scrollbar gutter), so dividing the
 * container by the column count inflates every cell and the error accumulates
 * across the row. After this change the same sweep disagrees nowhere inside the
 * screen element.
 *
 * MEASURED against the installed `@xterm/xterm` 6.0.0 bundle
 * (`node_modules/@xterm/xterm/lib/xterm.mjs`):
 *
 *  - xterm implements exactly three encodings, `DEFAULT` (the classic X10 byte
 *    form `ESC [ M Cb Cx Cy`), `SGR` and `SGR_PIXELS`. `?1005` and `?1015` are
 *    parsed and DELIBERATELY IGNORED ("DECSET 1005 not supported (see #2507)"),
 *    so a browser-side encoder that supported them would be encoding for a
 *    state xterm can never be in.
 *  - the active encoding lives on `CoreMouseService._activeEncoding` behind an
 *    `activeEncoding` accessor. `IModes` publishes `mouseTrackingMode` and
 *    NOTHING about the encoding, so there is no public read of it. Reaching
 *    into `term._core` for it is not on the table.
 *  - `DEFAULT`-encoded reports are emitted through `triggerBinaryEvent`, i.e.
 *    `Terminal.onBinary`, NOT `onData` (`triggerMouseEvent` branches on
 *    `activeEncoding === "DEFAULT"`). A pane that subscribes only to `onData`
 *    silently drops every X10-encoded mouse report, including a real DESKTOP
 *    click. `TerminalPane` now subscribes to both.
 *
 * So this module does not encode anything. It REPLAYS the DOM mouse events the
 * browser's own synthetic ones would have been, straight at the element xterm
 * binds its mouse pipeline to, and lets xterm do the coordinate math, the
 * protocol gating (X10 sends no release; a wheel report is suppressed for a
 * protocol that did not ask for one) and the encoding. That is the same
 * technique, and the same reasoning, as `lib/termlink.ts`: drive xterm through
 * its own public contract instead of re-implementing a private one beside it.
 *
 * # What real agent CLIs ask for
 *
 * MEASURED by reading the shipped code of each CLI installed on a development
 * machine (static analysis, not a wire capture), plus upstream source where it
 * is published. Be honest about what this table says: EVERY measured CLI pairs
 * its tracking mode with `?1006`, so the old hardcoded SGR happened to be right
 * for all four defaults, and the encoding half of this change is insurance
 * rather than a fix for them. The CELL half is not insurance: it was wrong for
 * every one of them (see below).
 *
 * | CLI          | version | tracking                | encoding | source |
 * | ------------ | ------- | ----------------------- | -------- | ------ |
 * | Claude Code  | 2.1.227 | `?1000`+`?1002`+`?1003` | `?1006`  | its bundle, composed at runtime from a numeric mode table |
 * | opencode     | 1.17.10 | `?1000`+`?1002`+`?1003` | `?1006`  | OpenTUI's `terminal.zig` `setMouseMode`, cross-checked against the shipped native lib |
 * | Copilot CLI  | 1.0.73  | `?1002` (or `?1003`)    | `?1006`  | shipped bundle only; no source is published |
 * | Codex        | 0.145.0 | NONE                    | n/a      | no `EnableMouseCapture` anywhere; it uses `?1007` alternate scroll instead |
 *
 * Two things that table teaches, both of which argue for the replay:
 *  - crossterm (which any ratatui provider a user configures will pull in)
 *    writes `?1015` urxvt BEFORE `?1006`. xterm ignores 1015, so SGR still
 *    wins, but a browser-side encoder reasoning from "what did the app ask
 *    for?" would have had to know that.
 *  - dux's whole provider model is "any CLI can be a provider", so the set of
 *    apps whose mouse modes matter is open-ended by design. Four measured rows
 *    are not a licence to hardcode the fifth.
 *
 * One gap is known and NOT fixed here, because it is not the browser's to fix:
 * `dux_core::pty`'s reconnect replay re-asserts 1000/1002/1003/1005/1006 but
 * has no X10 (`?9`) flag to re-assert, since alacritty_terminal does not model
 * one. So a `?9` app's tracking is lost the moment a browser attaches, and no
 * report is produced at all (MEASURED in `tools/preview-env`). Nothing in the
 * table above uses `?9`.
 *
 * # Where the events go
 *
 * xterm binds its mouse-report handler to `Terminal.element` (the `.xterm`
 * div), and moves `mouseup` onto the DOCUMENT for the duration of a press
 * (`bindMouse` in `CoreBrowserTerminal`), so a release dispatched at the
 * element alone is never seen. The steps below say which target each event
 * needs. Note that the `.xterm-screen` child is what `termlink.ts` targets, one
 * level down, which is why a link replay cannot accidentally fire a mouse
 * report and this one cannot accidentally activate a link.
 *
 * No clamping happens here on purpose. xterm clamps the point into the canvas
 * and then REJECTS a cell outside the grid in `triggerMouseEvent`, so a tap in
 * the padding resolves to the edge cell exactly as a desktop click there does.
 */

/** Which node an event has to be dispatched at to reach xterm's handler. */
export type MouseReplayTarget = "element" | "document"

/** One DOM event in a replayed gesture. */
export interface MouseReplayStep {
  type: "mousedown" | "mouseup" | "wheel"
  target: MouseReplayTarget
  /** `MouseEvent.button`: 0 is the left button. */
  button: number
  /** `MouseEvent.buttons`: the bitmask of buttons still held. */
  buttons: number
  /** Wheel steps only. Negative reveals OLDER output, matching `scrollLines`. */
  deltaY?: number
}

/**
 * The events a single-finger TAP would have produced.
 *
 * Press then release, left button. xterm's own `mousedown` handler is what
 * arms the document-level `mouseup` listener, so the order matters and the
 * release must go to the document. Under the `?9` X10 protocol xterm never arms
 * that listener and the release lands on nothing, which is correct: X10 reports
 * presses only.
 */
export function tapReplaySteps(): MouseReplayStep[] {
  return [
    { type: "mousedown", target: "element", button: 0, buttons: 1 },
    { type: "mouseup", target: "document", button: 0, buttons: 0 },
  ]
}

/**
 * The events `notches` wheel clicks would have produced.
 *
 * Signed like `Terminal.scrollLines`: NEGATIVE reveals older output (wheel up),
 * POSITIVE reveals newer output (wheel down). One event per notch, because a
 * real wheel emits one event per detent and a mouse-tracking pager is built for
 * that cadence — see the touch-scroll tenet in CLAUDE.md and `dragWheelReport`,
 * which is what caps a flick to a single notch before it gets here.
 *
 * `deltaY` is ±1 rather than the notch count: xterm reads only the SIGN of
 * `deltaY` to pick wheel-up from wheel-down, and passes the event through
 * `consumeWheelEvent` purely as a zero test. A `deltaMode` of
 * `WheelEvent.DOM_DELTA_LINE` keeps it out of the pixel branch, which
 * accumulates a fractional remainder across events and would swallow some.
 */
export function wheelReplaySteps(notches: number): MouseReplayStep[] {
  const count = Math.abs(Math.trunc(notches))
  if (count === 0) return []
  const deltaY = notches < 0 ? -1 : 1
  return Array.from({ length: count }, () => ({
    type: "wheel" as const,
    target: "element" as const,
    button: 0,
    buttons: 0,
    deltaY,
  }))
}

/** The centre of a rect, as a client point. A page-scroll has no finger. */
export function rectCenter(rect: {
  left: number
  top: number
  width: number
  height: number
}): { clientX: number; clientY: number } {
  return {
    clientX: rect.left + rect.width / 2,
    clientY: rect.top + rect.height / 2,
  }
}

/**
 * Dispatches a planned replay at xterm's mouse pipeline.
 *
 * `element` is `Terminal.element`; `undefined`/`null` (an unopened terminal) is
 * a no-op. The events BUBBLE, unlike the link replay's: xterm's listener is on
 * this exact node, and a bubbling event is what the browser would really have
 * delivered.
 */
export function dispatchMouseReplay(
  element: HTMLElement | null | undefined,
  steps: readonly MouseReplayStep[],
  clientX: number,
  clientY: number,
): void {
  if (!element) return
  const doc = element.ownerDocument
  for (const step of steps) {
    const target: EventTarget = step.target === "document" ? doc : element
    const init = {
      bubbles: true,
      cancelable: true,
      clientX,
      clientY,
      button: step.button,
      buttons: step.buttons,
      detail: 1,
    }
    const event =
      step.type === "wheel"
        ? new WheelEvent("wheel", {
            ...init,
            deltaY: step.deltaY ?? 0,
            deltaMode: 1, // WheelEvent.DOM_DELTA_LINE
          })
        : new MouseEvent(step.type, init)
    target.dispatchEvent(event)
  }
}

/**
 * Encodes an xterm `onBinary` payload.
 *
 * `onBinary` carries a "binary string": each code unit is one BYTE, values up
 * to 255. The X10 mouse encoding puts `col + 32` in a byte, so a column past 95
 * exceeds ASCII, and `TextEncoder` would emit the TWO-byte UTF-8 form and
 * corrupt the report. This is the reason `onBinary` exists as a separate event
 * from `onData` at all, so it must not share `onData`'s encoder.
 */
export function latin1Bytes(data: string): Uint8Array {
  const out = new Uint8Array(data.length)
  for (let i = 0; i < data.length; i++) {
    out[i] = data.charCodeAt(i) & 0xff
  }
  return out
}
