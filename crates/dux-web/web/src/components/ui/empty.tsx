import { Fragment, type ReactNode } from "react"

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
// - `compact` is one quiet sentence and no icon, for a list that sits in a
//   dialog, a menu, a popover or the sidebar, where an icon tile and a heading
//   would outweigh the list itself.
//
// A `page` one never puts content where it cannot be reached: it centres what
// fits, and what does not fit starts at the top and scrolls.
type RoomyEmptyState = {
  size: "page" | "section"
  /** A lucide glyph; drawn in a tile and hidden from a screen reader. */
  icon: ReactNode
  title: ReactNode
  /** One paragraph, or several drawn one under another. */
  description?: ReactNode | readonly ReactNode[]
} & (
  | {
      /** Buttons, laid out in a row that wraps. */
      actions?: ReactNode
      announce?: false
    }
  | {
      /** A status region, for an empty state that appears while the user
       * types. Not with buttons: a status region would announce them. */
      announce: true
      actions?: never
    }
)

type CompactEmptyState = {
  size: "compact"
  title: ReactNode
  /** A status region, for an empty state that appears while the user types. */
  announce?: boolean
}

export type EmptyStateProps = RoomyEmptyState | CompactEmptyState

// Where a list's one line sits in place of its rows. The compact empty state,
// the loading line (`ListLine`) and the failed load (`FailedLoad`) all use it,
// so a list that goes from one to another does not move or change size.
const LIST_LINE = "px-3 py-6 text-center text-sm text-balance"

/**
 * A list's loading line, or any other line that stands in place of its rows,
 * centred where the compact empty state sits. `muted` is for words; a spinner
 * names its own tone.
 */
export function ListLine({ children, muted = false }: { children: ReactNode; muted?: boolean }) {
  return (
    <div
      data-slot="list-line"
      className={cn("flex flex-col items-center gap-2", LIST_LINE, muted && "text-muted-foreground")}
    >
      {children}
    </div>
  )
}

export function EmptyState(props: EmptyStateProps) {
  const role = props.announce ? "status" : undefined
  if (props.size === "compact") {
    return (
      <p
        data-slot="empty"
        role={role}
        className={cn(LIST_LINE, "text-muted-foreground")}
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
        // No `justify-center`: it would push content that does not fit out
        // above the top of a scroller, where nothing can reach it. The auto
        // margins of the block inside centre it only while there is room.
        "flex w-full min-w-0 flex-col items-center p-6 text-center text-balance",
        size === "page" && "h-full min-h-0 flex-1 overflow-y-auto",
      )}
    >
      <div className="my-auto flex w-full flex-col items-center gap-4">
        <div
          data-slot="empty-header"
          className="flex max-w-sm flex-col items-center gap-2"
        >
          <div
            aria-hidden
            className="mb-2 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
          >
            {icon}
          </div>
          <div className="font-heading text-sm font-medium tracking-tight">{title}</div>
          {paragraphs.map((paragraph, index) => (
            // The space is collapsed on screen between blocks; it keeps the
            // sentences apart in the text a script or a reader copies out.
            <Fragment key={index}>
              {" "}
              <div className="text-sm/relaxed text-muted-foreground [&_a]:underline [&_a]:underline-offset-4 [&_a:hover]:text-primary">
                {paragraph}
              </div>
            </Fragment>
          ))}
        </div>
        {actions ? (
          <div className="flex w-full max-w-sm min-w-0 flex-row flex-wrap items-center justify-center gap-4 text-sm">
            {actions}
          </div>
        ) : null}
      </div>
    </div>
  )
}
