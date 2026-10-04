// The sign-in gate's layer, seen from the DOM: how to recognise it, and the
// capture shield that keeps what happens on it away from the hidden app.
//
// While the gate is up the app stays mounted underneath it, and some of its
// listeners sit on the document in the CAPTURE phase (the divider drags, which
// hit-test by rectangle with no notion of what is painted over them; the
// terminal's composition tracking; the panel library). Those run before the
// event reaches the gate, so a stop on the gate itself comes too late for them.
// The shield is a capture listener on the WINDOW, the first stop of every
// event's journey, registered when this module loads (before any component
// mounts and adds its own), so it runs ahead of all of them.
//
// It stops only events the gate's own controls do not need: presses, moves,
// wheel, touch, IME composition, and file drags (whose default it also
// prevents, so a file dropped on the login page does not navigate the tab away
// and take the page's unsaved state with it). Focus, typing and clicks still
// flow to the gate's fields and buttons, and a stop at the layer keeps their
// bubbling away from the app's document and window listeners (`AuthGate.tsx`).

export const GATE_LAYER_ATTR = "data-auth-gate-layer"

export function isInGateLayer(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(`[${GATE_LAYER_ATTR}]`) !== null
}

const SHIELDED_EVENTS = [
  "pointerdown",
  "pointermove",
  "pointerup",
  "pointercancel",
  "pointerover",
  "pointerout",
  "pointerenter",
  "pointerleave",
  "mousedown",
  "mousemove",
  "mouseup",
  "dblclick",
  "contextmenu",
  "touchstart",
  "touchmove",
  "touchend",
  "touchcancel",
  "wheel",
  "compositionstart",
  "compositionupdate",
  "compositionend",
  "dragenter",
  "dragover",
  "drop",
] as const

let gateUp = false

/// Called by the gate layer as it shows and goes.
export function setGateUp(up: boolean): void {
  gateUp = up
}

function shield(event: Event): void {
  if (!gateUp || !isInGateLayer(event.target)) return
  if (event.type === "dragover" || event.type === "drop" || event.type === "dragenter") {
    event.preventDefault()
  }
  event.stopPropagation()
}

if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  for (const type of SHIELDED_EVENTS) window.addEventListener(type, shield, true)
}
