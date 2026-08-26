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
// off.
//
// THERE IS NO GIVE-UP COPY ANY MORE. This used to switch to "dux is unreachable"
// once the retry budget was spent, and the budget is gone: reconnecting is
// indefinite while the page is visible, and parked (not abandoned) while it is
// hidden. Saying the server is unreachable while the app is in fact still trying
// every few seconds was telling the user to act on a state they were not in. The
// copy says what is true instead, and names the two things that are usually
// wrong, so a user who can act still knows what to check.
//
// The Retry button stays, and it is now a BACKOFF RESET rather than a rescue:
// `reconnect()` stops waiting out the current gap and attempts immediately.
//
// Rendered through a body portal at a high z-index so it sits above every pane,
// dialog, and toast. Its `backdrop-grayscale` desaturates the whole app behind
// it — the running UI stays visible but drains to black-and-white, leaving the
// full-color modal as the only live thing on screen. `bg-background/40` adds a
// light dim without hiding the grayscaled app the user asked to keep in view.
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
