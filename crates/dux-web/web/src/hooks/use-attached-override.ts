import { useEffect, useRef, useState } from "react"

import type { AttachedBlocker } from "@/lib/attached"

// A guarded confirmation's state: a delete, detach, stop, tab or terminal
// close, or project removal that the server refused because somebody else is
// attached stays open, names them, and turns its confirm into the override.
//
// `confirm(send)` makes the request (`send(force)` resolves the blockers of a
// refusal, or `null`) and resolves whether the change went ahead and the
// dialog may close; the override is the same call once somebody is named.
export function useAttachedOverride(open: boolean) {
  const [blockers, setBlockers] = useState<AttachedBlocker[] | null>(null)
  const [pending, setPending] = useState(false)
  const [wasOpen, setWasOpen] = useState(open)
  const cancelRef = useRef<HTMLButtonElement>(null)

  // A closed dialog forgets who it named, so the next open asks afresh.
  if (open !== wasOpen) {
    setWasOpen(open)
    if (!open) {
      setBlockers(null)
      setPending(false)
    }
  }

  // Focus goes back to Cancel once the dialog names somebody, so the keystroke
  // that confirmed cannot also go ahead over them.
  useEffect(() => {
    if (blockers !== null) cancelRef.current?.focus()
  }, [blockers])

  async function confirm(
    send: (force: boolean) => Promise<AttachedBlocker[] | null>,
  ): Promise<boolean> {
    setPending(true)
    const refused = await send(blockers !== null)
    setPending(false)
    if (refused == null) return true
    setBlockers(refused)
    return false
  }

  return { blockers, pending, cancelRef, confirm }
}
