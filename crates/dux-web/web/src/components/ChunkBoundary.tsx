import { Component, type ReactNode } from "react"

import { Button } from "@/components/ui/button"
import { confirmedSameDux } from "@/lib/authGate"

// dux embeds a content-hashed JS bundle in its Rust binary, so a tab left open
// across a server rebuild still references the old hashed chunk URLs: a lazy
// import then 404s and, with no error boundary, the whole React tree unmounts
// into a white screen. This boundary catches that rejection. It also catches
// every other error thrown while drawing what it wraps, and shows that one as
// this pane's own error, with no reload of its own.
//
// A failed chunk attempts one automatic reload to pick up the new bundle,
// guarded so a genuinely broken page cannot loop: an auto-reload only happens
// when none did in the last RELOAD_WINDOW_MS. Otherwise the card asks the user.
// The chunk may also be missing because something other than this dux answers
// at the address now, so the reload waits until the status route confirms it
// is this dux: a reload must never load somebody else's page into the tab.
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

// What each browser, and Vite's preload of a chunk's stylesheet, says when a
// lazily loaded script or stylesheet did not arrive.
const CHUNK_LOAD_FAILURE =
  /Failed to fetch dynamically imported module|error loading dynamically imported module|Importing a module script failed|Unable to preload CSS/i

function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message || error.name
  return String(error)
}

function isChunkLoadFailure(error: unknown): boolean {
  return CHUNK_LOAD_FAILURE.test(messageOf(error))
}

type State = { caught: { error: unknown } | null }

export class ChunkBoundary extends Component<
  { children: ReactNode },
  State
> {
  state: State = { caught: null }

  static getDerivedStateFromError(error: unknown): State {
    return { caught: { error } }
  }

  componentDidCatch(error: unknown) {
    if (!isChunkLoadFailure(error)) return
    // Auto-reload once; if we already did recently, fall through to the manual
    // card rendered below.
    void confirmedSameDux().then((same) => {
      if (same && tryClaimReload()) location.reload()
    })
  }

  render() {
    const { caught } = this.state
    if (caught === null) return this.props.children
    const chunk = isChunkLoadFailure(caught.error)
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-4 p-6 text-center">
        <div className="max-w-sm rounded-lg border bg-card p-6 text-card-foreground shadow-sm">
          <p className="text-sm font-medium">
            {chunk ? "dux needs a reload" : "This pane hit an error"}
          </p>
          {chunk ? (
            <p className="mt-2 text-sm text-muted-foreground">
              The dux server likely restarted with a new build, so this tab is out
              of date. Reload to pick up the latest version.
            </p>
          ) : (
            <>
              <p className="mt-2 text-sm text-muted-foreground">
                This pane stopped drawing after an error. Your agents and terminals
                keep running on the server, and reloading the page brings the pane
                back. If it happens again, include the error below when you report it.
              </p>
              <pre className="mt-3 max-h-40 overflow-auto rounded-md border bg-muted p-2 text-left font-mono text-xs whitespace-pre-wrap break-words text-foreground">
                {messageOf(caught.error)}
              </pre>
            </>
          )}
          <Button className="mt-4 max-md:min-h-10 pointer-coarse:min-h-10" onClick={() => location.reload()}>
            Reload
          </Button>
        </div>
      </div>
    )
  }
}
