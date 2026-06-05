import { CircleX, TriangleAlert } from "lucide-react"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Button } from "@/components/ui/button"
import { reconnect, useDux } from "@/lib/store"
import { statusPresentation } from "@/lib/statusLine"
import type { ConnState } from "@/lib/types"

// The ONE connection indicator, bottom-left of the statusline bar. A small
// colored dot gives the at-a-glance state; a short label spells it out. Colors
// follow the app's soft-color convention: green=open, amber=in-progress,
// red=failed. "closed" is amber, not red: the socket auto-retries (up to 4×
// with backoff) before declaring failure, so a normal blip reads as
// recovering — red is reserved for "gave up, needs your action".
const CONN: Record<ConnState, { dot: string; label: string }> = {
  open: { dot: "bg-green-500", label: "Connected" },
  connecting: { dot: "bg-amber-500", label: "Connecting" },
  closed: { dot: "bg-amber-500", label: "Reconnecting…" },
  failed: { dot: "bg-red-500", label: "Connection failed" },
}

function ConnectionIndicator() {
  const { conn } = useDux()
  const c = CONN[conn]

  return (
    <div className="flex shrink-0 items-center gap-2">
      <span className={`size-2 shrink-0 rounded-full ${c.dot}`} aria-hidden />
      <span className="truncate">{c.label}</span>
      {conn === "failed" ? (
        <Button variant="outline" size="sm" onClick={reconnect}>
          Reconnect
        </Button>
      ) : null}
    </div>
  )
}

// The persistent statusline, rendered 1:1 with the TUI: tone drives both the
// text color and the leading iconography (so the meaning survives in monochrome
// too). An empty message renders nothing.
function StatusLine() {
  const { statusLine } = useDux()
  if (!statusLine.message) {
    return null
  }
  const { icon, className } = statusPresentation(statusLine.tone)

  return (
    <div className={`flex min-w-0 items-center gap-1.5 ${className}`}>
      {icon === "spinner" ? <BrailleSpinner /> : null}
      {icon === "warning" ? (
        <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
      ) : null}
      {icon === "error" ? (
        <CircleX className="size-3.5 shrink-0" aria-hidden />
      ) : null}
      <span className="truncate" title={statusLine.message}>
        {statusLine.message}
      </span>
    </div>
  )
}

export function StatusBar() {
  return (
    <footer className="flex h-7 shrink-0 items-center gap-3 border-t px-3 text-xs text-muted-foreground">
      <ConnectionIndicator />
      <StatusLine />
    </footer>
  )
}
