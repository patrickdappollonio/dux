import { useEffect, useRef, useState } from "react"

import type { AttachedBlocker } from "@/lib/attached"

// A guarded confirmation's state: a delete, detach, stop, tab or terminal
// close, or project removal that the server refused because somebody else is
// attached stays open, names them, and turns its confirm into the override.
//
// `confirm(send)` makes the request and resolves whether the change went ahead
// and the dialog may close. `send(accepted)` resolves the blockers of a
// refusal, or `null`; `accepted` is null on a plain confirm and, on the
// override, the keys of exactly the blockers on screen, so the server refuses
// again rather than cut off anybody the person has not seen.
//
// Everything is keyed to one opening of the dialog on one target: closing it,
// or pointing it at another target, starts afresh, and an answer to a request
// sent from an earlier opening is dropped whole, its pending flag included.
export function useAttachedOverride(open: boolean, target: string | null) {
  const [blockers, setBlockers] = useState<AttachedBlocker[] | null>(null)
  const [pending, setPending] = useState(false)
  const [opening, setOpening] = useState({ open, target, generation: 0 })
  const generation = useRef(0)
  const cancelRef = useRef<HTMLButtonElement>(null)

  if (opening.open !== open || opening.target !== target) {
    setOpening({ open, target, generation: opening.generation + 1 })
    setBlockers(null)
    setPending(false)
  }

  useEffect(() => {
    generation.current = opening.generation
  }, [opening.generation])

  // Focus goes back to Cancel once the dialog names somebody, so the keystroke
  // that confirmed cannot also go ahead over them.
  useEffect(() => {
    if (blockers !== null) cancelRef.current?.focus()
  }, [blockers])

  async function confirm(
    send: (accepted: readonly string[] | null) => Promise<AttachedBlocker[] | null>,
  ): Promise<boolean> {
    const sentIn = generation.current
    setPending(true)
    const refused = await send(
      blockers === null ? null : blockers.map((blocker) => blocker.key),
    )
    if (generation.current !== sentIn) return false
    setPending(false)
    if (refused == null) return true
    setBlockers(refused)
    return false
  }

  return { blockers, pending, cancelRef, confirm }
}
