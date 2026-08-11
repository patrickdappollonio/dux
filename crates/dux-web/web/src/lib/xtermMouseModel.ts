/**
 * A TEST FIXTURE, not shipped code. Nothing under `components/` or `lib/`
 * imports it, so it never reaches a bundle; it exists because jsdom cannot open
 * a real xterm (no canvas) and the thing under test is precisely what xterm
 * does with a mouse event.
 *
 * It is a line-by-line transcription of the mouse pipeline in the INSTALLED
 * `@xterm/xterm` 6.0.0 (`node_modules/@xterm/xterm/lib/xterm.mjs`): the
 * `bindMouse` dispatcher on `CoreBrowserTerminal`, `MouseService`'s
 * `getCoordsRelativeToElement` / `getMouseReportCoords`, and
 * `CoreMouseService`'s protocol restrictions and encoding table. The
 * transcription is checked against the real thing by measuring a running
 * terminal in `tools/preview-env`; these tests pin dux's HALF of the contract
 * (that the right DOM events reach the right node, and that a report coming
 * back out is forwarded on the right channel with the right byte encoding).
 *
 * Deliberately faithful to the parts that bite:
 *  - `getMouseReportCoords` measures the SCREEN element, subtracts its CSS
 *    padding, clamps into the canvas, and divides by the MEASURED cell size,
 *    yielding a ZERO-based cell. `triggerMouseEvent` then rejects an
 *    out-of-grid cell and only afterwards makes the cell one-based.
 *  - `X10` reports presses only and strips modifiers; `VT200` drops motion;
 *    the wheel is bound only for a protocol whose event mask has bit 16.
 *  - the `DEFAULT` encoding goes out on `onBinary`, everything else on
 *    `onData`. That split is the reason a pane subscribing only to `onData`
 *    dropped X10 reports entirely.
 */

export type MouseProtocol = "NONE" | "X10" | "VT200" | "DRAG" | "ANY"
export type MouseEncoding = "DEFAULT" | "SGR" | "SGR_PIXELS"

interface MouseEventData {
  col: number
  row: number
  x: number
  y: number
  button: number
  action: number
  ctrl: boolean
  alt: boolean
  shift: boolean
}

/** `_protocols` in CoreMouseService. `events` is a bitmask: 1 down, 2 up, 4 drag, 8 move, 16 wheel. */
const PROTOCOLS: Record<
  MouseProtocol,
  { events: number; restrict: (e: MouseEventData) => boolean }
> = {
  NONE: { events: 0, restrict: () => false },
  X10: {
    events: 1,
    restrict: (e) => {
      if (e.button === 4 || e.action !== 1) return false
      e.ctrl = false
      e.alt = false
      e.shift = false
      return true
    },
  },
  VT200: { events: 19, restrict: (e) => e.action !== 32 },
  DRAG: { events: 23, restrict: (e) => !(e.action === 32 && e.button === 3) },
  ANY: { events: 31, restrict: () => true },
}

/** The `Ms` helper: the button/modifier byte. `isSGR` keeps the button on a release. */
function buttonCode(e: MouseEventData, isSGR: boolean): number {
  let code = (e.ctrl ? 16 : 0) | (e.shift ? 4 : 0) | (e.alt ? 8 : 0)
  if (e.button === 4) {
    code |= 64
    code |= e.action
  } else {
    code |= e.button & 3
    if (e.button & 4) code |= 64
    if (e.button & 8) code |= 128
    if (e.action === 32) code |= 32
    else if (e.action === 0 && !isSGR) code |= 3
  }
  return code
}

const chr = String.fromCharCode

/** `_encodings` in CoreMouseService. xterm implements exactly these three. */
const ENCODINGS: Record<MouseEncoding, (e: MouseEventData) => string> = {
  DEFAULT: (e) => {
    const p = [buttonCode(e, false) + 32, e.col + 32, e.row + 32]
    if (p[0] > 255 || p[1] > 255 || p[2] > 255) return ""
    return `\x1b[M${chr(p[0])}${chr(p[1])}${chr(p[2])}`
  },
  SGR: (e) => {
    const final = e.action === 0 && e.button !== 4 ? "m" : "M"
    return `\x1b[<${buttonCode(e, true)};${e.col};${e.row}${final}`
  },
  SGR_PIXELS: (e) => {
    const final = e.action === 0 && e.button !== 4 ? "m" : "M"
    return `\x1b[<${buttonCode(e, true)};${e.x};${e.y}${final}`
  },
}

export interface XtermMouseModelOptions {
  /** The `.xterm` node xterm binds its mouse handler to (`Terminal.element`). */
  element: HTMLElement
  /** The `.xterm-screen` node coordinates are measured against. */
  screen: HTMLElement
  cols: number
  rows: number
  cellWidth: number
  cellHeight: number
  /** CSS padding on the screen element, subtracted before the divide. */
  paddingLeft?: number
  paddingTop?: number
  /** Emits an SGR-family report. */
  onData: (data: string) => void
  /** Emits a DEFAULT (X10) report. Its bytes are NOT the same channel. */
  onBinary: (data: string) => void
  /** Called on `mousedown`, mirroring xterm's own focus grab. */
  onFocus?: () => void
}

/**
 * Installs the model on `options.element` and returns a handle whose
 * `protocol`/`encoding` can be moved the way a DECSET from the app moves them.
 */
export function installXtermMouseModel(options: XtermMouseModelOptions) {
  const state = {
    protocol: "NONE" as MouseProtocol,
    encoding: "DEFAULT" as MouseEncoding,
  }
  const doc = options.element.ownerDocument

  const reportCoords = (ev: MouseEvent) => {
    const rect = options.screen.getBoundingClientRect()
    let x = ev.clientX - rect.left - (options.paddingLeft ?? 0)
    let y = ev.clientY - rect.top - (options.paddingTop ?? 0)
    const canvasW = options.cols * options.cellWidth
    const canvasH = options.rows * options.cellHeight
    x = Math.min(Math.max(x, 0), canvasW - 1)
    y = Math.min(Math.max(y, 0), canvasH - 1)
    return {
      col: Math.floor(x / options.cellWidth),
      row: Math.floor(y / options.cellHeight),
      x: Math.floor(x),
      y: Math.floor(y),
    }
  }

  const trigger = (e: MouseEventData) => {
    if (
      e.col < 0 ||
      e.col >= options.cols ||
      e.row < 0 ||
      e.row >= options.rows ||
      (e.button === 4 && e.action === 32) ||
      (e.button === 3 && e.action !== 32) ||
      (e.button !== 4 && (e.action === 2 || e.action === 3))
    ) {
      return
    }
    e.col++
    e.row++
    if (!PROTOCOLS[state.protocol].restrict(e)) return
    const out = ENCODINGS[state.encoding](e)
    if (!out) return
    if (state.encoding === "DEFAULT") options.onBinary(out)
    else options.onData(out)
  }

  const report = (ev: MouseEvent) => {
    const coords = reportCoords(ev)
    let action: number
    let button: number
    switch (ev.type) {
      case "mouseup":
        action = 0
        button = ev.button < 3 ? ev.button : 3
        break
      case "mousedown":
        action = 1
        button = ev.button < 3 ? ev.button : 3
        break
      case "wheel": {
        const delta = (ev as WheelEvent).deltaY
        if (delta === 0) return
        action = delta < 0 ? 0 : 1
        button = 4
        break
      }
      default:
        return
    }
    trigger({
      ...coords,
      button,
      action,
      ctrl: ev.ctrlKey,
      alt: ev.altKey,
      shift: ev.shiftKey,
    })
  }

  // xterm registers `mouseup` on the DOCUMENT from inside its `mousedown`
  // handler, and only for a protocol whose mask has the up bit. That is why a
  // release dispatched at the element alone is never seen, and why X10 (mask 1)
  // silently reports no release.
  const documentMouseUp = (ev: Event) => {
    report(ev as MouseEvent)
    doc.removeEventListener("mouseup", documentMouseUp)
  }
  options.element.addEventListener("mousedown", (ev) => {
    options.onFocus?.()
    if (PROTOCOLS[state.protocol].events === 0) return
    report(ev as MouseEvent)
    if (PROTOCOLS[state.protocol].events & 2) {
      doc.addEventListener("mouseup", documentMouseUp)
    }
  })
  options.element.addEventListener("wheel", (ev) => {
    if (!(PROTOCOLS[state.protocol].events & 16)) return
    report(ev as MouseEvent)
  })

  return {
    setProtocol(p: MouseProtocol) {
      // xterm's `activeProtocol` setter throws on an unknown name, and so does
      // this: there is no fallback state to be in.
      if (!PROTOCOLS[p]) throw new Error(`unknown protocol "${p}"`)
      state.protocol = p
    },
    setEncoding(e: MouseEncoding) {
      if (!ENCODINGS[e]) throw new Error(`unknown encoding "${e}"`)
      state.encoding = e
    },
  }
}
