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
// detached=amber/◐, exited=muted/○). Soft-color: tinted background + matching
// text. This is the single source of truth for session-status badges.
const STATUS: Record<
  SessionStatus,
  { className: string; Icon: typeof Circle; fill: boolean; label: string }
> = {
  active: {
    className: "border-transparent bg-green-600/15 text-green-500",
    Icon: Circle,
    fill: true,
    label: "active",
  },
  detached: {
    className: "border-transparent bg-amber-600/15 text-amber-500",
    Icon: CirclePause,
    fill: false,
    label: "detached",
  },
  exited: {
    className: "border-transparent bg-muted text-muted-foreground",
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
  // When the agent is actively streaming output, an active badge gains a ping
  // ring radiating from its dot and its label becomes "active — working".
  // Honored only for the active status; ignored otherwise.
  working?: boolean
}) {
  const s = STATUS[status]
  const streaming = status === "active" && working
  const label = streaming ? `${s.label} — working` : s.label

  // The dot, optionally wrapped so a ping copy radiates from behind it. The
  // wrapper is sized to the icon and the ping copy is absolutely positioned, so
  // the ring never shifts surrounding layout. Gated on motion-safe: so users
  // with prefers-reduced-motion see a plain (non-animated) dot.
  const dot = streaming ? (
    <span className="relative inline-flex size-2.5">
      <s.Icon className="absolute inset-0 size-2.5 fill-current motion-safe:animate-ping" />
      <s.Icon className="relative size-2.5 fill-current" />
    </span>
  ) : (
    <s.Icon className={`size-2.5 ${s.fill ? "fill-current" : ""}`} />
  )

  if (iconOnly) {
    return (
      <TooltipProvider delay={300}>
        <Tooltip>
          <TooltipTrigger
            render={
              <Badge className={`${s.className} px-1.5`} aria-label={label} />
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
      {streaming ? (
        <span data-icon="inline-start" className="relative inline-flex size-2.5">
          <s.Icon className="absolute inset-0 size-2.5 fill-current motion-safe:animate-ping" />
          <s.Icon className="relative size-2.5 fill-current" />
        </span>
      ) : (
        <s.Icon
          data-icon="inline-start"
          className={`size-2.5 ${s.fill ? "fill-current" : ""}`}
        />
      )}
      {label}
    </Badge>
  )
}
