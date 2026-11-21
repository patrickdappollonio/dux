import { useLayoutEffect } from "react"
import type { ReactNode, RefObject } from "react"

/// The setter a `ConfirmsOutlet` hands its dialog's body.
export type ConfirmsSetterRef = RefObject<((confirms: ReactNode) => void) | null>

/// Hand `confirms` to the outlet on every render, and clear it on unmount.
export function usePublishConfirms(outlet: ConfirmsSetterRef, confirms: ReactNode): void {
  useLayoutEffect(() => {
    outlet.current?.(confirms)
  })
  useLayoutEffect(() => () => outlet.current?.(null), [outlet])
}
