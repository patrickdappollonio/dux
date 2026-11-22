import { useState } from "react"

/** For the component that holds a modal's state and reads the store for it:
 * `mounted` from an open until its close has finished animating, with a new
 * `epoch` to key it by on every open, and `onClosed` to hand to the `Modal`. */
export function useModalLifetime(open: boolean): {
  mounted: boolean
  epoch: number
  onClosed: () => void
} {
  const [epoch, setEpoch] = useState(0)
  const [was, setWas] = useState(open)
  const [mounted, setMounted] = useState(open)
  if (open !== was) {
    setWas(open)
    if (open) {
      setEpoch(epoch + 1)
      setMounted(true)
    }
  }
  return {
    mounted,
    epoch,
    // A close that a new open overtook ends nothing.
    onClosed: () => setMounted((now) => now && open),
  }
}
