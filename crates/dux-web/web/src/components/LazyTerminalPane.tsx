import { lazy } from "react"

// TerminalPane pulls xterm in, which only matters once a terminal is opened, so it
// is fetched as an async chunk. Every render site imports THIS, so they share one
// chunk and one component identity.
export const LazyTerminalPane = lazy(() =>
  import("@/components/TerminalPane").then((m) => ({ default: m.TerminalPane })),
)

// The same chunk's stack, for an agent laid out with its terminal dock.
export const LazyTerminalStack = lazy(() =>
  import("@/components/TerminalPane").then((m) => ({ default: m.TerminalStack })),
)
