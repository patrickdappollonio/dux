import { useEffect, useState } from "react"

import { cn } from "@/lib/utils"

const FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

// Animated braille spinner matching the dux TUI (80ms cadence, wall-clock).
export function BrailleSpinner({ className }: { className?: string }) {
  const [i, setI] = useState(0)
  useEffect(() => {
    const t = setInterval(() => setI((n) => (n + 1) % FRAMES.length), 80)
    return () => clearInterval(t)
  }, [])
  // Braille glyphs come from a monospace fallback font with taller metrics than
  // the UI sans, so a bare span sits higher than adjacent text. An inline-flex
  // box with leading-none centers the glyph on the row instead.
  return (
    <span
      aria-hidden
      className={cn(
        "inline-flex items-center justify-center leading-none",
        className
      )}
    >
      {FRAMES[i]}
    </span>
  )
}
