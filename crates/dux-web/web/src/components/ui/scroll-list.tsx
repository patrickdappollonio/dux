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
 * content past it dissolves rather than being cut flat. The mask spares what is
 * not a row (a scrollbar the box shows, a focus ring the box draws), and the
 * same depth is the box's scroll padding, so a row scrolled into view lands
 * clear of it; `reveal="flush"` is for a list sized to whole rows, which must
 * rest on a row boundary and not a shadow's depth above one.
 *
 * `scroll="own"` renders the box. `scroll="enclosing"` is for a list that
 * scrolls inside a box something else owns: the list renders a plain block and
 * puts the shadow on the nearest ancestor that scrolls, for as long as it is
 * mounted. That is a box of its own list kind (the list then does nothing, the
 * box already has its shadow), a scroll area viewport, a modal's body (or the
 * scroll area the body scrolls in, the rule the modal uses to find its
 * scroller), or any other element whose `overflow-y` is `auto` or `scroll`.
 * Several enclosing lists may share one box, which keeps its shadow until the
 * last of them goes. A list with no scrolling ancestor has nothing to shadow,
 * says so in the console and renders without one.
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
      /** `flush` drops the scroll padding, for a box a whole number of rows
       * tall that must come to rest on a row. */
      reveal?: "clear" | "flush"
      scrollerRef?: (node: HTMLDivElement | null) => void
    }
  | { scroll: "enclosing" }
)

const FADE = "data-scroll-list-fade"
const OWN_BOX = "data-scroll-list-box"
const MORE_ABOVE = "data-more-above"
const MORE_BELOW = "data-more-below"
const SCROLLBAR_VAR = "--dux-list-fade-bar"

/** The `peek` height: a cap of three and a half project shortcut rows, and the
 * room the same rows are held while they load. A shortcut row is 2.25rem with a
 * fine pointer and 2.75rem (its 44px floor) on a phone or under a finger;
 * `--rows` is how many rows are coming. */
export const PEEK = {
  cap: "max-h-[8.75rem] max-md:max-h-[9.625rem] pointer-coarse:max-h-[9.625rem]",
  held: "h-[8.75rem] max-md:h-[9.625rem] pointer-coarse:h-[9.625rem]",
  heldForCount:
    "h-[min(calc(var(--rows)*2.25rem),8.75rem)] max-md:h-[min(calc(var(--rows)*2.75rem),9.625rem)] pointer-coarse:h-[min(calc(var(--rows)*2.75rem),9.625rem)]",
} as const

const HEIGHT = {
  fill: "min-h-0 flex-1",
  peek: `flex flex-col ${PEEK.cap}`,
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
  reveal,
  scrollerRef,
  children,
}: Extract<ScrollListProps, { scroll: "own" }>) {
  const [node, setNode] = React.useState<HTMLDivElement | null>(null)
  const { moreAbove, moreBelow, scrollbar: bar } = useScrollEdgesOf(node)
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
      {...{ [FADE]: "", [OWN_BOX]: "" }}
      data-scroll-list-flush={reveal === "flush" ? "" : undefined}
      data-more-above={moreAbove ? "" : undefined}
      data-more-below={moreBelow ? "" : undefined}
      style={
        {
          [SCROLLBAR_VAR]: `${bar}px`,
          ...(measured ? { height: height.px } : null),
        } as React.CSSProperties
      }
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

type Scroller = HTMLElement | "own" | null

// The box an enclosed list scrolls in: the nearest ancestor that is one, the
// way a browser would scroll it. A modal's body is found by its slot rather
// than its styles, and a modal's scroller is the scroll area around the body
// when it has one.
function enclosingScroller(list: HTMLElement): Scroller {
  for (let el = list.parentElement; el !== null; el = el.parentElement) {
    if (el.hasAttribute(OWN_BOX)) return "own"
    if (el.dataset.slot === "scroll-area-viewport") return el
    if (el.dataset.slot === "modal-body") {
      return el.closest<HTMLElement>('[data-slot="scroll-area-viewport"]') ?? el
    }
    const overflow = getComputedStyle(el).overflowY
    if (overflow === "auto" || overflow === "scroll") return el
  }
  return null
}

// A box several enclosing lists share keeps its marks until the last one goes.
const marked = new WeakMap<HTMLElement, number>()

function mark(box: HTMLElement) {
  marked.set(box, (marked.get(box) ?? 0) + 1)
  box.setAttribute(FADE, "")
}

function unmark(box: HTMLElement) {
  const left = (marked.get(box) ?? 1) - 1
  if (left > 0) {
    marked.set(box, left)
    return
  }
  marked.delete(box)
  box.removeAttribute(FADE)
  box.removeAttribute(MORE_ABOVE)
  box.removeAttribute(MORE_BELOW)
  box.style.removeProperty(SCROLLBAR_VAR)
}

function EnclosedList({ children }: { children?: React.ReactNode }) {
  const [list, setList] = React.useState<HTMLDivElement | null>(null)
  const found = React.useMemo(() => (list === null ? undefined : enclosingScroller(list)), [list])
  const scroller = typeof found === "object" && found !== null ? found : null
  const { moreAbove, moreBelow, scrollbar } = useScrollEdgesOf(scroller)

  React.useEffect(() => {
    if (found === null) {
      console.error(
        'ScrollList scroll="enclosing" has no scrolling ancestor (a scroll area, a modal body, or an element with overflow-y auto or scroll), so it shows no shadow.',
      )
    }
  }, [found])

  // Attributes and a custom property and never a class: the box's owner
  // re-renders its own class names (a modal's body changes them between card
  // and cover) and would drop one added here, while what it never set is left
  // alone.
  React.useLayoutEffect(() => {
    if (scroller === null) return
    mark(scroller)
    return () => unmark(scroller)
  }, [scroller])
  React.useLayoutEffect(() => {
    if (scroller === null) return
    scroller.toggleAttribute(MORE_ABOVE, moreAbove)
    scroller.toggleAttribute(MORE_BELOW, moreBelow)
    scroller.style.setProperty(SCROLLBAR_VAR, `${scrollbar}px`)
  }, [scroller, moreAbove, moreBelow, scrollbar])

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
