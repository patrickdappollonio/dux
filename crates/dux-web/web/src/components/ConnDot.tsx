import { SimpleTooltip } from "@/components/SimpleTooltip"
import { useDux } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { cn } from "@/lib/utils"

// The ONE connection indicator, rendered as a small colored dot beside the dux
// logo (desktop sidebar header + mobile hub header). It replaces the old
// bottom-of-window "Connected" status bar: engine statuses surface as toasts,
// and the actionable "connection lost" case is owned by OfflineOverlay, so all
// this pip needs to do is give an at-a-glance health signal. Colors follow the
// app's soft-color convention: green=open, amber=in-progress, red=failed.
// "closed" is amber, not red: the socket auto-retries (a few times with backoff)
// before declaring failure, so a normal blip reads as recovering — red is
// reserved for "gave up, needs your action".
const CONN: Record<ConnState, { dot: string; label: string }> = {
  open: { dot: "bg-green-500", label: "Connected" },
  connecting: { dot: "bg-amber-500", label: "Connecting" },
  closed: { dot: "bg-amber-500", label: "Reconnecting…" },
  failed: { dot: "bg-red-500", label: "Connection failed" },
}

// A passive status dot. The connection state comes straight from the global
// store (no prop threading); the label rides an aria-label plus a hover tooltip.
// Callers position it (e.g. as a ring-separated badge on the logo corner) via
// `className`.
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
