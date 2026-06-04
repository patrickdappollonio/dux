import { Component, type ReactNode } from "react"

import { Button } from "@/components/ui/button"

// Why this exists: dux embeds a CONTENT-HASHED JS bundle in its Rust binary.
// When the server is rebuilt and restarted while a tab is still open, that
// stale tab's index.html still references the OLD hashed chunk URLs. Opening a
// terminal then fires import() for a chunk that no longer exists; the server
// 404s it, React.lazy rejects, and — with no error boundary — the entire React
// tree unmounts into a white screen. This boundary catches that rejection.
//
// On the first error we attempt ONE automatic reload to pick up the new
// bundle. The guard below prevents a reload loop if the page is genuinely
// broken (e.g. the new bundle also fails): we only auto-reload when no reload
// happened in the last RELOAD_WINDOW_MS. Otherwise we render a small branded
// card asking the user to reload manually.
const RELOAD_KEY = "dux-chunk-reload"
const RELOAD_WINDOW_MS = 30_000

// Returns true and records the attempt if we may auto-reload now; returns false
// if we reloaded recently (so we should show the manual fallback instead).
function tryClaimReload(): boolean {
  try {
    const last = Number(sessionStorage.getItem(RELOAD_KEY) ?? 0)
    if (Date.now() - last < RELOAD_WINDOW_MS) return false
    sessionStorage.setItem(RELOAD_KEY, String(Date.now()))
    return true
  } catch {
    // sessionStorage unavailable (private mode quirks): don't risk a loop.
    return false
  }
}

type State = { failed: boolean }

export class ChunkBoundary extends Component<
  { children: ReactNode },
  State
> {
  state: State = { failed: false }

  static getDerivedStateFromError(): State {
    return { failed: true }
  }

  componentDidCatch() {
    // Any caught error here is almost certainly a failed dynamic import after a
    // redeploy. Auto-reload once; if we already did recently, fall through to
    // the manual card rendered below.
    if (tryClaimReload()) location.reload()
  }

  render() {
    if (!this.state.failed) return this.props.children
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-4 p-6 text-center">
        <div className="max-w-sm rounded-lg border bg-card p-6 text-card-foreground shadow-sm">
          <p className="text-sm font-medium">dux needs a reload</p>
          <p className="mt-2 text-sm text-muted-foreground">
            The dux server likely restarted with a new build, so this tab is out
            of date. Reload to pick up the latest version.
          </p>
          <Button className="mt-4" onClick={() => location.reload()}>
            Reload
          </Button>
        </div>
      </div>
    )
  }
}
