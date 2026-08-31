import { useEffect, useState } from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  GLYPH_SPINNER_CLASS,
  SPINNER_FRAMES,
  SPINNER_FRAME_MS,
} from "@/lib/spinnerFrames"
import { cn } from "@/lib/utils"

// The one text spinner in the web UI, drawing dux's six-frame arc in the
// glyphs and cadence of `spinnerFrames.ts`. Every site that wants a glyph
// spinner renders THIS, so the fixed-width slot (`.glyph-spinner` in index.css)
// and the reduced-motion behavior below cannot drift between them; a site that
// wants an icon spinner uses lucide's `Loader2` with `motion-safe:animate-spin`
// instead.
export function GlyphSpinner({ className }: { className?: string }) {
  const reduceMotion = usePrefersReducedMotion()
  const [i, setI] = useState(0)
  useEffect(() => {
    // Reduced motion holds the first frame. The glyph stays on screen (like
    // the attention dot, which keeps its dot and drops only the pulse) so the
    // "something is happening" cue survives; only the cycling stops.
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
