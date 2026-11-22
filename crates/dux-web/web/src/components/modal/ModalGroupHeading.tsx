import type { ReactNode } from "react"

import { cn } from "@/lib/utils"

/** The caption over a group of rows in a modal's body. It is the heading the
 * app's menus and lists already use (a menu's group label, a command list's
 * group heading, the sidebar's group label): small, medium weight, muted and
 * in the case it was written in. */
export function ModalGroupHeading({
  children,
  id,
  caption,
  inset = false,
  hidden = false,
}: {
  children: ReactNode
  /** For a group that is `aria-labelledby` its heading. */
  id?: string
  /** A sentence after the heading, on its line while there is room. */
  caption?: ReactNode
  /** Lines it up with rows padded 12px from the edge rather than 8px. */
  inset?: boolean
  /** For a group that names itself with `aria-label`, so the name is not read twice. */
  hidden?: boolean
}) {
  return (
    <div
      aria-hidden={hidden ? true : undefined}
      className={cn(
        "flex flex-wrap items-baseline gap-x-2 gap-y-0.5 py-1.5 text-xs text-muted-foreground",
        inset ? "px-3" : "px-2",
      )}
    >
      <span id={id} className="font-medium">
        {children}
      </span>
      {caption ? <span>{caption}</span> : null}
    </div>
  )
}
