import { useCallback, type ComponentProps, type ReactNode, type Ref } from "react"

import type { usePillSort } from "@/hooks/use-pill-sort"
import { HOVER_REVEAL, REVEAL_ROW, REVEALED_ON_ANY_FOCUS } from "@/lib/hoverReveal"
import { scrollFocusedTabIntoView } from "@/lib/tabFocusScroll"
import { cn } from "@/lib/utils"

// The tab strip and pill behind an agent's provider tabs, its terminal dock and
// the editor's file tabs; each strip supplies its pills' content and its controls.

/**
 * The strip's scroller. Its pills must be its direct children (a
 * `SortableStrip` renders no element), because the arrow keys walk them there.
 */
export function TabStrip({
  stripRef,
  trailing,
  children,
  "data-testid": testId,
}: {
  stripRef?: Ref<HTMLDivElement>
  /** Controls after the pills: a + and whatever else the strip carries. */
  trailing?: ReactNode
  children: ReactNode
  "data-testid"?: string
}) {
  return (
    // A phone pads 3px a side, a focus ring's reach, since the scroller clips past
    // it. A tablet gets 8px between pills so a control's tap margin clears the next
    // pill, and an 18px end: the pane divider's touch band reaches 10px in.
    <div
      ref={stripRef}
      data-testid={testId}
      data-toast-clear
      onFocus={(e) => scrollFocusedTabIntoView(e.target)}
      className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1.25 scroll-px-2 max-md:py-0.75 md:pointer-coarse:gap-2 md:pointer-coarse:pr-4.5"
    >
      {children}
      {trailing}
    </div>
  )
}

type PillSort = ReturnType<typeof usePillSort>

/** One pill, its slots in the order the props list them; handlers, data
 * attributes and a ref land on the pill's element. */
export function TabPill({
  active,
  sort,
  icon,
  badge,
  label,
  italic = false,
  marker,
  menu,
  trailing,
  ref,
  ...rest
}: Omit<
  ComponentProps<"div">,
  "className" | "style" | "role" | "tabIndex" | "aria-selected" | "children"
> & {
  active: boolean
  /** Makes the pill draggable along a `SortableStrip`. */
  sort?: PillSort
  icon: ReactNode
  badge?: ReactNode
  label: ReactNode
  /** An editor preview tab's label. */
  italic?: boolean
  marker?: ReactNode
  /** Revealed on hover and focus, always under a finger. */
  menu?: ReactNode
  trailing?: ReactNode
}) {
  const setNodeRef = sort?.setNodeRef
  const attach = useCallback(
    (node: HTMLDivElement | null) => {
      setNodeRef?.(node)
      if (typeof ref === "function") ref(node)
      else if (ref) ref.current = node
    },
    [setNodeRef, ref],
  )
  return (
    <div
      ref={attach}
      style={sort?.style}
      {...sort?.listeners}
      {...rest}
      role="tab"
      aria-selected={active}
      tabIndex={0}
      // 36px on a phone and a tablet relaxes the 40px floor vertically only: the
      // neighbours there are the header and the pane, not other tabs. `max-md:py-0`
      // keeps a phone control's hit area from adding the padding back.
      className={cn(
        "flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-9 max-md:py-0 md:pointer-coarse:min-h-9",
        REVEAL_ROW,
        // A long press starts a drag, so it must not select text or raise the
        // touch callout.
        sort && "touch-manipulation select-none [-webkit-touch-callout:none]",
        sort?.isDragging && "relative z-10 shadow-lg",
        active
          ? "border-border bg-background text-foreground"
          : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
      )}
    >
      {icon}
      {badge}
      <span
        // `pr-0.5` sits inside the truncating box so an italic final ascender is
        // not clipped, and on every label so italic never shifts one.
        className={cn("max-w-40 truncate pr-0.5", italic && "italic")}
      >
        {label}
      </span>
      {marker}
      {menu != null ? (
        // The phone keeps the slot's width open for its 44px trigger.
        <div
          className={cn(
            HOVER_REVEAL,
            "[--reveal-cap:--spacing(8)] max-md:max-w-none",
            REVEALED_ON_ANY_FOCUS,
          )}
        >
          {menu}
        </div>
      ) : null}
      {trailing}
    </div>
  )
}
