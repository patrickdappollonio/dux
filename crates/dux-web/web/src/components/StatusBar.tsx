import { Button } from "@/components/ui/button"
import { reconnect, useDux } from "@/lib/store"
import type { ConnState } from "@/lib/types"

// The ONE connection indicator, bottom-left of the statusline bar. A small
// colored dot gives the at-a-glance state; a short label spells it out. Colors
// follow the app's soft-color convention: green=open, amber=in-progress,
// red=failed. "closed" is amber, not red: the socket auto-retries (a few times
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

export function StatusBar() {
  return (
    <footer className="flex h-7 shrink-0 items-center gap-3 border-t px-3 text-xs text-muted-foreground">
      <ConnectionIndicator />
    </footer>
  )
}
