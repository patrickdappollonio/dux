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

    const term = new Terminal({
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      convertEol: false,
      theme: { background: "#0b0d12" },
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
