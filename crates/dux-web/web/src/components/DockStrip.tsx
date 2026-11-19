import { Ellipsis, Plus, SquareTerminal, X } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { SortableStrip } from "@/components/SortableStrip"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useKeepCueClocks } from "@/hooks/use-keep-cue-clocks"
import { usePillSort } from "@/hooks/use-pill-sort"
import type { ComputerLayout, LayoutAction } from "@/lib/layoutModel"
import { layoutControls } from "@/components/stackLayoutControls"
import {
  createTerminal,
  openDeleteTerminal,
  reorderDock,
  selectTerminal,
} from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
import { cn } from "@/lib/utils"
import type { TerminalView } from "@/lib/types"

// The strip's icon buttons: the tab strip's own, at one height token. A phone
// lifts the box to the 40px floor; a tablet keeps the 24px box and an invisible
// 8px margin a side takes the tap area to 40px, as the tab strip's + does.
const STRIP_ICON_BUTTON =
  "relative flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors after:absolute after:-inset-2 after:hidden hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:pointer-coarse:size-10 md:pointer-coarse:after:block"

// An agent's terminal dock strip: one pill per terminal of the agent, in the
// dock's order, the same pills the provider tab strip draws. A pill's glyph
// pulses and bounces while its terminal runs, a click brings that terminal to
// the front, a drag along the strip reorders the dock, and its ⋯ holds that
// terminal's own actions. The + opens another terminal, and on a computer the
// strip's right end carries the layout controls; a phone shows one part at a
// time and has none.
export function DockStrip({
  sessionId,
  terminals,
  activeTerminal,
  layout,
  onLayoutAction,
  unlisted = null,
}: {
  sessionId: string
  terminals: TerminalView[]
  activeTerminal: string | null
  /** A terminal just created that the workspace does not list yet: its pill
   * has no menu until it does, because its close confirm has nothing to show. */
  unlisted?: string | null
  /** Absent on a phone, which has no layout to change. */
  layout?: ComputerLayout
  onLayoutAction?: (action: LayoutAction) => void
}) {
  const ids = terminals.map((t) => t.id)
  const attachStrip = useKeepCueClocks(ids.join("\n"))
  return (
    // A tablet gets the tab strip's spacing: 8px between pills, so one pill's ⋯
    // tap margin stays clear of the next pill, and an 18px end, since the pane
    // divider's touch band reaches 10px into the strip and the last layout
    // control's tap margin takes the other 8.
    <div
      ref={attachStrip}
      data-testid="dock-strip"
      className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1 md:pointer-coarse:gap-2 md:pointer-coarse:pr-4.5"
    >
      <SortableStrip ids={ids} onReorder={(next) => reorderDock(sessionId, next)}>
        {terminals.map((terminal) => (
          <DockPill
            key={terminal.id}
            sessionId={sessionId}
            terminal={terminal}
            label={terminalTitle(terminal, terminals)}
            active={terminal.id === activeTerminal}
            withMenu={terminal.id !== unlisted}
          />
        ))}
      </SortableStrip>
      <SimpleTooltip content="New terminal">
        <button
          type="button"
          aria-label="New terminal"
          onClick={() => createTerminal(sessionId)}
          // 2px more on a tablet keeps its tap area off the last pill's ⋯,
          // whose own reaches 1px past its pill.
          className={cn(STRIP_ICON_BUTTON, "md:pointer-coarse:ml-0.5")}
        >
          <Plus className="size-4" />
        </button>
      </SimpleTooltip>
      {/* The layout controls sit at the strip's far end, apart from the + by
          the free space between, so a press meant for one cannot land on the
          other. On a tablet 16px between them keeps their tap areas apart. */}
      {layout !== undefined && onLayoutAction !== undefined ? (
        <div className="ml-auto flex shrink-0 items-center gap-0.5 pl-2 md:pointer-coarse:gap-4">
          {layoutControls(layout).map((control) => (
            <SimpleTooltip key={control.action} content={control.label}>
              <button
                type="button"
                aria-label={control.label}
                onClick={() => onLayoutAction(control.action)}
                className={STRIP_ICON_BUTTON}
              >
                <control.icon className="size-3.5" />
              </button>
            </SimpleTooltip>
          ))}
        </div>
      ) : null}
    </div>
  )
}

function DockPill({
  sessionId,
  terminal,
  label,
  active,
  withMenu,
}: {
  sessionId: string
  terminal: TerminalView
  label: string
  active: boolean
  withMenu: boolean
}) {
  const { setNodeRef, style, listeners, isDragging } = usePillSort(terminal.id)
  const select = () => selectTerminal(terminal.id, { kind: "session", sessionId })
  return (
    <div
      ref={setNodeRef}
      style={style}
      data-cue-key={terminal.id}
      {...listeners}
      role="tab"
      aria-selected={active}
      tabIndex={0}
      onClick={select}
      onKeyDown={(e) => {
        // Only a press on the pill itself: Enter and Space from the ⋯ and its
        // menu bubble here too.
        if (e.target !== e.currentTarget) return
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault()
          select()
        }
      }}
      className={cn(
        // 36px on a tablet, the tab strip's pill height.
        "group/tab flex shrink-0 cursor-pointer touch-manipulation select-none [-webkit-touch-callout:none] items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors md:pointer-coarse:min-h-9",
        isDragging && "relative z-10 shadow-lg",
        active
          ? "border-border bg-background text-foreground"
          : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
      )}
    >
      {/* The tab strip's glyph on the terminal row's ladder: typing stands the
          cue down, and a terminal never needs you. */}
      <WorkingGlyph
        icon={SquareTerminal}
        working={terminal.working && !terminal.typing}
        className="size-3.5"
      />
      <span className="max-w-40 truncate">{label}</span>
      {/* Consumes no layout space at rest, like the tab pill's. Always revealed
          on a tablet, so the clip drops there and the trigger's tap margin
          reaches past this wrapper. */}
      {withMenu ? (
        <div
          className={cn(
            "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-w-0 opacity-0 group-hover/tab:max-w-8 group-hover/tab:opacity-100 group-focus-within/tab:max-w-8 group-focus-within/tab:opacity-100 has-[[data-popup-open]]:max-w-8 has-[[data-popup-open]]:opacity-100 md:pointer-coarse:overflow-visible",
            ALWAYS_REVEALED_ON_TOUCH,
          )}
        >
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <button
                  type="button"
                  aria-label="Terminal actions"
                  onClick={(e) => e.stopPropagation()}
                  // The touch floor under a finger: its neighbours are the next
                  // pill and its own pill's select. A tablet keeps the 20px box
                  // and an invisible 10px margin takes the tap area to 40px.
                  className="relative flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground after:absolute after:-inset-2.5 after:hidden hover:text-foreground max-md:pointer-coarse:size-10 md:pointer-coarse:after:block"
                />
              }
            >
              <Ellipsis className="size-3.5" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" onClick={(e) => e.stopPropagation()}>
              {/* This terminal's own actions only: the input rows and the editor
                  live in the pane's top menu. Neutral, like every destructive
                  item; the confirm is the danger signal. */}
              <DropdownMenuItem onClick={() => openDeleteTerminal(terminal.id)}>
                <X />
                Close terminal…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      ) : null}
    </div>
  )
}
