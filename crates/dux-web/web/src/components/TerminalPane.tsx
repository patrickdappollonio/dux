import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { Maximize2, Minimize2, MonitorSmartphone } from "lucide-react"
import { AccessoryBar } from "@/components/AccessoryBar"
import type { ScrollDir } from "@/components/AccessoryBar"
import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { dragScrollLines, keyboardLikelyOpen } from "@/lib/viewport"
import { applyModifiers, arrowSeq, ESC, TAB } from "@/lib/termkeys"
import { selectSession, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import {
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  terminalPtyUrl,
} from "@/lib/ptySocket"
import {
  isForeground,
  isOwnerAfterHandover,
  onPtyOwner,
} from "@/lib/ptyOwnership"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { suppressViewerReports } from "@/lib/suppressViewerReports"
import { BrailleSpinner } from "@/components/BrailleSpinner"

interface TerminalPaneProps {
  // The streamed target: an agent session or one of its companion terminals.
  // `id` is the session id for an agent and the terminal id for a terminal.
  kind: "agent" | "terminal"
  id: string
  // The owning session id. Equal to `id` for an agent; the parent session for a
  // companion terminal. Used to build the nested PTY socket URL and the macro
  // target, so it is passed explicitly (the spine may not yet list a just-created
  // terminal when this pane first mounts).
  sessionId: string
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

export function TerminalPane({ kind, id, sessionId }: TerminalPaneProps) {
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
  // The dedicated PTY socket for the focused target. Created in the wiring effect
  // and read by the accessory-bar key handlers (defined at component scope) so
  // they send stdin to the same socket xterm's `onData` does.
  const ptyRef = useRef<PtySocket | null>(null)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const isMobile = useIsMobile()
  // Visual-viewport height so a FULLSCREEN terminal can keep its content above
  // the soft keyboard. The non-fullscreen mobile layout is pinned to this by the
  // MobileApp root, but a fullscreen terminal escapes that root into the
  // fullscreen layer (sized to the full layout viewport by the browser), so it
  // has to track the keyboard itself — see the inner column in the mobile return.
  // null off-API; the hook runs on every platform but is only read on mobile.
  const viewportHeight = useVisualViewportHeight()

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

  const { spine, bootstrap } = useDux()
  // Size xterm's scrollback to the configured `agent_scrollback_lines` (now from
  // the bootstrap document) so the reconnect repaint's replayed history isn't
  // trimmed by xterm's 1000-line default. Read via a ref (not an effect dep) so
  // a bootstrap change never recreates the terminal; the fallback matches the
  // core default and only applies before the first bootstrap fetch lands.
  const scrollbackRef = useRef(
    bootstrap?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES
  )
  // Keep the ref current as the bootstrap document arrives or changes, without
  // writing it during render (React forbids ref writes in render) and without
  // making it a dependency of the terminal mount effect (which would recreate
  // the terminal). The terminal reads this ref lazily on (re)connect, so an
  // after-commit update lands in time for the first attach.
  useEffect(() => {
    scrollbackRef.current =
      bootstrap?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES
  }, [bootstrap?.agent_scrollback_lines])
  const session =
    kind === "agent"
      ? spine?.sessions.find((s) => s.id === id)
      : spine?.sessions.find((s) => s.terminals.some((t) => t.id === id))
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
      : { kind: "terminal", terminalId: id, sessionId }
  // Latch readiness: once the PTY has emitted output we keep the spinner hidden,
  // even if a later view model reports `has_output: false` (e.g. an exited
  // agent). Adjusting state during render is the React-sanctioned latch pattern
  // — the guard makes it run at most once, so it can't cascade.
  const [everReady, setEverReady] = useState(false)
  if (hasOutput && !everReady) {
    setEverReady(true)
  }
  // True while the PTY socket has dropped and is retrying (non-blocking). Drives a
  // "Reconnecting…" overlay that re-arms even after `everReady` has latched, so a
  // mid-session disconnect is visible rather than the terminal silently freezing.
  // Cleared on the next (re)open. Input typed while disconnected is dropped by the
  // socket's readyState guard; this overlay is the signal that it would be.
  const [reconnecting, setReconnecting] = useState(false)

  // Per-PTY ownership. A PTY is shared across every connected device, but only
  // the owner drives its size and may type into it; the others render a read-only
  // take-over placeholder (so two people can't fight over one prompt). This view
  // claims ownership on attach ONLY if the tab is foregrounded (a backgrounded
  // tab attaches as a silent observer). The server broadcasts a `pty.owner` signal
  // carrying the claimer's connection id on every handover; we compare it against
  // OUR PTY-socket connection id (`myConnIdRef`) to decide definitively whether the
  // handover is our own claim (stay owner) or another device taking over (demote to
  // placeholder). `isOwnerRef` mirrors the state so the stable mount-effect closures
  // (onData, the resize senders) read it live rather than capturing a stale value.
  const [isOwner, setIsOwner] = useState(isForeground)
  // Mirror of `isOwner` for the stable mount-effect closures (onData, the resize
  // senders) to read synchronously. Kept in sync only at the mutation points
  // (a take-over and the handover handler), never written during render.
  const isOwnerRef = useRef(isOwner)
  // This view's PTY-socket connection id, delivered as the socket's first
  // `connected` frame (and re-issued on every reconnect). Compared against each
  // `pty.owner` event's claimer id to decide ownership. Null until that frame lands.
  const myConnIdRef = useRef<string | null>(null)
  // A one-shot "claim as soon as our connection id is known" flag. `takeOver`
  // sets it when it fires before the `connected` frame has assigned our id; the
  // next `onConnected` consumes it and performs the deferred resize/claim. Without
  // it, an optimistic claim sent while our id is null carries no recognisable
  // owner and would be immediately revoked by its own `pty.owner` echo.
  const pendingClaimRef = useRef(false)

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
    // This xterm is a VIEWER of a PTY that dux-core's alacritty_terminal already
    // drives and answers device/color queries for. Stop it from also answering
    // (and injecting duplicate replies back into the shared PTY via onData); see
    // suppressViewerReports. Install before open so it is armed before any byte.
    suppressViewerReports(term)
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

    // The dedicated PTY socket for THIS target: the agent's main provider PTY, or
    // a companion terminal's PTY (nested under its owning session). Opening it IS
    // the subscription — connecting an agent socket launches/resumes the provider,
    // exactly as the legacy `Subscribe` did. Registered as the active socket so the
    // macro picker can write to it; cleared on unmount. The byte feed and the
    // first-frame resize are wired further down (after the sizing state exists).
    const pty = new PtySocket(
      kind === "agent" ? agentPtyUrl(id) : terminalPtyUrl(sessionId, id),
    )
    ptyRef.current = pty
    setActivePtySocket(pty)
    // Record this socket's connection id (the socket's first `connected` frame, and
    // again on every reconnect since the server allocates a fresh id per open) so
    // the `pty.owner` handler can compare a handover's claimer id against ours.
    pty.onConnected = (connId) => {
      myConnIdRef.current = connId
      // A take-over requested before our id was known deferred its claim; now that
      // we know our id, perform the resize/claim so the server's resulting
      // `pty.owner` carries an id we recognise as ours.
      if (pendingClaimRef.current) {
        pendingClaimRef.current = false
        const term = termRef.current
        if (term) pty.sendResize(term.rows, term.cols)
      }
    }

    // Forward keystrokes to the PTY as binary. On mobile, sticky modifiers from
    // the accessory bar transform a single typed char (Ctrl-chord, Alt/Meta
    // prefix) before sending; the latch then clears one-shot (visual included).
    // Multi-char chunks (paste/IME) pass through untransformed but still clear
    // any latch. `modsRef` is read live so this once-created closure sees the
    // current latch rather than a stale capture.
    const encoder = new TextEncoder()
    const dataSub = term.onData((s) => {
      // Read-only when we are not the owner: a secondary viewer's keystrokes are
      // dropped client-side (and the server drops them too) so it can never
      // disrupt the active device's typing. The take-over button reclaims input.
      if (!isOwnerRef.current) return
      const mods = modsRef.current
      const out =
        mods.ctrl || mods.alt ? applyModifiers(s, mods) : s
      if (mods.ctrl || mods.alt) {
        setMods({ ctrl: false, alt: false })
      }
      pty.sendInput(encoder.encode(out))
    })

    // Focus the terminal on selection so the user can type immediately — no extra
    // click into the pane. This effect re-runs (and the pane remounts) on every
    // agent OR companion-terminal selection (keyed by [kind, id]), so both cases
    // are covered. Runs after the click that selected the row, so it wins focus.
    // Skip when we attached as a read-only observer (non-owner): there is nothing
    // to type into, and the take-over placeholder owns the surface instead.
    if (isOwnerRef.current) term.focus()

    // Touch gestures over the terminal, mapped to the natural mobile model:
    //   - a one-finger DRAG scrolls the scrollback,
    //   - a stationary LONG-PRESS hands off to the browser's native text
    //     selection (and its handle-drag to extend it),
    //   - a quick TAP falls through to xterm so it focuses and the keyboard opens.
    // xterm's text layer sits over its scrollable viewport, so a finger drag on
    // the output never reaches the native scroll (only the slim scrollbar does);
    // we bridge that by translating a vertical drag into xterm's own
    // scrollLines() — the same scrollback the accessory-bar page buttons move
    // through (they call scrollPages/scrollToTop/scrollToBottom). Touch-only
    // listeners, so this also lights up a touchscreen laptop, not just the mobile
    // layout. Only the normal buffer has scrollback; alt-screen TUIs own the
    // screen (no history), so we leave their touches alone and let the arrow row
    // drive them.
    //
    // Disambiguation: a long-press timer marks the gesture as a selection the
    // moment the finger has been held still past the delay; from then on we never
    // scroll, so extending a selection by dragging a handle is not hijacked. If
    // the finger instead MOVES past a small threshold before that fires, it's a
    // scroll — we cancel the timer and take over. A short, still tap trips
    // neither and reaches xterm as a normal focus tap.
    const LONG_PRESS_MS = 400
    const SCROLL_THRESHOLD_PX = 8
    let touchLastY = 0
    let touchAccum = 0
    let touchScrolling = false
    let touchActive = false
    let touchSelecting = false
    let longPressTimer: ReturnType<typeof setTimeout> | undefined
    const onTouchStart = (e: TouchEvent) => {
      // Any new touch (including a second finger landing mid-gesture) supersedes
      // a pending long-press, so always cancel it first.
      clearTimeout(longPressTimer)
      if (e.touches.length !== 1 || term.buffer.active.type !== "normal") {
        touchActive = false
        return
      }
      touchActive = true
      touchScrolling = false
      touchSelecting = false
      touchAccum = 0
      touchLastY = e.touches[0].clientY
      longPressTimer = setTimeout(() => {
        touchSelecting = true
      }, LONG_PRESS_MS)
    }
    const onTouchMove = (e: TouchEvent) => {
      // Re-check the buffer type: an agent can enter an alt-screen TUI (no
      // scrollback) mid-drag, and we leave those to native handling.
      if (
        !touchActive ||
        touchSelecting ||
        e.touches.length !== 1 ||
        term.buffer.active.type !== "normal"
      )
        return
      const y = e.touches[0].clientY
      touchAccum += y - touchLastY
      touchLastY = y
      // Engage only once the finger has clearly moved, so a tap or an
      // about-to-be long-press is never stolen.
      if (!touchScrolling && Math.abs(touchAccum) < SCROLL_THRESHOLD_PX) return
      if (!touchScrolling) {
        // Movement won the race against the long-press: this is a scroll.
        clearTimeout(longPressTimer)
        touchScrolling = true
        // Reading gesture: get the keyboard out of the way (see onScroll).
        term.textarea?.blur()
      }
      e.preventDefault()
      const { scrollLines, remainderPx } = dragScrollLines(
        touchAccum,
        container.clientHeight / term.rows,
      )
      if (scrollLines !== 0) {
        term.scrollLines(scrollLines)
        touchAccum = remainderPx
      }
    }
    const endTouch = () => {
      clearTimeout(longPressTimer)
      touchActive = false
      touchScrolling = false
      touchSelecting = false
    }
    container.addEventListener("touchstart", onTouchStart, { passive: true })
    container.addEventListener("touchmove", onTouchMove, { passive: false })
    container.addEventListener("touchend", endTouch, { passive: true })
    container.addEventListener("touchcancel", endTouch, { passive: true })

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
    // A resize frame IS a claim of ownership server-side, so we only ever send one
    // while we are the owner: a read-only observer (and a backgrounded tab) drives
    // nothing, which is what keeps two viewers from thrashing the PTY's size and a
    // secondary view from stealing control. A steady-state resize by the current
    // owner does NOT change the owner (no `pty.owner` echo), so it deliberately
    // does not arm one here — only the ownership-ACQUIRING claim below (and
    // take-over) notes a claim.
    const sendOwnedResize = (rows: number, cols: number) => {
      if (!isOwnerRef.current) return
      pty.sendResize(rows, cols)
    }
    const sendSize = () => {
      if (term.rows !== lastRows || term.cols !== lastCols) {
        lastRows = term.rows
        lastCols = term.cols
        sendOwnedResize(term.rows, term.cols)
      }
    }
    // Local fit so the canvas matches this viewport right away, and seed
    // lastRows/lastCols so the ResizeObserver's initial observe callback does NOT
    // send a (racing) resize before the first paint. The initial PTY resize is
    // deferred to the first-frame handler below.
    fit.fit()
    lastRows = term.rows
    lastCols = term.cols

    // Defer the initial PTY resize until the FIRST PTY frame after each (re)open
    // has fully rendered. That frame is the server's repaint: a STATIC snapshot
    // taken at the PTY's current size, which can differ from this viewport.
    // Resizing too early (before the repaint has even arrived over the wire, or
    // mid-render) races a half-painted buffer and leaves the cursor and the
    // bottom-anchored agent prompt in the wrong rows; only a later real resize
    // fixed it. xterm's write callback fires once that frame is parsed, so we fit
    // + resize right after it lands and the agent's SIGWINCH redraw then cleanly
    // replaces the snapshot at our true size. The repaint is sent as a single
    // binary frame, so the first chunk is the whole paint. A fallback timer covers
    // a session that emits no first frame (e.g. an idle freshly launched agent) so
    // its PTY still gets sized. The dedicated socket auto-reconnects, and the
    // server replays the repaint as the first binary frame on EVERY (re)open, so
    // `pty.onOpen` re-arms this guard to re-fit/resize after a reconnect too.
    let initialResizeDone = false
    let jiggleTimer: ReturnType<typeof setTimeout> | undefined
    const sendInitialResize = () => {
      if (initialResizeDone) return
      initialResizeDone = true
      fit.fit()
      lastRows = term.rows
      lastCols = term.cols
      // Attaching while foregrounded claims ownership by sending our size. The
      // server broadcasts a `pty.owner` carrying our connection id; the handover
      // handler recognises it as ours by id, so no echo bookkeeping is needed here.
      // A backgrounded observer is not the owner, so the sends below no-op.
      // Force the agent to FULLY redraw at our size now that the first paint has
      // landed. A same-size resize is a kernel no-op (no SIGWINCH), so when the
      // PTY already matches this viewport the agent never repaints and the
      // reconnect snapshot (which is imperfect for a tall buffer with a
      // bottom-anchored prompt) stays on screen with the cursor and input box
      // misplaced. Nudge the width down one column and back: each step is a real
      // winsize change, so the kernel raises SIGWINCH and the agent redraws its
      // true UI, ending at the correct size. This automates the manual
      // divider-nudge that reliably fixed it.
      sendOwnedResize(term.rows, Math.max(1, term.cols - 1))
      jiggleTimer = setTimeout(() => {
        sendOwnedResize(term.rows, term.cols)
      }, 60)
    }
    // On a RECONNECT the server replays the FULL scrollback as the first binary
    // frame. xterm still holds the buffer from before the drop, so writing the
    // replay on top would stack a second copy of history (duplicated/garbled
    // output). Reset xterm before that first reconnect frame so the replay
    // rebuilds the buffer cleanly. The very FIRST open starts from an empty buffer
    // (a fresh terminal), so it needs no reset — only opens after the first do.
    let firstOpen = true
    let resetBeforeNextFrame = false
    pty.onBytes((bytes) => {
      if (resetBeforeNextFrame) {
        resetBeforeNextFrame = false
        term.reset()
      }
      if (!initialResizeDone) {
        // Resize only once xterm has parsed this first frame (the repaint).
        term.write(bytes, sendInitialResize)
      } else {
        term.write(bytes)
      }
    })
    // On every (re)open the server replays a fresh repaint as the first binary
    // frame; re-arm the first-frame resize so a reconnect re-fits and re-asserts
    // this viewport's size (the same handling the very first open gets). A
    // reconnect (any open after the first) also arms the buffer reset above so the
    // replayed scrollback replaces, rather than stacks on, the stale buffer.
    pty.onOpen = () => {
      // The server allocates a FRESH connection id per open, so the previous id is
      // stale the instant the socket reopens. Clear it now (not only on the next
      // `connected` frame): on reconnect a `pty.owner` over the separate
      // `/ws/events` socket can arrive before this socket's new `connected` frame,
      // and a stale id would make `isOwnerAfterHandover` misjudge ownership. With
      // it null, a pre-`connected` handover safely reads as non-owner and resolves
      // once the new `connected` frame lands (epoch dedup keeps the latest claim).
      myConnIdRef.current = null
      initialResizeDone = false
      setReconnecting(false)
      if (firstOpen) {
        firstOpen = false
      } else {
        resetBeforeNextFrame = true
      }
    }
    // The socket dropped and is retrying: surface the non-blocking reconnect state.
    pty.onReconnecting = () => {
      setReconnecting(true)
    }
    // Fallback for a session that emits no first frame (e.g. an idle freshly
    // launched agent): size its PTY anyway. If the first frame arrives first,
    // the `initialResizeDone` guard makes this a no-op.
    const initialResizeFallback = setTimeout(sendInitialResize, 250)
    // Open the socket now that the byte feed and first-frame handling are wired.
    pty.connect()

    // (A background tab throttles rAF but not timers, so a resize received
    // while hidden refits late or not at all and its debounced send dedupes to
    // a no-op — the visibilitychange handler below re-syncs the PTY on return.)
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(fitFrame)
      fitFrame = requestAnimationFrame(() => fit.fit())
      clearTimeout(sendTimer)
      sendTimer = setTimeout(sendSize, 200)
    })
    ro.observe(container)

    // Re-assert THIS client's size whenever the tab or window returns to the
    // foreground. Two things can leave it stale on return:
    //  1. While hidden, rAF is throttled so the ResizeObserver's deferred fit()
    //     never ran, pinning the canvas and PTY to the pre-switch size.
    //  2. The PTY is SHARED across clients. Another client (typically a phone)
    //     may have resized it to its own dimensions while this tab was
    //     backgrounded or merely unfocused, so the PTY is now sized for that
    //     other viewport, not this one.
    // visibilitychange covers tab hide/show, but moving between a desktop and a
    // phone usually leaves the desktop tab "visible" the whole time, so we also
    // listen for window focus to catch the never-hidden case. The resize send is
    // FORCED (not routed through the deduped sendSize) because the PTY's current
    // size was set by the OTHER client, so our cached lastRows/lastCols would
    // wrongly suppress the re-assert. A same-size resize is a kernel no-op (no
    // SIGWINCH), so re-asserting an unchanged size never causes a spurious
    // redraw; a changed one makes the child redraw at this viewport's true size.
    //
    // The send is debounced (coalescing rapid focus/visibility flaps) and, like
    // the re-attach redraw above, gated on xterm draining its write queue: a
    // foreground return can coincide with the server's scrollback replay still
    // streaming in, and resizing mid-replay corrupts the scroll position. The
    // empty-write callback fires only once the queued writes have drained, so we
    // fit + resize against a settled buffer.
    let resyncTimer: ReturnType<typeof setTimeout> | undefined
    const resyncToForeground = () => {
      if (document.visibilityState !== "visible") return
      clearTimeout(resyncTimer)
      resyncTimer = setTimeout(() => {
        term.write("", () => {
          fit.fit()
          lastRows = term.rows
          lastCols = term.cols
          sendOwnedResize(term.rows, term.cols)
        })
      }, 150)
    }
    document.addEventListener("visibilitychange", resyncToForeground)
    window.addEventListener("focus", resyncToForeground)

    return () => {
      cancelAnimationFrame(fitFrame)
      clearTimeout(sendTimer)
      clearTimeout(initialResizeFallback)
      clearTimeout(jiggleTimer)
      clearTimeout(resyncTimer)
      clearTimeout(longPressTimer)
      container.removeEventListener("touchstart", onTouchStart)
      container.removeEventListener("touchmove", onTouchMove)
      container.removeEventListener("touchend", endTouch)
      container.removeEventListener("touchcancel", endTouch)
      ro.disconnect()
      document.removeEventListener("visibilitychange", resyncToForeground)
      window.removeEventListener("focus", resyncToForeground)
      dataSub.dispose()
      // Close this target's PTY socket (user-initiated: no reconnect) and clear
      // the active-socket registration ONLY if it still points at this one. A
      // focus switch swaps panes; whichever order React runs old-cleanup vs
      // new-effect, the guard ensures we never null out the incoming pane's
      // registration (it has already replaced ours by the time we'd clear it).
      pty.close()
      if (ptyRef.current === pty) ptyRef.current = null
      if (getActivePtySocket() === pty) setActivePtySocket(null)
      termRef.current = null
      term.dispose()
    }
  }, [kind, id, sessionId])

  // React to ownership handovers. The server broadcasts a `pty.owner` carrying the
  // claimer's connection id; the store fans it out by pty id plus that owner id. For
  // OUR pty we compare the owner id against our own PTY-socket connection id: an
  // equal id confirms our own claim (stay the owner), a different id means another
  // device took over (demote to the read-only placeholder). This definitive
  // comparison replaces the old timing heuristic, so two devices claiming at once
  // both converge on the same final owner instead of both falling to the placeholder.
  // Keyed by `id` (the pty id: session id for an agent, terminal id for a companion)
  // so a focus switch re-subscribes for the new target.
  useEffect(() => {
    return onPtyOwner((ptyId, ownerId) => {
      if (ptyId !== id) return
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      // Flip the ref synchronously so an in-flight keystroke is gated by the new
      // state at once, then re-render into the owner view or take-over placeholder.
      isOwnerRef.current = mine
      setIsOwner(mine)
    })
  }, [id])

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

  // Reclaim ownership from another device. Sending our current size IS the claim
  // server-side (most-recent claim wins), so the PTY snaps back to this viewport
  // and our input is forwarded again. Flip the ref synchronously (so the resize
  // passes the owner gate before the state re-render lands), then refocus. The
  // server's resulting `pty.owner` carries our connection id, so the handover
  // handler recognises it as ours by id and keeps us the owner.
  function takeOver() {
    isOwnerRef.current = true
    setIsOwner(true)
    const term = termRef.current
    const pty = ptyRef.current
    if (term && pty) {
      // Only claim now if our connection id is known: the server stamps the
      // resulting `pty.owner` with our id, and we must be able to recognise it as
      // ours or the handover echo would immediately revoke this optimistic claim.
      // If the `connected` frame has not landed yet (myConnIdRef null), defer the
      // claim to the next `onConnected` via a one-shot flag instead of sending a
      // claim whose owner we cannot match.
      if (myConnIdRef.current !== null) {
        pty.sendResize(term.rows, term.cols)
      } else {
        pendingClaimRef.current = true
      }
    }
    term?.focus()
  }

  // Accessory-bar key sends. Esc/Tab/arrows are full sequences, not single
  // chars, so they bypass `applyModifiers` (which only transforms single-char
  // input). We still honor a latched Alt by prefixing ESC, and we clear any
  // latch one-shot afterward — Ctrl on a non-char key has no meaning here, so
  // it's simply consumed. Sends go through the same socket path as typed input.
  const encoder = new TextEncoder()
  function sendSeq(seq: string) {
    // Read-only when not the owner: the accessory-bar keys (Esc/Tab/arrows) are
    // input too, so a secondary viewer's taps are dropped just like typed input.
    if (!isOwnerRef.current) return
    const mods = modsRef.current
    const out = mods.alt ? ESC + seq : seq
    if (mods.ctrl || mods.alt) {
      setMods({ ctrl: false, alt: false })
    }
    ptyRef.current?.sendInput(encoder.encode(out))
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
  // can't. (Alt-screen TUIs keep no scrollback, so page/jump scrolling is a
  // no-op there — the cursor-arrow row drives those instead.)
  //
  // Scrolling is a READ gesture, so it drops the hidden textarea's focus: that
  // slides the soft keyboard away to free the whole screen for reading back and,
  // crucially, stops a scroll-button tap from re-summoning it. On iOS the
  // textarea stays the focused element after the user swipes the keyboard down,
  // so any later tap on a focus-retaining (preventDefault) button pops it right
  // back up; blurring here is what keeps it down. Tapping the terminal refocuses
  // to resume typing. (The key row above instead KEEPS focus — that's input.)
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
    // Only a touch device has a soft keyboard to dismiss. Gating on touch
    // capability stops a narrow-window mouse user (who also gets this mobile bar)
    // from silently losing terminal focus when paging through output.
    if (navigator.maxTouchPoints > 0) term.textarea?.blur()
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
      {/* Readiness / reconnect overlay. Non-blocking (pointer-events-none) so it
          never steals input. Shows while the PTY is still starting up (before its
          first output latches `everReady`) OR whenever the socket has dropped and
          is reconnecting — the latter re-arms even after `everReady`, so a
          mid-session disconnect is visible instead of a silently frozen terminal.
          Reconnect text wins when both apply. */}
      {!everReady || reconnecting ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <BrailleSpinner className="text-primary" />
            <span className="text-sm text-muted-foreground">
              {reconnecting
                ? "Reconnecting…"
                : kind === "agent"
                  ? `Starting ${providerName ?? "agent"}…`
                  : "Launching terminal…"}
            </span>
          </div>
        </div>
      ) : null}
      {/* Read-only secondary view. When another device has taken over this PTY we
          replace the editable terminal with a take-over placeholder (the xterm
          stays mounted underneath, still receiving output, so reclaiming is
          instant — but it is covered and its input is gated off). A solid
          bg-background overlay so it reads as "instead of" the terminal rather
          than a banner over it. */}
      {!isOwner ? (
        <div className="absolute inset-0 z-20 flex items-center justify-center bg-background p-4">
          <Card className="w-full max-w-sm text-center">
            <CardHeader className="items-center gap-3">
              <MonitorSmartphone className="size-8 text-muted-foreground" />
              <CardTitle>This session is active on another device.</CardTitle>
              <CardDescription>
                Only one device can type at a time. Take over to drive this{" "}
                {kind === "agent" ? "agent" : "terminal"} from here.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Button onClick={takeOver} className="w-full max-md:min-h-11">
                <MonitorSmartphone />
                Take over
              </Button>
            </CardContent>
          </Card>
        </div>
      ) : null}
    </div>
  )

  // Desktop: render the pane exactly as before — no extra wrapper, no bar.
  if (!isMobile) return pane

  // In fullscreen the column lives in the fullscreen layer, which the browser
  // forces to the full layout-viewport height (`:fullscreen { height:100%
  // !important }`) — so we CAN'T shrink the fullscreen element itself for the
  // keyboard. The soft keyboard shrinks only the VISUAL viewport, so without
  // this the bottom rows (the prompt) and the accessory bar sit behind the
  // keyboard, unreachable even by scrolling. The fix caps an INNER wrapper (not
  // the fullscreen element) at the visual-viewport height. Only engage when the
  // keyboard is actually up; the `&&` short-circuit avoids reading innerHeight
  // when there's no viewport. The non-fullscreen path is already handled by the
  // MobileApp root, so this is fullscreen-only.
  const constrainToKeyboard =
    isFullscreen &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  // Mobile: a column root so the terminal host (flex-1 min-h-0) and the
  // accessory bar (shrink-0) stack. The ResizeObserver on the host already
  // refits + debounce-resizes the PTY when this column reflows, so no extra
  // resize wiring is needed.
  return (
    <div
      // The mobile fullscreen target: the column wraps the pane AND the bar so
      // both fill the screen in fullscreen. bg-background paints the fullscreen
      // backdrop (incl. the strip behind the keyboard when the inner column is
      // capped below) so it matches the theme rather than going black.
      ref={fullscreenRef}
      className="flex h-full w-full flex-col bg-background"
    >
      {/* Inner column owns the keyboard/safe-area sizing. In fullscreen it caps
          at the visual-viewport height when the keyboard is up (max-height is NOT
          subject to the UA :fullscreen override, since this is a child, not the
          fullscreen element), keeping the bar + prompt above the keyboard. The
          safe-area insets live HERE so they fall inside the capped (border-box)
          height; the bottom inset drops above an open keyboard so no dead strip
          sits between the bar and the keyboard (mirrors MobileApp). Outside
          fullscreen the MobileApp root handles insets, so no inline style. */}
      <div
        className="flex min-h-0 w-full flex-1 flex-col"
        style={
          isFullscreen
            ? {
                maxHeight:
                  constrainToKeyboard && viewportHeight !== null
                    ? viewportHeight
                    : undefined,
                paddingTop: "env(safe-area-inset-top)",
                paddingBottom: constrainToKeyboard
                  ? 0
                  : "env(safe-area-inset-bottom)",
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
    </div>
  )
}
