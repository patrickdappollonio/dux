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

// The app-wide "the events socket is down" modal. It mirrors the installed-PWA
// offline page (`public/offline.html`) but lives inside the running SPA: when the
// connection drops mid-session there is no navigation for the service worker to
// intercept, so this React surface stands in for it.
//
// Driven by the sticky `offline` flag (see `store.ts`), NOT the raw `conn`, so a
// reconnect attempt re-entering `connecting` between drops does not flicker it
// off. While the socket is still auto-retrying it reads "Reconnecting…"; once the
// retries are exhausted (`conn === "failed"`) it switches to the "unreachable"
// give-up copy. Either way the button calls `reconnect()` (which resets the
// attempt budget and reopens), so the user can always force a fresh attempt.
//
// Rendered through a body portal at a high z-index so it sits above every pane,
// dialog, and toast. Its `backdrop-grayscale` desaturates the whole app behind
// it — the running UI stays visible but drains to black-and-white, leaving the
// full-color modal as the only live thing on screen. `bg-background/40` adds a
// light dim without hiding the grayscaled app the user asked to keep in view.
export function OfflineOverlay() {
  const { offline, conn } = useDux()
  if (!offline) return null

  // `failed` means the socket gave up after its retry budget; anything else while
  // offline is an in-progress reconnect.
  const gaveUp = conn === "failed"

  return createPortal(
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="offline-overlay-title"
      aria-describedby="offline-overlay-desc"
      className="fixed inset-0 z-[100] flex items-center justify-center bg-background/40 p-6 backdrop-grayscale"
    >
      <div className="w-full max-w-md rounded-xl border bg-card p-6 text-center text-card-foreground shadow-xl">
        <pre
          aria-hidden
          className="mx-auto mb-6 inline-block text-left font-mono text-[11px] leading-[1.15] text-muted-foreground"
        >
          {DUX_ART}
        </pre>
        <h1
          id="offline-overlay-title"
          className="mb-1.5 flex items-center justify-center gap-2 text-lg font-semibold"
        >
          {!gaveUp ? (
            <Loader2
              className="size-4 animate-spin text-muted-foreground"
              aria-hidden
            />
          ) : null}
          {gaveUp ? "dux is unreachable" : "Reconnecting to dux…"}
        </h1>
        <p
          id="offline-overlay-desc"
          className="mb-6 text-sm leading-relaxed text-muted-foreground"
        >
          {gaveUp
            ? "The dux server may be down, or this device may be offline. Reconnect to the network or restart the server, then try again."
            : "The connection to the dux server dropped. Trying to get you back online…"}
        </p>
        <Button onClick={reconnect}>
          <RefreshCw aria-hidden />
          {gaveUp ? "Retry" : "Reconnect now"}
        </Button>
      </div>
    </div>,
    document.body,
  )
}
