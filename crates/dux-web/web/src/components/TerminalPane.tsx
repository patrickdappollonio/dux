import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { Maximize2, Minimize2 } from "lucide-react"
import { AccessoryBar } from "@/components/AccessoryBar"
import { Button } from "@/components/ui/button"
import { useIsMobile } from "@/hooks/use-mobile"
import { applyModifiers, arrowSeq, ESC, TAB } from "@/lib/termkeys"
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
  // The padded, background-painted host. Padding must live HERE — one layer
  // OUTSIDE the element xterm opens into — because FitAddon measures the open
  // target's parent via getComputedStyle().height, which under Tailwind's
  // global box-sizing: border-box INCLUDES padding. Padding on the measured
  // element inflates availableHeight by 16px and mints a phantom terminal row
  // (~16 of every 17 window heights) that renders clipped under the status
  // bar — and the PTY is told about it, so bottom-anchored TUIs (codex's
  // input box) draw into an invisible row.
  const hostRef = useRef<HTMLDivElement>(null)
  // The unpadded element xterm opens into; its border-box equals its content
  // box, so FitAddon's measurement is exact.
  const containerRef = useRef<HTMLDivElement>(null)
  const wrapperRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const isMobile = useIsMobile()

  // Sticky (one-shot latched) soft-keyboard modifiers for the mobile accessory
  // bar. The state drives the latch's visual highlight; the ref mirrors it so
  // the value is readable inside the stable `onData` closure (which is created
  // once per [kind, id] and would otherwise capture a stale `ctrl`/`alt`).
  // `setMods` writes BOTH together, so they never diverge. This is the
  // ref-mirror approach — no setState-in-effect — chosen over a render-tick
  // split because the byte path must see the latch synchronously on the very
  // next keystroke, and the latch must clear the instant it's consumed.
  const [ctrl, setCtrl] = useState(false)
  const [alt, setAlt] = useState(false)
  const modsRef = useRef({ ctrl: false, alt: false })
  function setMods(next: { ctrl: boolean; alt: boolean }) {
    modsRef.current = next
    setCtrl(next.ctrl)
    setAlt(next.alt)
  }

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
    const host = hostRef.current
    const container = containerRef.current
    if (!host || !container) return

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

    // Apply the resolved bg to the padded host so the padding area matches the
    // canvas, making the padding feel like it belongs to the terminal rather
    // than being an external border.
    host.style.background = resolvedBg

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
    // xterm already sets autocorrect/autocapitalize="off" and spellcheck="false"
    // on its hidden input, but leaves autocomplete unset; turn it off too so the
    // mobile keyboard never injects autofill/suggestions into the PTY stream.
    if (term.textarea) {
      term.textarea.setAttribute("autocomplete", "off")
    }
    fit.fit()
    termRef.current = term

    // Feed live PTY bytes into the terminal.
    socket.onPtyBytes = (bytes) => term.write(bytes)

    // Forward keystrokes to the PTY as binary. On mobile, sticky modifiers from
    // the accessory bar transform a single typed char (Ctrl-chord, Alt/Meta
    // prefix) before sending; the latch then clears one-shot (visual included).
    // Multi-char chunks (paste/IME) pass through untransformed but still clear
    // any latch. `modsRef` is read live so this once-created closure sees the
    // current latch rather than a stale capture.
    const encoder = new TextEncoder()
    const dataSub = term.onData((s) => {
      const mods = modsRef.current
      const out =
        mods.ctrl || mods.alt ? applyModifiers(s, mods) : s
      if (mods.ctrl || mods.alt) {
        setMods({ ctrl: false, alt: false })
      }
      socket.sendInput(encoder.encode(out))
    })

    // Subscribe to the selected target's PTY. The server tracks the currently
    // subscribed id (agent session OR terminal) and routes input/resize to it,
    // so the rest of the wiring is identical for both kinds.
    if (kind === "terminal") {
      socket.subscribeTerminal(id)
    } else {
      socket.subscribe(id)
    }

    // Sizing has two halves with very different costs:
    //  - LOCAL refits (fit.fit()) are cheap, so the canvas tracks the container
    //    every frame while the user drags a divider or the window edge.
    //  - PTY resizes are expensive: each one is a SIGWINCH that makes the child
    //    TUI fully redraw. Sending them per-frame during a drag is the resize
    //    jitter. So the send is DEBOUNCED — one resize with the final
    //    dimensions once the drag settles — and deduplicated, since
    //    ResizeObserver also fires an initial callback on observe.
    let lastRows = 0
    let lastCols = 0
    let fitFrame = 0
    let sendTimer: ReturnType<typeof setTimeout> | undefined
    const sendSize = () => {
      if (term.rows !== lastRows || term.cols !== lastCols) {
        lastRows = term.rows
        lastCols = term.cols
        socket.resize(id, term.rows, term.cols)
      }
    }
    // Attach: fit and report immediately so the launch/repaint uses real dims.
    fit.fit()
    sendSize()

    // (Known edge: background tabs throttle rAF but not timers, so a resize
    // while hidden can send pre-fit dims; the next foreground tick corrects it.)
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(fitFrame)
      fitFrame = requestAnimationFrame(() => fit.fit())
      clearTimeout(sendTimer)
      sendTimer = setTimeout(sendSize, 200)
    })
    ro.observe(container)

    return () => {
      cancelAnimationFrame(fitFrame)
      clearTimeout(sendTimer)
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

  // Accessory-bar key sends. Esc/Tab/arrows are full sequences, not single
  // chars, so they bypass `applyModifiers` (which only transforms single-char
  // input). We still honor a latched Alt by prefixing ESC, and we clear any
  // latch one-shot afterward — Ctrl on a non-char key has no meaning here, so
  // it's simply consumed. Sends go through the same socket path as typed input.
  const encoder = new TextEncoder()
  function sendSeq(seq: string) {
    const mods = modsRef.current
    const out = mods.alt ? ESC + seq : seq
    if (mods.ctrl || mods.alt) {
      setMods({ ctrl: false, alt: false })
    }
    socket.sendInput(encoder.encode(out))
    termRef.current?.focus()
  }

  function onArrow(dir: "up" | "down" | "left" | "right") {
    const app = termRef.current?.modes.applicationCursorKeysMode ?? false
    sendSeq(arrowSeq(dir, app))
  }

  function toggleCtrl() {
    setMods({ ctrl: !modsRef.current.ctrl, alt: modsRef.current.alt })
    termRef.current?.focus()
  }

  function toggleAlt() {
    setMods({ ctrl: modsRef.current.ctrl, alt: !modsRef.current.alt })
    termRef.current?.focus()
  }

  // The host div owns the padding so the resolved bg fills the padding area
  // seamlessly — no external "border" look. FitAddon measures the content box.
  // The wrapper is `relative` so the readiness spinner can overlay the host
  // until the PTY emits its first output (latched via `everReady`). On mobile it
  // becomes the flex-1 child of a column root so the accessory bar can sit
  // beneath it; on desktop it stays the lone full-size element.
  //
  // overflow-hidden: the pane is its own clip boundary. Between a container
  // resize and the next-rAF refit, xterm still holds its previous (possibly
  // larger) size; if that one-frame overflow escapes to a scrollable ancestor
  // it flashes scrollbars and oscillates the layout (scrollbar shrinks the box
  // → ResizeObserver → refit → scrollbar gone → grow → repeat). Clipping at
  // the pane covers every host: the desktop ResizablePanel, the mobile
  // viewport-pinned root, and fullscreen. The overlays (fullscreen button,
  // readiness card) are absolutely positioned inside these bounds, so
  // clipping never affects them.
  const pane = (
    <div
      ref={wrapperRef}
      className={
        isMobile
          ? "group relative min-h-0 w-full flex-1 overflow-hidden bg-background"
          : "group relative h-full w-full overflow-hidden bg-background"
      }
    >
      {/* Padding lives on the host, NOT the measured element below — see the
          hostRef comment: border-box computed heights include padding, and
          FitAddon would mint a phantom row/column from it. */}
      <div ref={hostRef} className="h-full w-full p-2">
        <div ref={containerRef} className="h-full w-full" />
      </div>
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

  // Desktop: render the pane exactly as before — no extra wrapper, no bar.
  if (!isMobile) return pane

  // Mobile: a column root so the terminal host (flex-1 min-h-0) and the
  // accessory bar (shrink-0) stack. The ResizeObserver on the host already
  // refits + debounce-resizes the PTY when this column reflows, so no extra
  // resize wiring is needed.
  return (
    <div className="flex h-full w-full flex-col">
      {pane}
      <AccessoryBar
        onEsc={() => sendSeq(ESC)}
        onTab={() => sendSeq(TAB)}
        onArrow={onArrow}
        ctrl={ctrl}
        alt={alt}
        onToggleCtrl={toggleCtrl}
        onToggleAlt={toggleAlt}
      />
    </div>
  )
}
