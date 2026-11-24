import { useCallback, useId, useState } from "react"

import { proseText, type Prose } from "@/lib/prose"

/** A reason for one or more menu items, made once per sentence a menu can show. */
export interface MenuReasonHandle {
  /** The sentence, or null when nothing it covers is disabled for this reason. */
  readonly text: string | Prose | null
  readonly id: string
  /**
   * What every item it greys passes as `aria-describedby`: the id of the line
   * that says it, and nothing until a line is on screen, so an item can never
   * point at a sentence that was not drawn.
   */
  readonly describedBy: string | undefined
  /** Said by the line that draws it, and returns the way to take that back. */
  readonly drawnAs: (lineId: string) => () => void
}

/** The reason `MenuReasonsHead`, `MenuGroupReason` or `MenuItemReason` says (components/MenuReason.tsx). */
export function useMenuReason(text: string | Prose | null): MenuReasonHandle {
  const id = useId()
  const [lineId, setLineId] = useState<string | null>(null)
  const drawnAs = useCallback((line: string) => {
    setLineId(line)
    return () => setLineId(null)
  }, [])
  return { text, id, describedBy: text === null ? undefined : (lineId ?? undefined), drawnAs }
}

/** The sentence as plain words, which is what makes two reasons the same one. */
export function reasonWords(text: string | Prose): string {
  return typeof text === "string" ? text : proseText(text)
}
