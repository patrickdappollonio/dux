import { useEffect, useState } from "react"
import { BorderBeam } from "border-beam"

// A glowing light that travels along the bottom edge of a working agent's row to
// signal activity, alongside the bouncing Bot icon. The glow is drawn by the
// border-beam library's `line` preset (rotate family: a bottom-edge traveling
// beam); `colorVariant="mono"` + `theme="dark"` match the app's neutral,
// dark-only palette. `.agent-beam` (index.css) lays it over the row as a
// click-through overlay; BorderBeam fills that overlay (children is required, so
// we pass `null`, and set `borderRadius` explicitly to skip child auto-detect).
//
// When the agent STOPS working we don't yank the glow instantly: `active`
// follows `working`, so the library plays its fade-out and calls `onDeactivate`
// when it finishes — that's when we unmount, for a clean finish. A fallback
// timer unmounts shortly after regardless, in case `onDeactivate` never fires.
// Note: the rotate family (unlike the pulse types) ships no prefers-reduced-
// motion block, so the travel does not auto-disable under reduced motion.
// Rendered unconditionally by the row; returns null when idle.
export function AgentBeam({ working }: { working: boolean }) {
  const [show, setShow] = useState(working)

  // Begin the moment work starts — the React-sanctioned "adjust state during
  // render" pattern, so there's no setState inside an effect.
  if (working && !show) setShow(true)

  useEffect(() => {
    if (working) return
    // Work stopped: fallback-unmount shortly after, in case the fade-out (and so
    // onDeactivate) never fires.
    const timer = setTimeout(() => setShow(false), 1700)
    return () => clearTimeout(timer)
  }, [working])

  if (!show) return null
  return (
    <div className="agent-beam" aria-hidden>
      <BorderBeam
        size="line"
        colorVariant="mono"
        theme="dark"
        borderRadius={6}
        strength={1}
        active={working}
        onDeactivate={() => setShow(false)}
        style={{ width: "100%", height: "100%" }}
      >
        {null}
      </BorderBeam>
    </div>
  )
}
