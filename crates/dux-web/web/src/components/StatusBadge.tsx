import { Circle, CirclePause } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import type { SessionStatus } from "@/lib/types"
import { cn } from "@/lib/utils"

// The one session-status badge, colour and icon mirroring the TUI. The colour
// lives on the icon and label only, with no pill background. `bg-transparent` is
// explicit because the base Badge's default variant would otherwise show its
// `bg-primary` through.
const STATUS: Record<
  SessionStatus,
  { className: string; Icon: typeof Circle; fill: boolean; label: string }
> = {
  active: {
    className: "border-transparent bg-transparent text-green-500",
    Icon: Circle,
    fill: true,
    label: "active",
  },
  detached: {
    className: "border-transparent bg-transparent text-amber-500",
    Icon: CirclePause,
    fill: false,
    label: "detached",
  },
  exited: {
    className: "border-transparent bg-transparent text-muted-foreground",
    Icon: Circle,
    fill: false,
    label: "exited",
  },
}

export function StatusBadge({
  status,
  iconOnly = false,
  working = false,
}: {
  status: SessionStatus
  // Compact mode for tight rows (the sidebar): show just the colored icon and
  // reveal the label in a tooltip on hover, so long agent names keep their room.
  iconOnly?: boolean
  // Extends an active badge's label while the agent streams output. The motion
  // cues for working live on the agent row, so the badge stays calm. Honored for
  // the active status only.
  working?: boolean
}) {
  const s = STATUS[status]
  const streaming = status === "active" && working
  const label = streaming ? `${s.label} — working` : s.label

  // Status icons rest slightly transparent, being quiet metadata. Only the active
  // dot pulses its opacity while the agent works, settling back to the resting
  // value when work stops (see .agent-status-dot in index.css).
  const dotClass = cn(
    "size-2.5 agent-status-dot",
    s.fill && "fill-current",
    streaming && "agent-status-dot--on",
  )

  const dot = <s.Icon className={dotClass} />

  if (iconOnly) {
    return (
      <TooltipProvider delay={300}>
        <Tooltip>
          <TooltipTrigger
            render={
              // `role="img"` so the aria-label is announced: a bare span's is
              // ignored by most screen readers, and the dot is visual only.
              <Badge
                role="img"
                className={`${s.className} px-1.5`}
                aria-label={label}
              />
            }
          >
            {dot}
          </TooltipTrigger>
          <TooltipContent side="right">{label}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
    )
  }

  return (
    <Badge className={s.className}>
      <s.Icon data-icon="inline-start" className={dotClass} />
      {label}
    </Badge>
  )
}
