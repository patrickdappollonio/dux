import { Component, type ReactNode } from "react"

import { Button } from "@/components/ui/button"

// dux embeds a content-hashed JS bundle in its Rust binary, so a tab left open
// across a server rebuild still references the old hashed chunk URLs: a lazy
// import then 404s and, with no error boundary, the whole React tree unmounts
// into a white screen. This boundary catches that rejection.
//
// The first error attempts one automatic reload to pick up the new bundle,
// guarded so a genuinely broken page cannot loop: an auto-reload only happens
// when none did in the last RELOAD_WINDOW_MS. Otherwise the card asks the user.
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
