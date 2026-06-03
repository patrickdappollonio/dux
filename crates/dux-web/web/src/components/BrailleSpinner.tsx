import { useEffect, useState } from "react"

const FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

// Animated braille spinner matching the dux TUI (80ms cadence, wall-clock).
export function BrailleSpinner({ className }: { className?: string }) {
  const [i, setI] = useState(0)
  useEffect(() => {
    const t = setInterval(() => setI((n) => (n + 1) % FRAMES.length), 80)
    return () => clearInterval(t)
  }, [])
  return <span className={className}>{FRAMES[i]}</span>
}
