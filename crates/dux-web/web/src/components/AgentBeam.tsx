import { useEffect, useState } from "react"

// A soft light that sweeps LEFT → RIGHT across a working agent's row to signal
// activity, alongside the bouncing Bot icon. The geometry/keyframes live in
// `.agent-beam` / `.agent-beam-light` in index.css.
//
// When the agent STOPS working we don't yank the beam mid-sweep (that froze the
// light part-way across the name). Instead we keep it mounted and let the current
// left→right pass finish: on the next animation-iteration boundary (the end of a
// sweep) with work done, we unmount. A timer is the fallback for when the sweep
// is disabled (prefers-reduced-motion), so the beam can't linger forever.
// Mirrors how the Bot icon eases back to rest rather than snapping. Rendered
// unconditionally by the row; returns null when idle.
export function AgentBeam({ working }: { working: boolean }) {
  const [show, setShow] = useState(working)

  // Begin the moment work starts — the React-sanctioned "adjust state during
  // render" pattern (like CodeEditor's everReady), so there's no setState inside
  // an effect. The stop side is handled by the iteration handler + fallback timer.
  if (working && !show) setShow(true)

  useEffect(() => {
    if (working) return
    // Work stopped: fallback-unmount one sweep later in case the animation is
    // off (reduced motion) so `onAnimationIteration` never fires. The setState
    // is inside the timer (async), not synchronous in the effect body.
    const timer = setTimeout(() => setShow(false), 1700)
    return () => clearTimeout(timer)
  }, [working])

  if (!show) return null
  return (
    <div className="agent-beam" aria-hidden>
      <span
        className="agent-beam-light"
        onAnimationIteration={() => {
          // End of a full sweep — if work is done, stop here for a clean finish.
          if (!working) setShow(false)
        }}
      />
    </div>
  )
}
