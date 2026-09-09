/**
 * Replays the DOM mouse events a touch gesture would have produced, letting xterm
 * do the cell math, protocol gating and encoding: xterm publishes the mouse
 * tracking protocol but no read of the active encoding, so encoding here cannot
 * be correct. Reports under the X10 encoding arrive on `onBinary`, not `onData`.
 */

import { markDuxReplay } from "./termreplay"

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
 * The events a single-finger tap would have produced, in order: xterm's own
 * `mousedown` arms the document-level `mouseup`, so the release goes to the document.
 */
export function tapReplaySteps(): MouseReplayStep[] {
  return [
    { type: "mousedown", target: "element", button: 0, buttons: 1 },
    { type: "mouseup", target: "document", button: 0, buttons: 0 },
  ]
}

/**
 * The events `notches` wheel clicks would have produced, one per notch and signed
 * like `Terminal.scrollLines`: negative reveals older output.
 *
 * `deltaY` is ±1 because xterm reads only its sign; the line `deltaMode` keeps the
 * event out of xterm's pixel branch, which accumulates a fractional remainder.
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
 * Dispatches a planned replay at `Terminal.element`; a null element (an unopened
 * terminal) is a no-op. The events bubble and carry the dux-replay tag, so the
 * container's capture-phase link intercept judges only what a human did.
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
    target.dispatchEvent(markDuxReplay(event))
  }
}

/**
 * Encodes an xterm `onBinary` payload, whose code units are single bytes: the X10
 * mouse encoding puts `col + 32` in a byte and `TextEncoder` would emit two.
 */
export function latin1Bytes(data: string): Uint8Array {
  const out = new Uint8Array(data.length)
  for (let i = 0; i < data.length; i++) {
    out[i] = data.charCodeAt(i) & 0xff
  }
  return out
}
