import { useEffect, useState } from "react"

import { cn } from "@/lib/utils"

const FRAMES = ["◜", "◠", "◝", "◞", "◡", "◟"]

// Animated arc spinner matching the dux TUI's six-frame, 100ms cadence.
export function GlyphSpinner({ className }: { className?: string }) {
  const [i, setI] = useState(0)
  useEffect(() => {
    const t = setInterval(() => setI((n) => (n + 1) % FRAMES.length), 100)
    return () => clearInterval(t)
  }, [])
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
