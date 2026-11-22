import * as React from "react"

import { readFlapHidden, subscribeFlapHidden } from "@/lib/flapHidden"

/** The device's flap show/hide choice, live across every mounted screen. */
export function useFlapHidden(): boolean {
  return React.useSyncExternalStore(subscribeFlapHidden, readFlapHidden, () => false)
}
