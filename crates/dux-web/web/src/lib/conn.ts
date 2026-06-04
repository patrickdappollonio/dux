import type { ConnState } from "@/lib/types"

// Connection-state badge mapping shared by the desktop inset header and the
// mobile home top bar so both surfaces label the socket identically.
export const CONN_BADGE: Record<
  ConnState,
  { variant: "default" | "secondary" | "outline"; label: string }
> = {
  open: { variant: "default", label: "Connected" },
  connecting: { variant: "secondary", label: "Connecting" },
  closed: { variant: "outline", label: "Offline" },
  failed: { variant: "outline", label: "Disconnected" },
}
