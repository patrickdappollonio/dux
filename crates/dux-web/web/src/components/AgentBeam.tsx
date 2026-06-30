import { useEffect, useState } from "react"

// A white, slightly tilted light bar that sweeps left→right across a working
// agent's row to signal activity, alongside the bouncing Bot icon. The sweep is
// a pure-CSS keyframe animation (`.agent-beam` / `.agent-beam-bar` in index.css,
// keyframes `agent-beam-sweep`) — no external dependency. It's laid over the row
// as a click-through overlay (`pointer-events: none`) clipped to the row's
// rounded corners. The overlay is mounted on the full-width row wrapper (not the
// inner label button), so the beam travels across the trailing ⋯ menu too.
//
// When the agent STOPS working we don't yank the beam mid-pass: we keep it
// mounted until the current sweep finishes (the next `animationiteration` after
// `working` goes false), then unmount for a clean exit. A fallback timer
// unmounts shortly after regardless, in case that event never fires (the
// animation is disabled under reduced motion, the row unmounted, etc.).
// Rendered unconditionally by the row; returns null when idle.
export function AgentBeam({ working }: { working: boolean }) {
  const [show, setShow] = useState(working)

  // Begin the moment work starts — the React-sanctioned "adjust state during
  // render" pattern, so there's no setState inside an effect.
  if (working && !show) setShow(true)

  useEffect(() => {
    if (working) return
    // Work stopped: fallback-unmount shortly after, in case the
    // animationiteration handler never fires.
    const timer = setTimeout(() => setShow(false), 1600)
    return () => clearTimeout(timer)
  }, [working])

  if (!show) return null
  return (
    <div className="agent-beam" aria-hidden>
      {/* The travelling bar. We let the current sweep complete before
          unmounting so the light exits cleanly rather than blinking out. */}
      <span
        className="agent-beam-bar"
        onAnimationIteration={() => {
          if (!working) setShow(false)
        }}
      />
    </div>
  )
}
