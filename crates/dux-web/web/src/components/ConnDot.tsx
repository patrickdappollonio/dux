import { SimpleTooltip } from "@/components/SimpleTooltip"
import { useDux } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { cn } from "@/lib/utils"

// The one connection indicator, an at-a-glance health signal beside the logo; the
// actionable connection-lost case belongs to OfflineOverlay. "closed" is amber
// rather than red, because the socket auto-retries before declaring failure, and
// red is reserved for "gave up, needs your action".
const CONN: Record<ConnState, { dot: string; label: string }> = {
  open: { dot: "bg-green-500", label: "Connected" },
  connecting: { dot: "bg-amber-500", label: "Connecting" },
  closed: { dot: "bg-amber-500", label: "Reconnecting…" },
  failed: { dot: "bg-red-500", label: "Connection failed" },
}

// A passive status dot: the state comes from the store, the label rides an
// aria-label plus a tooltip, and callers position it through `className`.
export function ConnDot({ className }: { className?: string }) {
  const { conn } = useDux()
  // Fall back to the neutral "connecting" presentation for any unexpected state
  // (e.g. an as-yet-unset store), so the dot never crashes its host header.
  const c = CONN[conn] ?? CONN.connecting
  return (
    <SimpleTooltip content={c.label}>
      <span
        role="status"
        aria-label={c.label}
        className={cn("block size-2.5 shrink-0 rounded-full", c.dot, className)}
      />
    </SimpleTooltip>
  )
}
