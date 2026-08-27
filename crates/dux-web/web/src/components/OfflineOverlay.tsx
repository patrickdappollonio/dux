import { Loader2, RefreshCw } from "lucide-react"
import { createPortal } from "react-dom"

import { Button } from "@/components/ui/button"
import { reconnect, useDux } from "@/lib/store"

// The same ASCII "dux" wordmark the standalone PWA offline page (`public/
// offline.html`) shows, so the in-app modal and the service-worker page read as
// one experience. Left-aligned inside an inline-block so the body's centering
// places the art as a block without shearing each line independently.
const DUX_ART = `       ░██
       ░██
 ░████████ ░██    ░██ ░██    ░██
░██    ░██ ░██    ░██  ░██  ░██
░██    ░██ ░██    ░██   ░█████
░██   ░███ ░██   ░███  ░██  ░██
 ░█████░██  ░█████░██ ░██    ░██ `

// The sticky offline state avoids flicker between retries. Reconnection is
// indefinite while visible and parked while hidden; Retry resets the backoff.
export function OfflineOverlay() {
  const { offline } = useDux()
  if (!offline) return null

  return createPortal(
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="offline-overlay-title"
      aria-describedby="offline-overlay-desc"
      className="fixed inset-0 z-[100] flex items-center justify-center bg-background/40 p-6 backdrop-grayscale supports-backdrop-filter:backdrop-blur-sm"
    >
      <div className="w-full max-w-md rounded-xl border bg-card p-6 text-center text-card-foreground shadow-xl">
        <pre
          aria-hidden
          className="mx-auto mb-6 inline-block text-left font-blocks text-[11px] leading-[1.15] text-muted-foreground"
        >
          {DUX_ART}
        </pre>
        <h1
          id="offline-overlay-title"
          className="mb-1.5 flex items-center justify-center gap-2 text-lg font-semibold"
        >
          <Loader2
            className="size-4 animate-spin text-muted-foreground"
            aria-hidden
          />
          Reconnecting to dux…
        </h1>
        <p
          id="offline-overlay-desc"
          className="mb-6 text-sm leading-relaxed text-muted-foreground"
        >
          The connection to the dux server dropped, and dux keeps trying for as
          long as this page is open. If it does not come back, the server may be
          down or this device may be offline.
        </p>
        <Button onClick={reconnect}>
          <RefreshCw aria-hidden />
          Reconnect now
        </Button>
      </div>
    </div>,
    document.body,
  )
}
