import { useEffect, useState } from "react"

// Re-render once a second while `active`, so an age printed from a document the
// server has no reason to send again keeps moving.
export function useSecondTicks(active: boolean): void {
  const [, setTicks] = useState(0)
  useEffect(() => {
    if (!active) return
    const timer = setInterval(() => setTicks((n) => n + 1), 1000)
    return () => clearInterval(timer)
  }, [active])
}
