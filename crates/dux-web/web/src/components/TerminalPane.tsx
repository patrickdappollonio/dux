import { useEffect, useRef } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { socket } from "@/lib/store"

interface TerminalPaneProps {
  sessionId: string
}

export function TerminalPane({ sessionId }: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null)

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

    // Feed live PTY bytes into the terminal.
    socket.onPtyBytes = (bytes) => term.write(bytes)

    // Forward keystrokes to the PTY as binary.
    const encoder = new TextEncoder()
    const dataSub = term.onData((s) => socket.sendInput(encoder.encode(s)))

    // Subscribe to this session's PTY, then send the initial size.
    socket.subscribe(sessionId)
    socket.resize(sessionId, term.rows, term.cols)

    // Refit + report size on container resize.
    const ro = new ResizeObserver(() => {
      fit.fit()
      socket.resize(sessionId, term.rows, term.cols)
    })
    ro.observe(container)

    return () => {
      ro.disconnect()
      dataSub.dispose()
      socket.onPtyBytes = () => {}
      term.dispose()
    }
  }, [sessionId])

  return <div ref={containerRef} className="h-full w-full" />
}
