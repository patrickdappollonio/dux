import type { ReactNode } from "react"

import { cn } from "@/lib/utils"

// What a surface shows when it has nothing to show. A caller hands over the
// words, the icon and any buttons, and names how much room the empty state
// has; its padding, type, colours and what a screen reader hears are decided
// here. Where it sits in the caller's layout (a box shared with a spinner,
// centred in a detail pane) stays the caller's.
//
// - `page` fills the box it is in, for a pane or a dialog body with nothing
//   else in it.
// - `section` takes only its own height, for the place a list would be inside
//   a pane that has other things in it.
// - `compact` is one quiet sentence and no icon, for a list inside a dialog or
//   a menu, where an icon tile and a heading would outweigh the list itself.
type RoomyEmptyState = {
  size: "page" | "section"
  /** A lucide glyph; drawn in a tile and hidden from a screen reader. */
  icon: ReactNode
  title: ReactNode
  /** One paragraph, or several drawn one under another. */
  description?: ReactNode | readonly ReactNode[]
  /** Buttons, laid out in a row that wraps. */
  actions?: ReactNode
  /** A status region, for an empty state that appears while the user types. */
  announce?: boolean
}

type CompactEmptyState = {
  size: "compact"
  title: ReactNode
  announce?: boolean
}

export type EmptyStateProps = RoomyEmptyState | CompactEmptyState

export function EmptyState(props: EmptyStateProps) {
  const role = props.announce ? "status" : undefined
  if (props.size === "compact") {
    return (
      <p
        data-slot="empty"
        role={role}
        className="px-3 py-6 text-center text-sm text-balance text-muted-foreground"
      >
        {props.title}
      </p>
    )
  }
  const { size, icon, title, description, actions } = props
  const paragraphs: readonly ReactNode[] =
    description === undefined
      ? []
      : Array.isArray(description)
        ? description
        : [description]
  return (
    <div
      data-slot="empty"
      role={role}
      className={cn(
        "flex w-full min-w-0 flex-col items-center justify-center gap-4 p-6 text-center text-balance",
        size === "page" && "h-full min-h-0 flex-1",
      )}
    >
      <div className="flex max-w-sm flex-col items-center gap-2">
        <div
          aria-hidden
          className="mb-2 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
        >
          {icon}
        </div>
        <div className="font-heading text-sm font-medium tracking-tight">{title}</div>
        {paragraphs.map((paragraph, index) => (
          <div key={index} className="text-sm/relaxed text-muted-foreground">
            {paragraph}
          </div>
        ))}
      </div>
      {actions ? (
        <div className="flex w-full max-w-sm min-w-0 flex-row flex-wrap items-center justify-center gap-4 text-sm">
          {actions}
        </div>
      ) : null}
    </div>
  )
}
