import { useState } from "react"
import { Bot, Plus, SquareTerminal } from "lucide-react"
import type { LucideIcon } from "lucide-react"
import type { ReactNode } from "react"

import { AttentionDot } from "@/components/AttentionDot"
import { layoutControls } from "@/components/stackLayoutControls"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { defaultProviderForSession } from "@/lib/agentTabs"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { FOCUS_RING } from "@/lib/focusRing"
import type { ComputerLayout, LayoutAction } from "@/lib/layoutModel"
import { addTab, createTerminal, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"

/** Where the pill rests on the stack's box. */
export type LayoutPillPlacement =
  /** Centred on the divider, whose top edge sits this far down the stack. */
  | { kind: "divider"; agentPercent: number }
  /** Tucked inside the bottom of the stack, over a divider folded to it. */
  | { kind: "bottom" }
  /** Tucked inside the top of the stack, under a divider folded to it. */
  | { kind: "top" }

// The pill's buttons are the cluster's: one height token, lifted to the touch
// floor under a finger. Their neighbours are each other, 2px apart.
const PILL_BUTTON = "size-8 shrink-0 rounded-full pointer-coarse:size-10"

// What an agent's stack floats when its strips are hidden: the layout controls
// and a +, in a rounded pill styled like the phone's floating one, at the right
// end of the divider. With the terminal maximized it leads with a dot carrying
// the agent's state, since the agent is then off screen. An agent with no
// terminals has no divider and gets the + alone at its bottom right.
export function StackLayoutPill({
  session,
  layout,
  hasDock,
  placement,
  onLayoutAction,
}: {
  session: SessionView
  layout: ComputerLayout
  hasDock: boolean
  placement: LayoutPillPlacement
  onLayoutAction: (action: LayoutAction) => void
}) {
  const [adding, setAdding] = useState(false)
  return (
    <>
      <div
        data-testid="stack-layout-pill"
        className="dux-pill-surface absolute right-3 z-30 flex items-center gap-0.5 rounded-full border p-1 shadow-lg"
        style={placementStyle(placement)}
      >
        {hasDock && layout === "terminal" ? <AgentStateDot session={session} /> : null}
        <SimpleTooltip content="New agent tab or terminal">
          <Button
            variant="ghost"
            size="icon"
            aria-label="New agent tab or terminal…"
            className={PILL_BUTTON}
            onClick={() => setAdding(true)}
          >
            <Plus />
          </Button>
        </SimpleTooltip>
        {hasDock
          ? layoutControls(layout).map((control) => (
              <SimpleTooltip key={control.action} content={control.label}>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={control.label}
                  className={PILL_BUTTON}
                  onClick={() => onLayoutAction(control.action)}
                >
                  <control.icon />
                </Button>
              </SimpleTooltip>
            ))
          : null}
      </div>
      <StackNewDialog session={session} open={adding} onClose={() => setAdding(false)} />
    </>
  )
}

function placementStyle(placement: LayoutPillPlacement): React.CSSProperties {
  switch (placement.kind) {
    case "divider":
      // The divider is 8px, so its middle is 4px below where it starts.
      return { top: `calc(${placement.agentPercent}% + 4px)`, transform: "translateY(-50%)" }
    case "bottom":
      return { bottom: "12px" }
    case "top":
      return { top: "12px" }
  }
}

// What the agent is doing, for the pill over its maximized terminal: the
// attention look when it needs you, the working pulse while it works, nothing
// when it is idle. Needs-you outranks working, as on its row.
export function AgentStateDot({ session }: { session: SessionView }) {
  if (session.needs_attention) {
    return (
      <span className="flex size-8 items-center justify-center pointer-coarse:size-10">
        <AttentionDot />
      </span>
    )
  }
  if (session.status === "active" && session.working && !session.typing) {
    return (
      <span className="flex size-8 items-center justify-center pointer-coarse:size-10">
        <SimpleTooltip content="The agent is working">
          <span
            aria-label="The agent is working"
            className="size-2 rounded-full bg-dux-working motion-safe:animate-working-pulse"
          />
        </SimpleTooltip>
      </span>
    )
  }
  return null
}

// The pill's + dialog: another of this agent's tabs, on its default provider,
// or another terminal in its dock. It runs exactly what the agent's own menu
// runs, and another provider stays in that menu's New agent tab….
export function StackNewDialog({
  session,
  open,
  onClose,
}: {
  session: SessionView
  open: boolean
  onClose: () => void
}) {
  const { spine, bootstrap, createTabInFlight } = useDux()
  const provider = defaultProviderForSession(spine, session)
  const cap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const tabDisabled =
    session.tabs.length >= cap || createTabInFlight.includes(session.id)
  const run = (act: () => void) => {
    onClose()
    act()
  }
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
      <DialogContent className="gap-3 sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add to this agent</DialogTitle>
          <DialogDescription>
            Another tab works in the same worktree; a terminal opens in its dock.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-0.5">
          <NewRow
            icon={Bot}
            title="New agent tab"
            excerpt={
              <>
                Starts <InlineCode>{provider}</InlineCode> beside this agent&rsquo;s
                other tabs.
              </>
            }
            disabled={tabDisabled}
            autoFocus
            onActivate={() => run(() => addTab(session.id))}
          />
          <NewRow
            icon={SquareTerminal}
            title="New terminal"
            excerpt="Opens a shell in this agent's worktree, in its dock."
            onActivate={() => run(() => createTerminal(session.id))}
          />
        </div>
      </DialogContent>
    </Dialog>
  )
}

function NewRow({
  icon: Icon,
  title,
  excerpt,
  disabled = false,
  autoFocus = false,
  onActivate,
}: {
  icon: LucideIcon
  title: string
  excerpt: ReactNode
  disabled?: boolean
  autoFocus?: boolean
  onActivate: () => void
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      autoFocus={autoFocus}
      onClick={onActivate}
      // The rows sit 2px apart with nothing else between them, and each is at
      // least 56px tall, well over the touch floor.
      className={cn(
        "flex min-h-14 w-full items-center gap-3 rounded-lg border border-transparent px-2.5 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none",
        FOCUS_RING,
      )}
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-muted text-foreground/90">
        <Icon className="size-[18px]" />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-sm font-medium">{title}</span>
        <span className="text-[12.5px] leading-snug text-muted-foreground">{excerpt}</span>
      </span>
    </button>
  )
}
