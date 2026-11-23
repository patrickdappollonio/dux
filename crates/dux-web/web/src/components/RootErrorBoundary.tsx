import { Component, type ErrorInfo, type ReactNode } from "react"
import { RotateCwIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { CopyDetails } from "@/components/CopyDetails"
import { messageOf, type Caught } from "@/lib/errorText"

// The last-resort boundary around everything main.tsx renders. Without it a
// render exception outside the inner ChunkBoundary areas unmounts the whole
// tree into a blank page. Unlike ChunkBoundary it never reloads by itself: the
// only automatic reload in the app is the one for a new server run, so here
// the reload is a button the user presses. It raises no toast either, because
// the toast host may be part of what crashed.

type State = { caught: Caught | null }

export class RootErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { caught: null }

  static getDerivedStateFromError(error: unknown): State {
    return { caught: { error, componentStack: "" } }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("dux's page stopped drawing after an error", error, info.componentStack)
    this.setState({ caught: { error, componentStack: info.componentStack ?? "" } })
  }

  render() {
    if (!this.state.caught) return this.props.children
    return <ErrorScreen caught={this.state.caught} />
  }
}

function ErrorScreen({ caught }: { caught: Caught }) {
  return (
    <main className="flex min-h-svh items-center justify-center bg-background px-4 py-10 text-foreground">
      <div className="flex w-full max-w-xl flex-col gap-4">
        <h1 className="text-lg font-semibold">This page hit an error</h1>
        <p className="text-sm text-muted-foreground">
          dux's page hit an error and stopped drawing. Your agents and terminals keep
          running on the server; reloading the page brings them back.
        </p>
        <pre className="max-h-60 overflow-auto rounded-lg border bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-words text-foreground">
          {messageOf(caught.error)}
        </pre>
        <div className="flex flex-wrap items-center gap-2">
          {/* Filled: reloading is the screen's one primary act. */}
          <Button className="h-10" onClick={() => location.reload()}>
            <RotateCwIcon />
            Reload page
          </Button>
          <CopyDetails caught={caught} />
        </div>
      </div>
    </main>
  )
}
