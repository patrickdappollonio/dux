import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { Maximize2, Minimize2 } from "lucide-react"
import { AccessoryBar } from "@/components/AccessoryBar"
import type { ScrollDir } from "@/components/AccessoryBar"
import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { useIsMobile } from "@/hooks/use-mobile"
import { applyModifiers, arrowSeq, ESC, TAB } from "@/lib/termkeys"
import { selectSession, socket, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
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
  // The element handed to the Fullscreen API. On desktop it is the pane itself;
  // on mobile it is the OUTER column (pane + accessory bar) so the bar stays
  // visible in fullscreen — fullscreening the pane alone would crop it out.
  const fullscreenRef = useRef<HTMLDivElement>(null)
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
  // Size xterm's scrollback to the configured `agent_scrollback_lines` so the
  // reconnect repaint's replayed history isn't trimmed by xterm's 1000-line
  // default. Read via a ref (not an effect dep) so a ViewModel change never
  // recreates the terminal; the fallback matches the core default and only
  // applies before the first ViewModel arrives.
  const scrollbackRef = useRef(
    viewModel?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES
  )
  scrollbackRef.current =
    viewModel?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES
  const session =
    kind === "agent"
      ? viewModel?.sessions.find((s) => s.id === id)
      : viewModel?.sessions.find((s) => s.terminals.some((t) => t.id === id))
  const hasOutput =
    kind === "agent"
      ? (session?.has_output ?? false)
      : (session?.terminals.find((t) => t.id === id)?.has_output ?? false)
  const providerName = session?.provider
  // The macro popover's target. For an agent the streamed id IS the session id;
  // for a terminal it is the terminal id, and the owning session id comes from
  // the resolved `session` (falls back to the prop, though a focused terminal
  // always resolves a session). Mirrors the store's `SelectedTarget` shape so
  // the popover filters macros by the focused surface and runs against the
  // right PTY.
  const macroTarget: SelectedTarget =
    kind === "agent"
      ? { kind: "agent", sessionId: id }
      : { kind: "terminal", terminalId: id, sessionId: session?.id ?? id }
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

    // xterm 6 draws a custom DOM scrollbar whose width is the `overviewRuler.width`
    // option (default 14). Drive it from the SAME `--xterm-scrollbar-width` CSS
    // var the button overlay reserves its gutter from, so the slimmed scrollbar
    // and the reserved space always agree (single source). Setting the option
    // also instantiates an overview-ruler canvas; index.css hides it (dux uses no
    // decorations, so it's always empty).
    const scrollbarWidth =
      parseInt(
        getComputedStyle(document.documentElement).getPropertyValue(
          "--xterm-scrollbar-width"
        ),
        10
      ) || 8

    const term = new Terminal({
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 14,
      cursorBlink: true,
      convertEol: false,
      scrollback: scrollbackRef.current,
      overviewRuler: { width: scrollbarWidth },
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

    // Focus the terminal on selection so the user can type immediately — no extra
    // click into the pane. This effect re-runs (and the pane remounts) on every
    // agent OR companion-terminal selection (keyed by [kind, id]), so both cases
    // are covered. Runs after the click that selected the row, so it wins focus.
    term.focus()

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

    // First-load redraw. The immediate fit above can read a STALE size when the
    // pane isn't at its final dimensions synchronously on mount — e.g. after a
    // mobile→desktop layout switch, opening an agent rendered it at the old
    // (mobile) width until the user nudged a divider. The ResizeObserver only
    // re-sends on an actual size CHANGE, so it doesn't self-correct here. Re-fit
    // once the layout has settled and report unconditionally, so the child gets a
    // SIGWINCH and redraws at the true size on its own. A same-size resize is a
    // kernel no-op (no SIGWINCH), so this never triggers a spurious redraw.
    const redrawTimer = setTimeout(() => {
      fit.fit()
      lastRows = term.rows
      lastCols = term.cols
      socket.resize(id, term.rows, term.cols)
    }, 50)

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
      clearTimeout(redrawTimer)
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
    let scrollTimer: ReturnType<typeof setTimeout> | undefined
    function handleFullscreenChange() {
      const active = document.fullscreenElement === fullscreenRef.current
      setIsFullscreen(active)
      if (!active) {
        unlockKeyboard()
      }
      // Entering or leaving fullscreen reflows the pane to a new size; xterm
      // does not re-anchor to the latest output on its own, so the agent's input
      // line can end up scrolled off-screen. Pull the viewport back to the
      // bottom once the resize has settled so the prompt is visible without the
      // user having to fight the slim mobile scrollbar. Tracked + cleared so a
      // rapid toggle (or an unmount) can't leave a stale timer firing.
      clearTimeout(scrollTimer)
      scrollTimer = setTimeout(() => termRef.current?.scrollToBottom(), 50)
    }
    document.addEventListener("fullscreenchange", handleFullscreenChange)
    return () => {
      clearTimeout(scrollTimer)
      document.removeEventListener("fullscreenchange", handleFullscreenChange)
      unlockKeyboard()
    }
  }, [])

  async function toggleFullscreen() {
    const target = fullscreenRef.current
    if (!target) return
    if (document.fullscreenElement === target) {
      await document.exitFullscreen().catch(() => {})
    } else {
      try {
        await target.requestFullscreen()
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

  // Scroll the xterm viewport from the accessory bar's second row. These drive
  // xterm's own scrollback (the normal-buffer history that accumulates as the
  // agent streams output), giving a reliable touch target the slim scrollbar
  // can't. preventDefault on the buttons keeps the soft keyboard up, so there's
  // no need to refocus here. (Alt-screen TUIs keep no scrollback, so page/jump
  // scrolling is a no-op there — the cursor-arrow row drives those instead.)
  function onScroll(dir: ScrollDir) {
    const term = termRef.current
    if (!term) return
    switch (dir) {
      case "top":
        term.scrollToTop()
        break
      case "bottom":
        term.scrollToBottom()
        break
      case "pageUp":
        term.scrollPages(-1)
        break
      case "pageDown":
        term.scrollPages(1)
        break
    }
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
      // On desktop the pane IS the fullscreen target; on mobile the outer column
      // below owns that ref so the accessory bar is included in fullscreen.
      // Crossing the mobile breakpoint swaps the whole app subtree (App.tsx), so
      // this instance never sees isMobile flip mid-life — the ref never churns.
      ref={isMobile ? null : fullscreenRef}
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
      {/* Pane chrome buttons. Grouped in ONE absolutely-positioned overlay (a
          sibling of the xterm host, NOT inside the unpadded containerRef xterm
          opens into) so they never change the terminal's box measurement — see
          the hostRef comment. Both buttons carry a visible icon + text label and
          stay always visible (no hover reveal) so they read at a glance on every
          surface, touch included. Widening them is layout-safe: the overlay is a
          positioned sibling of the measured host, so the terminal box is
          unaffected. The right offset reserves the xterm scrollbar gutter so the
          buttons never overlap the scrollbar: 0.5rem MUST match the host's `p-2`
          padding below, then the shared --xterm-scrollbar-width (fallback keeps
          the offset valid if the var is ever missing), then a small gap. */}
      <div className="absolute top-3 right-[calc(0.5rem+var(--xterm-scrollbar-width,8px)+0.25rem)] z-10 flex gap-2">
        {/* The popover trigger renders a secondary labeled Button (see
            MacroPopover); it must remain reachable on touch, so it does not
            hide on blur. On close we hand Base UI the terminal's textarea as the
            focus target instead of calling termRef.focus() imperatively like the
            accessory-bar handlers do, because Base UI owns focus during a
            popover close — see the MacroPopover finalFocus comment. */}
        <MacroPopover
          target={macroTarget}
          finalFocus={() => termRef.current?.textarea ?? null}
        />
        {/* Fullscreen toggle: embedded mode already forwards every key the
            browser will give a page; fullscreen + keyboard lock additionally
            captures reserved shortcuts (Ctrl+T, Ctrl+W, …) on Chromium. The
            label is the affordance; the tooltip carries the non-obvious
            keyboard-lock behavior that the label alone cannot. */}
        <SimpleTooltip
          content={
            isFullscreen
              ? "Exit fullscreen — holding Esc also exits"
              : "Fullscreen — captures browser-reserved shortcuts like Ctrl+T"
          }
        >
          <Button
            variant="secondary"
            onClick={() => void toggleFullscreen()}
            aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
          >
            {isFullscreen ? <Minimize2 /> : <Maximize2 />}
            {isFullscreen ? "Exit fullscreen" : "Fullscreen"}
          </Button>
        </SimpleTooltip>
      </div>
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
    <div
      // The mobile fullscreen target: the column wraps the pane AND the bar so
      // both fill the screen in fullscreen. bg-background paints the fullscreen
      // backdrop so it matches the theme rather than going black. In fullscreen
      // this column escapes the safe-area-padded mobile root (App.tsx) into the
      // fullscreen layer, so it must clear the notch / home indicator / rounded
      // corners itself; in normal layout the root handles it, so no inset here.
      ref={fullscreenRef}
      className="flex h-full w-full flex-col bg-background"
      style={
        isFullscreen
          ? {
              paddingTop: "env(safe-area-inset-top)",
              paddingBottom: "env(safe-area-inset-bottom)",
              paddingLeft: "env(safe-area-inset-left)",
              paddingRight: "env(safe-area-inset-right)",
            }
          : undefined
      }
    >
      {pane}
      <AccessoryBar
        onEsc={() => sendSeq(ESC)}
        onTab={() => sendSeq(TAB)}
        onArrow={onArrow}
        onScroll={onScroll}
        ctrl={ctrl}
        alt={alt}
        onToggleCtrl={toggleCtrl}
        onToggleAlt={toggleAlt}
      />
    </div>
  )
}
