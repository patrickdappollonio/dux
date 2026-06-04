import { lazy } from "react"

// TerminalPane pulls in @xterm/xterm + @xterm/addon-fit (~400KB), which only
// matters once a terminal is actually opened. Lazy-loading it moves all of that
// into an async chunk fetched on first terminal open, keeping the initial bundle
// small. Both render sites (desktop TerminalArea, mobile TerminalScreen) import
// this single lazy component so they share one async chunk and one identity.
export const LazyTerminalPane = lazy(() =>
  import("@/components/TerminalPane").then((m) => ({ default: m.TerminalPane })),
)
