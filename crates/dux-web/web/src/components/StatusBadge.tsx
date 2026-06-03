import { Circle, CirclePause } from "lucide-react"

import { Badge } from "@/components/ui/badge"
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

export function StatusBadge({ status }: { status: SessionStatus }) {
  const s = STATUS[status]
  return (
    <Badge className={s.className}>
      <s.Icon
        data-icon="inline-start"
        className={`size-2.5 ${s.fill ? "fill-current" : ""}`}
      />
      {s.label}
    </Badge>
  )
}
