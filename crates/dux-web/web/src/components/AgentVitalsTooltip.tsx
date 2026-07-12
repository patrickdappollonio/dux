import { Circle } from "lucide-react"
import { Fragment } from "react"

import { buildAgentVitals } from "@/lib/agentVitals"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// The "full vitals" tooltip content shared by the collapsed icon rail and the
// expanded sidebar agent rows, so the two surfaces can never drift. Pure
// presentational: all data comes in as props (the row model is built by the
// framework-free `buildAgentVitals`, kept separately so it stays unit-testable
// without mounting a tooltip). Renders on the popover surface (see
// components/ui/tooltip.tsx) inherited from the wrapping SimpleTooltip/Tooltip.
export function AgentVitalsTooltip({
  session,
  projectName,
  changesCount,
}: {
  session: SessionView
  projectName: string
  changesCount: number | null
}) {
  const vitals = buildAgentVitals(session, projectName, changesCount)

  return (
    <div className="flex w-64 flex-col gap-1.5 text-xs">
      <div className="flex items-center gap-1.5">
        <span className="truncate font-medium text-sm">{vitals.name}</span>
        <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
          {vitals.provider}
        </span>
      </div>
      <div className="flex items-center gap-1.5 text-muted-foreground">
        <Circle className={cn("size-2 shrink-0 fill-current", vitals.statusColorClass)} />
        <span className={vitals.statusColorClass}>{vitals.statusLabel}</span>
        <span>· {vitals.projectName}</span>
      </div>
      {vitals.rows.length > 0 ? (
        <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1 border-t pt-1.5">
          {vitals.rows.map((row) => (
            <Fragment key={row.key}>
              <span className="text-muted-foreground">{row.label}</span>
              <span
                className={cn(
                  "min-w-0 truncate text-right",
                  row.mono && "font-mono",
                )}
              >
                {row.value}
              </span>
            </Fragment>
          ))}
        </div>
      ) : null}
    </div>
  )
}
