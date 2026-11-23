import {
  createContext,
  useCallback,
  useContext,
  type ComponentProps,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
} from "react"

import { EllipsisMenuTrigger } from "@/components/EllipsisMenuTrigger"
import { DropdownMenu } from "@/components/ui/dropdown-menu"
import type { usePillSort } from "@/hooks/use-pill-sort"
import { REVEAL_ROW } from "@/lib/hoverReveal"
import { focusAdjacentTab, scrollFocusedTabIntoView } from "@/lib/tabFocusScroll"
import { cn } from "@/lib/utils"

// The tab strip and pill behind an agent's provider tabs, its terminal dock and
// the editor's file tabs; each strip supplies its pills' content and its controls.

const InStrip = createContext(false)

/**
 * The strip's scroller. The pills sit in its tab list, which holds nothing else
 * (a `SortableStrip` renders no element), because the arrow keys walk the
 * list's children; the trailing controls sit after the list, outside it, so a
 * screen reader does not count them among the tabs.
 */
export function TabStrip({
  label,
  besidePaneDivider,
  stripRef,
  trailing,
  children,
  "data-testid": testId,
}: {
  /** What a screen reader calls the list of tabs. */
  label: string
  /**
   * The strip ends at a pane divider, whose touch band reaches 10px into it, so
   * a tablet keeps the strip's last control clear of that band.
   */
  besidePaneDivider: boolean
  stripRef?: Ref<HTMLDivElement>
  /** Controls after the pills: a + and whatever else the strip carries. */
  trailing?: ReactNode
  children: ReactNode
  "data-testid"?: string
}) {
  return (
    // A phone pads 3px a side, a focus ring's reach, since the scroller clips past
    // it. A tablet gets 8px between pills so a control's tap margin clears the next
    // pill. Beside a pane divider a tablet's end is 18px: the divider's band takes
    // 10px and the last control's tap margin the other 8.
    <div
      ref={stripRef}
      data-testid={testId}
      data-toast-clear
      onFocus={(e) => scrollFocusedTabIntoView(e.target)}
      className={cn(
        "flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1.25 scroll-px-2 max-md:py-0.75 md:pointer-coarse:gap-2",
        besidePaneDivider && "md:pointer-coarse:pr-4.5",
      )}
    >
      <div
        role="tablist"
        aria-label={label}
        aria-orientation="horizontal"
        className="flex shrink-0 items-center gap-1 md:pointer-coarse:gap-2"
      >
        <InStrip.Provider value={true}>{children}</InStrip.Provider>
      </div>
      {trailing}
    </div>
  )
}

/**
 * An icon button among a strip's trailing controls, the + included, so every
 * strip's controls share one box. A phone keeps the 44px width, because the
 * neighbour on that axis is another control or the last pill's `⋯`, and drops
 * the height to 36px, inside the strip's height, because above and below are
 * the header and the pane. A tablet keeps the 24px box and an invisible 8px
 * margin a side takes the tap area to 40px.
 */
export function TabStripButton(
  props: Omit<ComponentProps<"button">, "className" | "style" | "type">,
) {
  return (
    <button
      type="button"
      {...props}
      className="relative flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors after:absolute after:-inset-2 after:hidden hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:h-9 max-md:w-11 md:pointer-coarse:after:block"
    />
  )
}

/**
 * The tablet tap area of a disabled `TabStripButton`, for what holds it: a
 * disabled button takes no pointer events, so its own margin is dead.
 */
export const TAB_STRIP_BUTTON_DISABLED_TAP_AREA =
  "relative after:absolute after:-inset-2 after:hidden md:pointer-coarse:after:block"

type PillSort = ReturnType<typeof usePillSort>

// What the pill element takes from a caller. Its click and its keys are the
// pill's own (`onSelect`), so a strip cannot leave one out.
type PillElementProps = Omit<
  ComponentProps<"div">,
  | "className"
  | "style"
  | "role"
  | "tabIndex"
  | "aria-selected"
  | "children"
  | "onClick"
  | "onKeyDown"
>

/** One pill, its slots in the order the props list them; other handlers, data
 * attributes and a ref land on the pill's element. */
export function TabPill({
  active,
  onSelect,
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
}: PillElementProps & {
  active: boolean
  /** A click on the pill, or Enter or Space while the pill itself has focus. */
  onSelect: () => void
  /** Makes the pill draggable along a `SortableStrip`. */
  sort?: PillSort
  icon: ReactNode
  badge?: ReactNode
  label: ReactNode
  /** An editor preview tab's label. */
  italic?: boolean
  marker?: ReactNode
  /** The pill's `⋯` menu: the trigger's name and the menu's content. */
  menu?: { label: string; content: ReactNode }
  trailing?: ReactNode
}) {
  if (!useContext(InStrip)) {
    throw new Error("A TabPill renders inside a TabStrip, whose tab list its arrow keys walk.")
  }
  const setNodeRef = sort?.setNodeRef
  const attach = useCallback(
    (node: HTMLDivElement | null) => {
      setNodeRef?.(node)
      if (typeof ref === "function") ref(node)
      else if (ref) ref.current = node
    },
    [setNodeRef, ref],
  )
  // The pill's own handlers and the drag's run first; one of the same name
  // that reaches the pill from outside (a caller, or a tooltip wrapped around
  // it) runs after them rather than replacing them.
  const handlers: Record<string, unknown> = { ...rest }
  const first: Record<string, (event: never) => void> = {
    onClick: onSelect,
    onKeyDown: (e: KeyboardEvent<HTMLDivElement>) => {
      // Only a press on the pill itself: a key on a control inside it (its ⋯,
      // the editor's ✕) or in the ⋯'s menu, which bubbles here through the
      // portal, is that control's.
      if (e.target !== e.currentTarget) return
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault()
        onSelect()
        return
      }
      focusAdjacentTab(e)
    },
  }
  for (const [name, handler] of [
    ...Object.entries(first),
    ...Object.entries(sort?.listeners ?? {}),
  ]) {
    const after = handlers[name] as ((event: unknown) => void) | undefined
    handlers[name] =
      after === undefined
        ? handler
        : (event: never) => {
            handler(event)
            after(event)
          }
  }
  return (
    <div
      ref={attach}
      style={sort?.style}
      {...handlers}
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
      {menu !== undefined ? (
        <DropdownMenu>
          <EllipsisMenuTrigger
            placement="tab-pill"
            reveal="hover-or-focus"
            aria-label={menu.label}
          />
          {menu.content}
        </DropdownMenu>
      ) : null}
      {trailing}
    </div>
  )
}
