import { useId } from "react"

import type { Prose } from "@/lib/prose"

/** A reason for one or more menu items, made once per sentence a menu can show. */
export interface MenuReasonHandle {
  /** The sentence, or null when nothing it covers is disabled for this reason. */
  readonly text: string | Prose | null
  readonly id: string
  /** What every item it greys passes as `aria-describedby`. */
  readonly describedBy: string | undefined
}

/** The reason `MenuReasonsHead` or `MenuItemReason` says (components/MenuReason.tsx). */
export function useMenuReason(text: string | Prose | null): MenuReasonHandle {
  const id = useId()
  return { text, id, describedBy: text === null ? undefined : id }
}
