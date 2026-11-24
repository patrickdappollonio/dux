import * as React from "react"

import { useScrollEdgesOf } from "@/hooks/use-scroll-edges"
import { cn } from "@/lib/utils"

/**
 * A list's scrolling container: the box that scrolls, the shadow at the start
 * and at the end that says more of the list is hidden past that edge, and
 * nothing about what a row looks like. Any child is a row; `ScrollListRows`
 * holds a run of them with the list's spacing, its semantics and, for a
 * listbox, its keyboard movement.
 *
 * The shadow is the `[data-scroll-list-fade]` mask in index.css: the edge with
 * content past it dissolves rather than being cut flat, and the same depth is
 * the box's scroll padding, so a row scrolled into view lands clear of it.
 *
 * `scroll="own"` renders the box. `scroll="enclosing"` is for a list that
 * scrolls inside a box something else owns, a modal's body or a scroll area:
 * the list renders a plain block and puts the shadow on that box for as long
 * as it is mounted, so one modal body or one scroll area holds at most one
 * enclosing list. The box is the nearest scroll area viewport or modal body
 * above the list, and a modal body's own scroll area when it scrolls in one
 * (the same rule the modal uses to find its scroller). Outside both, the list
 * renders with no shadow.
 */
export type ScrollListProps = { children?: React.ReactNode } & (
  | {
      scroll: "own"
      /** `fill` grows into the room it is given; `peek` is capped at about
       * three and a half rows so the next one peeks; a number of pixels is a
       * height the caller measured, and undefined until it has. */
      height: "fill" | "peek" | { px: number | undefined }
      /** `gutter` keeps a stable scrollbar gutter pulled into the space beside
       * the list, so the rows still line up with what is above them. */
      scrollbar: "hidden" | "native" | "gutter"
      /** `pane` insets the rows from a pane's edges, with room above the first
       * one for a focus ring. */
      inset?: "pane"
      scrollerRef?: (node: HTMLDivElement | null) => void
    }
  | { scroll: "enclosing" }
)

const FADE = "data-scroll-list-fade"
const MORE_ABOVE = "data-more-above"
const MORE_BELOW = "data-more-below"

const HEIGHT = {
  fill: "min-h-0 flex-1",
  // A shortcut row is 2.25rem with a fine pointer and 2.75rem (its 44px floor)
  // on a phone or under a finger.
  peek: "flex flex-col max-h-[8.75rem] max-md:max-h-[9.625rem] pointer-coarse:max-h-[9.625rem]",
  px: "flex min-h-0 flex-col",
} as const

const SCROLLBAR = {
  hidden: "no-scrollbar",
  native: "",
  gutter: "-mr-2 pr-2 [scrollbar-gutter:stable]",
} as const

export function ScrollList(props: ScrollListProps) {
  return props.scroll === "own" ? <OwnBox {...props} /> : <EnclosedList>{props.children}</EnclosedList>
}

function OwnBox({
  height,
  scrollbar,
  inset,
  scrollerRef,
  children,
}: Extract<ScrollListProps, { scroll: "own" }>) {
  const [node, setNode] = React.useState<HTMLDivElement | null>(null)
  const { moreAbove, moreBelow } = useScrollEdgesOf(node)
  const ref = React.useCallback(
    (next: HTMLDivElement | null) => {
      setNode(next)
      scrollerRef?.(next)
    },
    [scrollerRef],
  )
  const measured = typeof height === "object"
  return (
    <div
      ref={ref}
      {...{ [FADE]: "" }}
      data-more-above={moreAbove ? "" : undefined}
      data-more-below={moreBelow ? "" : undefined}
      style={measured ? { height: height.px } : undefined}
      className={cn(
        "overflow-y-auto",
        HEIGHT[measured ? "px" : height],
        SCROLLBAR[scrollbar],
        inset === "pane" && "px-2 pt-[3px] pb-2",
      )}
    >
      {children}
    </div>
  )
}

// The box an enclosed list scrolls in, found the way the modal finds its own.
function enclosingScroller(list: HTMLElement): HTMLElement | null {
  const near = list.parentElement?.closest<HTMLElement>(
    '[data-slot="scroll-area-viewport"], [data-slot="modal-body"]',
  )
  if (!near) return null
  if (near.dataset.slot === "modal-body") {
    return near.closest<HTMLElement>('[data-slot="scroll-area-viewport"]') ?? near
  }
  return near
}

function EnclosedList({ children }: { children?: React.ReactNode }) {
  const [list, setList] = React.useState<HTMLDivElement | null>(null)
  const scroller = React.useMemo(() => (list === null ? null : enclosingScroller(list)), [list])
  const { moreAbove, moreBelow } = useScrollEdgesOf(scroller)

  // Attributes and never a class: the box's owner re-renders its own class
  // names (a modal's body changes them between card and cover) and would drop
  // one added here, while attributes it never set are left alone.
  React.useLayoutEffect(() => {
    if (scroller === null) return
    scroller.setAttribute(FADE, "")
    return () => {
      scroller.removeAttribute(FADE)
      scroller.removeAttribute(MORE_ABOVE)
      scroller.removeAttribute(MORE_BELOW)
    }
  }, [scroller])
  React.useLayoutEffect(() => {
    if (scroller === null) return
    scroller.toggleAttribute(MORE_ABOVE, moreAbove)
    scroller.toggleAttribute(MORE_BELOW, moreBelow)
  }, [scroller, moreAbove, moreBelow])

  return <div ref={setList}>{children}</div>
}

/** Where a key moves a listbox's current row, or null for a key that does not
 * move it. `at` is -1 while no listed row is current, and the move then starts
 * from the first row. */
export function scrollListMoveTarget(key: string, at: number, count: number): number | null {
  switch (key) {
    case "ArrowDown":
      return Math.min(at + 1, count - 1)
    case "ArrowUp":
      return Math.max(at - 1, 0)
    case "Home":
      return 0
    case "End":
      return count - 1
    default:
      return null
  }
}

/**
 * A run of rows: their spacing, what a screen reader is told they are, and for
 * a listbox the arrow keys, Home and End. The caller keeps which row is current
 * (`at`) and decides what a move does, selecting and focusing the row it is
 * handed; the rows are children, drawn as the caller draws them.
 */
export type ScrollListRowsProps = {
  /** `none`: rows meet; `tight` and `normal`: 2px and 4px between rows. */
  gap: "none" | "tight" | "normal"
  children?: React.ReactNode
} & (
  | { semantics: "none" | "list" }
  | {
      semantics: "listbox"
      label: string
      movement: { count: number; at: number; onMove: (index: number) => void }
    }
)

const GAP = { none: "", tight: "gap-0.5", normal: "gap-1" } as const

export function ScrollListRows(props: ScrollListRowsProps) {
  const className = cn("flex flex-col", GAP[props.gap])
  if (props.semantics !== "listbox") {
    return props.semantics === "none" ? (
      <div className={className}>{props.children}</div>
    ) : (
      <ul className={className}>{props.children}</ul>
    )
  }
  const { label, movement } = props
  return (
    <ul
      role="listbox"
      aria-label={label}
      className={className}
      onKeyDown={(event) => {
        const target = scrollListMoveTarget(event.key, movement.at, movement.count)
        if (target === null || movement.count === 0) return
        event.preventDefault()
        movement.onMove(target)
      }}
    >
      {props.children}
    </ul>
  )
}
