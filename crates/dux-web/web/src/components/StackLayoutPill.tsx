import { Circle } from "lucide-react"

import { layoutControls } from "@/components/stackLayoutControls"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import { Button } from "@/components/ui/button"
import type { ComputerLayout, LayoutAction } from "@/lib/layoutModel"
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

// What an agent's stack floats over its divider when its strips are hidden:
// the layout controls, in a rounded pill styled like the phone's floating one,
// at the right end of the divider. With the terminal maximized it leads with a
// dot carrying the agent's state, since the agent is then off screen. Only an
// agent with terminals has a layout to change, so only its stack floats one;
// adding a tab or a terminal is the pane header's New tab….
export function StackLayoutPill({
  session,
  layout,
  placement,
  onLayoutAction,
}: {
  session: SessionView
  layout: ComputerLayout
  placement: LayoutPillPlacement
  onLayoutAction: (action: LayoutAction) => void
}) {
  return (
    <div
      data-testid="stack-layout-pill"
      className="dux-pill-surface absolute right-3 z-30 flex items-center gap-0.5 rounded-full border p-1 shadow-lg"
      style={placementStyle(placement)}
    >
      {layout === "terminal" ? <AgentStateDot session={session} /> : null}
      {layoutControls(layout).map((control) => (
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
      ))}
    </div>
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
// when it is idle. Needs-you outranks working, as on its row. It is the working
// glyph drawn as a dot, without the bounce: a hand-over to needs-you lets the
// pulse land before the attention look eases in, as on the list.
export function AgentStateDot({ session }: { session: SessionView }) {
  const attention = session.needs_attention
  const working = !attention && session.status === "active" && session.working && !session.typing
  if (!attention && !working) return null
  const label = attention ? "Needs attention" : "The agent is working"
  return (
    <span className="flex size-8 items-center justify-center pointer-coarse:size-10">
      <SimpleTooltip content={label}>
        <span role="img" aria-label={label} className="inline-flex">
          <WorkingGlyph
            icon={Circle}
            working={working}
            look={
              attention
                ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
                : "text-dux-working"
            }
            bounce={false}
            cueKey={`layout-dot:${session.id}`}
            className="size-2 fill-current stroke-none"
          />
        </span>
      </SimpleTooltip>
    </span>
  )
}
