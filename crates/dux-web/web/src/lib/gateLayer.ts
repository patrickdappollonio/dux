// The sign-in gate's layer, as the hidden app's own listeners see it.
//
// While the gate is up the app stays mounted underneath it, and a few of dux's
// listeners sit on the document in the CAPTURE phase (the divider drags and
// the Changes pane's gesture tracking, the terminal's composition tracking,
// the link press's release watcher). Those see an event before the gate does,
// so each one asks whether the event is aimed at the gate and leaves it alone
// if so (`outsideGate`, or `isInGateLayer` directly). Nothing is stopped on the
// way: the gate's own controls hear every event, presses and hovers included.
// (react-resizable-panels needs nothing here: it already ignores a press on
// content stacked above its separator.)
//
// A gesture that began in the app before the gate went up can have its release
// land on the gate, where those listeners now ignore it; `onGateUp` is how
// each one resets what such a release would have ended.

import { authPaused, subscribeAuth } from "./authGate"

export const GATE_LAYER_ATTR = "data-auth-gate-layer"

export function isInGateLayer(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(`[${GATE_LAYER_ATTR}]`) !== null
}

/// Wrap a listener of the hidden app's so it ignores events aimed at the gate.
export function outsideGate<E extends Event>(listener: (event: E) => void): (event: E) => void {
  return (event: E) => {
    if (isInGateLayer(event.target)) return
    listener(event)
  }
}

/// Run `reset` each time the page goes to one of the gate's pages. Returns the
/// way to stop.
export function onGateUp(reset: () => void): () => void {
  let up = authPaused()
  return subscribeAuth(() => {
    const now = authPaused()
    if (now && !up) reset()
    up = now
  })
}
