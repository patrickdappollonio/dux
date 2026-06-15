import { Circle, CirclePause } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import type { SessionStatus } from "@/lib/types"

// Meaningful status color + icon, mirroring the dux TUI (active=green/●,
// detached=amber/◐, exited=muted/○). The color lives on the icon and label
// only — no pill background — so the indicator stays minimal. `bg-transparent`
// is explicit because the base Badge defaults to `variant="default"`, whose
// `bg-primary` would otherwise show through. This is the single source of truth
// for session-status badges.
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
  // When the agent is actively streaming output, an active badge's label becomes
  // "active — working". The MOTION cue for "working" lives on the agent icon (a
  // gentle bounce in the sidebar/mobile rows), so the badge itself stays calm —
  // one animation per row, not two. Honored only for active; ignored otherwise.
  working?: boolean
}) {
  const s = STATUS[status]
  const streaming = status === "active" && working
  const label = streaming ? `${s.label} — working` : s.label

  const dot = <s.Icon className={`size-2.5 ${s.fill ? "fill-current" : ""}`} />

  if (iconOnly) {
    return (
      <TooltipProvider delay={300}>
        <Tooltip>
          <TooltipTrigger
            render={
              // role="img" so the aria-label (e.g. "active — working") is
              // actually announced — a bare span's aria-label is otherwise
              // ignored by most screen readers, and the dot/bounce are visual
              // (and motion-safe) only.
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
      <s.Icon
        data-icon="inline-start"
        className={`size-2.5 ${s.fill ? "fill-current" : ""}`}
      />
      {label}
    </Badge>
  )
}
