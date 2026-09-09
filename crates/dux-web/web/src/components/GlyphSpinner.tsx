import { useEffect, useState } from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  GLYPH_SPINNER_CLASS,
  SPINNER_FRAMES,
  SPINNER_FRAME_MS,
} from "@/lib/spinnerFrames"
import { cn } from "@/lib/utils"

// The one text spinner in the web UI, in the glyphs and cadence of
// `spinnerFrames.ts`, so the fixed-width slot and the reduced-motion behavior
// cannot drift. An icon spinner is lucide's `Loader2` instead.
export function GlyphSpinner({ className }: { className?: string }) {
  const reduceMotion = usePrefersReducedMotion()
  const [i, setI] = useState(0)
  useEffect(() => {
    // Reduced motion holds the first frame rather than removing the glyph, so the
    // "something is happening" cue survives and only the cycling stops.
    if (reduceMotion) return
    const t = setInterval(
      () => setI((n) => (n + 1) % SPINNER_FRAMES.length),
      SPINNER_FRAME_MS,
    )
    return () => clearInterval(t)
  }, [reduceMotion])
  return (
    <span aria-hidden className={cn(GLYPH_SPINNER_CLASS, className)}>
      {SPINNER_FRAMES[reduceMotion ? 0 : i]}
    </span>
  )
}
