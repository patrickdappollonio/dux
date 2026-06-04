import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { Maximize2, Minimize2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { selectSession, socket, useDux } from "@/lib/store"
import { BrailleSpinner } from "@/components/BrailleSpinner"

interface TerminalPaneProps {
  // The streamed target: an agent session or one of its companion terminals.
  // `id` is the session id for an agent and the terminal id for a terminal.
  kind: "agent" | "terminal"
  id: string
}

// The Keyboard Lock API (Chromium-only): while the pane is fullscreen it lets
// the page receive browser-reserved shortcuts like Ctrl+T / Ctrl+W so they
// reach the agent instead of opening tabs. Elsewhere these helpers no-op and
// fullscreen still works — just without the reserved keys.
type KeyboardLockNavigator = Navigator & {
  keyboard?: {
    lock?: (keys?: string[]) => Promise<void>
    unlock?: () => void
  }
}

function lockKeyboard(): void {
  const keyboard = (navigator as KeyboardLockNavigator).keyboard
  void keyboard?.lock?.().catch(() => {})
}

function unlockKeyboard(): void {
  const keyboard = (navigator as KeyboardLockNavigator).keyboard
  keyboard?.unlock?.()
}

export function TerminalPane({ kind, id }: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const wrapperRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const [isFullscreen, setIsFullscreen] = useState(false)

  const { viewModel } = useDux()
  const session =
    kind === "agent"
      ? viewModel?.sessions.find((s) => s.id === id)
      : viewModel?.sessions.find((s) => s.terminals.some((t) => t.id === id))
  const hasOutput =
    kind === "agent"
      ? (session?.has_output ?? false)
      : (session?.terminals.find((t) => t.id === id)?.has_output ?? false)
  const providerName = session?.provider
  // Latch readiness: once the PTY has emitted output we keep the spinner hidden,
  // even if a later view model reports `has_output: false` (e.g. an exited
  // agent). Adjusting state during render is the React-sanctioned latch pattern
  // — the guard makes it run at most once, so it can't cascade.
  const [everReady, setEverReady] = useState(false)
  if (hasOutput && !everReady) {
    setEverReady(true)
  }

  // Mirror the TUI's exit behavior: when the agent we were attached to stops
  // running (it produced output in this pane, then its session left `active`
  // — the exit prune marks it detached), reset the center pane back to the
  // welcome screen. The "Agent exited" toast explains why. A fresh selection
  // of the detached agent remounts this pane and relaunches it.
  const sessionStatus = session?.status
  useEffect(() => {
    if (kind === "agent" && everReady && sessionStatus && sessionStatus !== "active") {
      selectSession(null)
    }
  }, [kind, everReady, sessionStatus])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    // Resolve the app's background token so the terminal canvas matches the
    // shadcn palette rather than using a hardcoded hex color.
    const rawBg = getComputedStyle(document.documentElement)
      .getPropertyValue("--background")
      .trim()
    // The CSS variable is an oklch / hsl value; xterm expects a hex string.
    // Resolve it by painting a 1×1 canvas with the variable.
    let resolvedBg = "#000000"
    try {
      const canvas = document.createElement("canvas")
      canvas.width = 1
      canvas.height = 1
      const ctx = canvas.getContext("2d")
      if (ctx && rawBg) {
        ctx.fillStyle = `oklch(${rawBg})`
        ctx.fillRect(0, 0, 1, 1)
        const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
        resolvedBg = `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${b.toString(16).padStart(2, "0")}`
      }
    } catch {
      // Fallback silently — resolvedBg stays black.
    }

    // Apply the resolved bg to the host so the padding area matches the canvas,
    // making the padding feel like it belongs to the terminal rather than being
    // an external border.
    container.style.background = resolvedBg

    const term = new Terminal({
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      convertEol: false,
      theme: { background: resolvedBg },
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(container)
    fit.fit()
    termRef.current = term

    // Feed live PTY bytes into the terminal.
    socket.onPtyBytes = (bytes) => term.write(bytes)

    // Forward keystrokes to the PTY as binary.
    const encoder = new TextEncoder()
    const dataSub = term.onData((s) => socket.sendInput(encoder.encode(s)))

    // Subscribe to the selected target's PTY. The server tracks the currently
    // subscribed id (agent session OR terminal) and routes input/resize to it,
    // so the rest of the wiring is identical for both kinds.
    if (kind === "terminal") {
      socket.subscribeTerminal(id)
    } else {
      socket.subscribe(id)
    }

    // Fit + report the PTY size through ONE deduplicated path. Every resize we
    // send triggers a SIGWINCH redraw in the child, and ResizeObserver fires an
    // initial callback on observe — so the old unconditional fit-and-send here
    // plus the observer's first tick produced back-to-back redraws at attach,
    // visible as jitter. Only genuinely new dimensions are sent, and observer
    // bursts are coalesced to one measurement per frame.
    let lastRows = 0
    let lastCols = 0
    let fitFrame = 0
    const syncSize = () => {
      fit.fit()
      if (term.rows !== lastRows || term.cols !== lastCols) {
        lastRows = term.rows
        lastCols = term.cols
        socket.resize(id, term.rows, term.cols)
      }
    }
    syncSize()

    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(fitFrame)
      fitFrame = requestAnimationFrame(syncSize)
    })
    ro.observe(container)

    return () => {
      cancelAnimationFrame(fitFrame)
      ro.disconnect()
      dataSub.dispose()
      socket.onPtyBytes = () => {}
      termRef.current = null
      term.dispose()
    }
  }, [kind, id])

  // Track fullscreen state for this pane and release the keyboard lock the
  // moment fullscreen ends, however it ends (button, held Esc, tab switch).
  useEffect(() => {
    function handleFullscreenChange() {
      const active = document.fullscreenElement === wrapperRef.current
      setIsFullscreen(active)
      if (!active) {
        unlockKeyboard()
      }
    }
    document.addEventListener("fullscreenchange", handleFullscreenChange)
    return () => {
      document.removeEventListener("fullscreenchange", handleFullscreenChange)
      unlockKeyboard()
    }
  }, [])

  async function toggleFullscreen() {
    const wrapper = wrapperRef.current
    if (!wrapper) return
    if (document.fullscreenElement === wrapper) {
      await document.exitFullscreen().catch(() => {})
    } else {
      try {
        await wrapper.requestFullscreen()
        // Only meaningful while fullscreen; held-Esc exits, single Esc presses
        // flow to the agent.
        lockKeyboard()
      } catch {
        // Fullscreen request denied — leave the pane embedded.
      }
    }
    termRef.current?.focus()
  }

  // The host div owns the padding so the resolved bg fills the padding area
  // seamlessly — no external "border" look. FitAddon measures the content box.
  // The wrapper is `relative` so the readiness spinner can overlay the host
  // until the PTY emits its first output (latched via `everReady`).
  return (
    <div ref={wrapperRef} className="group relative h-full w-full bg-background">
      <div ref={containerRef} className="h-full w-full p-2" />
      {/* Fullscreen toggle: embedded mode already forwards every key the
          browser will give a page; fullscreen + keyboard lock additionally
          captures reserved shortcuts (Ctrl+T, Ctrl+W, …) on Chromium. */}
      <Button
        variant="secondary"
        size="icon"
        onClick={() => void toggleFullscreen()}
        title={
          isFullscreen
            ? "Exit fullscreen (hold Esc also works)"
            : "Fullscreen — captures browser-reserved shortcuts like Ctrl+T"
        }
        aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
        className="absolute top-3 right-3 z-10 opacity-0 transition-opacity focus-visible:opacity-100 group-hover:opacity-100"
      >
        {isFullscreen ? <Minimize2 /> : <Maximize2 />}
      </Button>
      {!everReady ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <BrailleSpinner className="text-primary" />
            <span className="text-sm text-muted-foreground">
              {kind === "agent"
                ? `Starting ${providerName ?? "agent"}…`
                : "Launching terminal…"}
            </span>
          </div>
        </div>
      ) : null}
    </div>
  )
}
