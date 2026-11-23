import { Plus, SquareTerminal, X } from "lucide-react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { SortableStrip } from "@/components/SortableStrip"
import {
  TAB_STRIP_BUTTON_DISABLED_TAP_AREA,
  TabPill,
  TabStrip,
  TabStripButton,
} from "@/components/TabStrip"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import {
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import {
  addToAgentBlockHint,
  agentChangeBlockReason,
  useRowDriverLabel,
  useAgentChangeBlockById,
} from "@/hooks/use-agent-active-elsewhere"
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
import type { RowBlock } from "@/lib/paneCover"
import { terminalTitle } from "@/lib/terminals"
import type { TerminalView } from "@/lib/types"

// An agent's terminal dock strip: one pill per terminal of the agent, in the
// dock's order, the same pills the provider tab strip draws. A pill's glyph
// pulses and bounces while its terminal runs, a click brings that terminal to
// the front, a drag along the strip reorders the dock, and its ⋯ holds that
// terminal's own actions. The + opens another terminal, and on a computer the
// strip's right end carries the layout controls; a phone shows one part at a
// time and has none. While this device does not drive the agent, the +, the
// reorder and every pill's actions are disabled, as on the agent's tab strip.
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
  const block = useAgentChangeBlockById(sessionId)
  const device = useRowDriverLabel({ sessionId })
  return (
    <TabStrip
      label="Terminals"
      besidePaneDivider
      stripRef={attachStrip}
      data-testid="dock-strip"
      trailing={
        <DockControls
          sessionId={sessionId}
          block={block}
          device={device}
          layout={layout}
          onLayoutAction={onLayoutAction}
        />
      }
    >
      <SortableStrip
        ids={ids}
        onReorder={(next) => reorderDock(sessionId, next)}
        disabled={block !== null}
      >
        {terminals.map((terminal) => (
          <DockPill
            key={terminal.id}
            sessionId={sessionId}
            terminal={terminal}
            label={terminalTitle(terminal, terminals)}
            active={terminal.id === activeTerminal}
            withMenu={terminal.id !== unlisted}
            block={block}
          />
        ))}
      </SortableStrip>
    </TabStrip>
  )
}

function DockControls({
  sessionId,
  block,
  device,
  layout,
  onLayoutAction,
}: {
  sessionId: string
  block: RowBlock | null
  device: string | null
  layout?: ComputerLayout
  onLayoutAction?: (action: LayoutAction) => void
}) {
  return (
    <>
      <ExplainedWhenDisabled
        disabled={block !== null}
        hint={block === null ? "New terminal" : addToAgentBlockHint(block, device)}
        // 2px more on a tablet keeps the +'s tap area off the last pill's ⋯,
        // whose own reaches 1px past its pill.
        className="md:pointer-coarse:ml-0.5"
        disabledClassName={TAB_STRIP_BUTTON_DISABLED_TAP_AREA}
      >
        <TabStripButton
          aria-label="New terminal"
          disabled={block !== null}
          onClick={() => createTerminal(sessionId)}
        >
          <Plus className="size-4" />
        </TabStripButton>
      </ExplainedWhenDisabled>
      {/* The layout controls sit at the strip's far end, apart from the + by
          the free space between, so a press meant for one cannot land on the
          other. On a tablet 16px between them keeps their tap areas apart. */}
      {layout !== undefined && onLayoutAction !== undefined ? (
        <div className="ml-auto flex shrink-0 items-center gap-0.5 pl-2 md:pointer-coarse:gap-4">
          {layoutControls(layout).map((control) => (
            <SimpleTooltip key={control.action} content={control.label}>
              <TabStripButton
                aria-label={control.label}
                onClick={() => onLayoutAction(control.action)}
              >
                <control.icon className="size-3.5" />
              </TabStripButton>
            </SimpleTooltip>
          ))}
        </div>
      ) : null}
    </>
  )
}

function DockPill({
  sessionId,
  terminal,
  label,
  active,
  withMenu,
  block,
}: {
  sessionId: string
  terminal: TerminalView
  label: string
  active: boolean
  withMenu: boolean
  /** Why nothing here may change the agent, or null when it may. */
  block: RowBlock | null
}) {
  const sort = usePillSort(terminal.id)
  const select = () => selectTerminal(terminal.id, { kind: "session", sessionId })
  const device = useRowDriverLabel({ sessionId })
  return (
    <TabPill
      data-cue-key={terminal.id}
      active={active}
      sort={sort}
      onSelect={select}
      // The tab strip's glyph on the terminal row's ladder: typing stands the
      // cue down, and a terminal never needs you.
      icon={
        <WorkingGlyph
          icon={SquareTerminal}
          working={terminal.working && !terminal.typing}
          className="size-3.5"
        />
      }
      label={label}
      menu={
        withMenu
          ? {
              label: "Terminal actions",
              content: (
                <DropdownMenuContent align="start" onClick={(e) => e.stopPropagation()}>
                  {/* This terminal's own actions only: the input rows and the editor
                      live in the pane's top menu. Neutral, like every destructive
                      item; the confirm is the danger signal. */}
                  {block !== null ? (
                    <DropdownMenuGroup>
                      <DropdownMenuLabel className="max-w-60 whitespace-normal">
                        {agentChangeBlockReason(block, device)}
                      </DropdownMenuLabel>
                      <DropdownMenuSeparator />
                    </DropdownMenuGroup>
                  ) : null}
                  <DropdownMenuItem
                    disabled={block !== null}
                    onClick={() => openDeleteTerminal(terminal.id)}
                  >
                    <X />
                    Close terminal…
                  </DropdownMenuItem>
                </DropdownMenuContent>
              ),
            }
          : undefined
      }
    />
  )
}
