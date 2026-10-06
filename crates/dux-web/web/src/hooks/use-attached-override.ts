import { useEffect, useRef, useState } from "react"

import type { AttachedBlocker } from "@/lib/attached"

/** A guarded confirmation's state, reset per opening and target: a refusal keeps
 * the dialog open, names who is attached, and turns its confirm into the override.
 * @returns `confirm(send)`, resolving true when the dialog may close; `send` gets
 * the on-screen blockers' keys (null on a plain confirm) and resolves new blockers or null. */
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
