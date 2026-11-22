import { useState } from "react"

/** Changes each time `open` turns true: the `key` for the component that
 * holds a modal's state, so every open starts fresh and a close still animates. */
export function useOpenEpoch(open: boolean): number {
  const [epoch, setEpoch] = useState(0)
  const [was, setWas] = useState(open)
  if (open !== was) {
    setWas(open)
    if (open) setEpoch(epoch + 1)
  }
  return epoch
}
