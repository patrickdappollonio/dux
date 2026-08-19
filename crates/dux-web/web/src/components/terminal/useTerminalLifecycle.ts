// THE ONE LIFECYCLE OWNER for the terminal pane.
//
// It creates the terminal, opens the socket, wires every listener the pair
// needs, and tears all of it down. It re-runs ONLY when the streamed target
// changes, which is the property everything else here is arranged around: every
// closure it creates outlives the render that created it, so nothing may reach
// a closure by being captured. Read-only values come from the live-settings
// container, the three values the wiring writes come from named channels, and
// what remains (a focus call, the pane's own paste handler) is passed in as an
// explicit port.
//
// THE KEY IS THE TARGET, NOT THE ADDRESS. `ptyUrl` is derived here rather than
// handed in, because it is a function of the target and nothing else. The
// target's own parts are then real parameters of the wiring, not merely parts of
// a key: `kind` and `id` decide the tab-gone retry policy, the notification tag,
// and the pty field the upload route stamps.
//
// SURVIVING SUBSCRIPTIONS. Everything below registers inside the one effect and
// is disposed by the one cleanup. The pane keeps a small number of SEPARATE
// registration effects on purpose (the attach capability, the compose sink, the
// focus target, the ownership ledger); those are inventoried in `TerminalPane`
// beside their own code, so a reviewer can tell a deliberate subscription from a
// smuggled-back settings mirror.
import { useEffect, type RefObject } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"

import { inactiveCursorStyle } from "@/lib/composebar"
import { copyTermSelection } from "@/lib/termClipboard"
import { dragScrollLines, dragWheelReport } from "@/lib/viewport"
import { isApplePlatform } from "@/lib/platform"
import {
  applyModifiers,
  classifyClipboardKey,
  copyOnSelectAction,
  forcesTextPaste,
  softNewlineAction,
} from "@/lib/termkeys"
import {
  dispatchMouseReplay,
  latin1Bytes,
  tapReplaySteps,
  wheelReplaySteps,
} from "@/lib/termmouse"
import {
  activateLinkAtPoint,
  linkifierElement,
  terminalTapAction,
} from "@/lib/termlink"
import { handleTabGone, noteOwnPtyConnection } from "@/lib/store"
import { isTabGone } from "@/lib/agentTabs"
import {
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  tabPtyUrl,
  terminalSocketUrl,
} from "@/lib/ptySocket"
import {
  clampTerminalFontSize,
  loadTerminalFontsThenRefit,
  terminalFontFamily,
} from "@/lib/terminalFont"
import {
  VIEWED_PING_INTERVAL_MS,
  shouldSendViewed,
  visibleSinceAfterTransition,
} from "@/lib/viewedPing"
import { isFocusReport } from "@/lib/suppressViewerReports"
import { suppressViewerReports } from "@/lib/suppressViewerReports"
import { registerAgentNotifications } from "@/lib/agentNotifications"
import type { TerminalOwnerRef } from "@/lib/store"
import { ownerSessionId } from "@/lib/terminalOwner"

import type { LiveSettings } from "./liveValues"
import type {
  ConnectionIdentity,
  ModifierLatch,
  OwnershipVerdict,
} from "./channels"
import {
  mouseCaptureHintShown,
  raiseMouseCaptureHint,
} from "./pageSessionHints"
import {
  DRAG_THRESHOLD_PX,
  WHEEL_SCROLL_SENSITIVITY,
  writeSoftNewline,
} from "./constants"
import { createResizeCoordinator } from "./resizeCoordinator"
import { createAttachReplay } from "./attachReplay"
import { createSelectionDrag } from "./selectionDrag"
import { createTouchGesture } from "./touchGesture"
import { createLinkPress } from "./linkPress"

/// The streamed target: an agent tab, or a companion terminal of either owner.
/// `id` is the FOCUSED TAB id for an agent (the session-slot tab's equals
/// `sessionId`; an extra tab's does not) and the terminal id for a terminal.
export type TerminalTarget =
  | { kind: "agent"; id: string; sessionId: string }
  | { kind: "terminal"; id: string; owner: TerminalOwnerRef }

/// Everything the lifecycle needs that is neither a read-only setting nor one
/// of the three channels: the DOM it mounts into, the handles the rest of the
/// pane reads the live terminal and socket through, and the four calls back
/// into the component.
export type TerminalLifecyclePorts = {
  hostRef: RefObject<HTMLDivElement | null>
  containerRef: RefObject<HTMLDivElement | null>
  termRef: RefObject<Terminal | null>
  fitAddonRef: RefObject<FitAddon | null>
  ptyRef: RefObject<PtySocket | null>
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  /// The pointer type of the most recent press on the host, written by the
  /// pane's own JSX handler and read by the contextmenu guard.
  pointerTypeRef: RefObject<string>
  /// The attention-grace transition trackers, shared with the ownership-gain
  /// effect outside this hook.
  visibleSinceRef: RefObject<number | undefined>
  prevVisibleRef: RefObject<boolean | undefined>
  /// A take-over that fired before the connection id was known: consumed by the
  /// next `connected` frame.
  pendingClaimRef: RefObject<boolean>
  live: LiveSettings
  mods: ModifierLatch
  ownership: OwnershipVerdict
  connId: ConnectionIdentity
  focusTypingSurface: () => void
  onClipboardPaste: (e: ClipboardEvent) => void
  /// Arm the force-text-paste hatch. The key handler here arms it and the
  /// pane's own paste listener consumes it, because a key event carries no
  /// clipboard contents and a paste event carries no modifiers.
  armForcedTextPaste: () => void
  setReconnecting: (value: boolean) => void
  setConnectionLost: (value: boolean) => void
}

export function useTerminalLifecycle(
  target: TerminalTarget,
  ports: TerminalLifecyclePorts,
): void {
  const { kind, id } = target
  // The owning session id, when there is one: the agent's own session, or a
  // session-owned terminal's parent. A PROJECT or STANDALONE terminal has none.
  const sessionId =
    target.kind === "agent" ? target.sessionId : ownerSessionId(target.owner)
  // The PTY socket URL for THIS target, derived from the target and nothing
  // else. For an agent, the session-slot tab (`id === sessionId`) uses the
  // session PTY route and an extra tab its own nested route; a terminal uses
  // its owner's nested route.
  const ptyUrl =
    target.kind === "agent"
      ? target.id === target.sessionId
        ? agentPtyUrl(target.sessionId)
        : tabPtyUrl(target.sessionId, target.id)
      : terminalSocketUrl(target.owner, target.id)

  const {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    ptyRef,
    composeInputRef,
    pointerTypeRef,
    visibleSinceRef,
    prevVisibleRef,
    pendingClaimRef,
    live,
    mods,
    ownership,
    connId,
    focusTypingSurface,
    onClipboardPaste,
    armForcedTextPaste,
    setReconnecting,
    setConnectionLost,
  } = ports

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
      // Fallback silently: resolvedBg stays black.
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

    // Stable for this mount, and read by the link machine as well as the key
    // handler further down: it decides which chord is the force-forward hatch
    // (Cmd on a Mac, Ctrl elsewhere) and which clipboard chords to intercept.
    const isMac = isApplePlatform()
    // THE LINK-PRESS MACHINE. Constructed before the terminal because its
    // `linkHandler` has to be passed INTO the constructor; it learns its
    // terminal immediately afterwards.
    const links = createLinkPress({
      hyperlinks: () => live.current.hyperlinks,
      isMac,
    })
    const fontFamily = terminalFontFamily(live.current.fontFamily)
    const fontSize = clampTerminalFontSize(live.current.fontSize)
    const term = new Terminal({
      fontFamily,
      fontSize,
      cursorBlink: true,
      // The unfocused caret. With the compose bar up xterm never holds focus,
      // so this is the ONLY caret that client ever renders; see
      // `inactiveCursorStyle` for why that flips it off the default outline.
      // Read from the ref because this mount effect is stable per target; the
      // dedicated effect below keeps an OPEN terminal in step with the live
      // value.
      cursorInactiveStyle: inactiveCursorStyle(live.current.composeActive),
      convertEol: false,
      scrollback: live.current.scrollbackLines,
      // One wheel notch = 3 lines of local scrollback (see the constant's doc).
      scrollSensitivity: WHEEL_SCROLL_SENSITIVITY,
      overviewRuler: { width: scrollbarWidth },
      theme: { background: resolvedBg },
      // When the app in the PTY enables mouse reporting, xterm forwards a drag to
      // the host instead of selecting locally. The escape hatch is a modifier that
      // forces a LOCAL selection: Shift on Linux/Windows (xterm default), but on
      // macOS xterm gates it behind Option AND this flag. Enable it so a Mac-browser
      // visitor can Option-drag to select and copy to THEIR clipboard rather than
      // the host's. See the `onMouseUp` mouse-capture hint below.
      macOptionClickForcesSelection: true,
      // Make OSC 8 hyperlinks the agent emits clickable. xterm 6's built-in
      // linkHandler resolves real OSC 8 links (unlike the web-links addon, which
      // regex-scans for bare URLs). Opened with noopener,noreferrer so the new
      // tab can't reach back into the app.
      //
      // WHICH activations count is decided by the pure `linkActivateAction`:
      // xterm's Linkifier fires this from a bare `mouseup` with no button or
      // click-count check, so without the filter the second click of a
      // double-click (the select-a-word gesture) opened a SECOND tab, and a
      // right-click opened one on top of dux's own paste. See the helper.
      //
      // `hover`/`leave` exist for the desktop intercept, not for decoration:
      // they are the only public read of "is there a link at this point", since
      // the OSC 8 uri lives in an internal service. All three come from the
      // link machine.
      linkHandler: links.linkHandler,
    })
    links.setTerminal(term)
    // This xterm is a VIEWER of a PTY that dux-core's alacritty_terminal already
    // drives and answers device/color queries for. Stop it from also answering
    // (and injecting duplicate replies back into the shared PTY via onData); see
    // suppressViewerReports. Install before open so it is armed before any byte.
    suppressViewerReports(term)
    // Bridge the agent's notification/clipboard OSC sequences to the browser,
    // mirroring the TUI host passthrough. Registered next to suppressViewerReports
    // so both viewer hooks are armed before the first byte.
    const disposeAgentNotifications = registerAgentNotifications(term, {
      enabled: () => live.current.webNotifications,
      title: () => live.current.notifyTitle,
      clipboardMode: () => live.current.clipboardPassthrough,
      // A stable per-target tag so repeat notifications from this agent/terminal
      // replace instead of stack.
      tag: () => `dux-agent-${id}`,
    })
    // OSC 8 hyperlink gate at the PARSER layer: when hyperlinks are disabled,
    // consume the sequence (return true) so xterm creates no clickable link and it
    // renders as plain text; when enabled, fall through (return false) to xterm's
    // own OSC 8 handler, whose links the `linkHandler` above then gates to http(s).
    // Live-toggleable via the ref; links created before a toggle persist until the
    // cells are rewritten.
    const disposeOsc8Gate = term.parser.registerOscHandler(
      8,
      () => !live.current.hyperlinks,
    )
    // The dedicated PTY socket for THIS target: the agent's main provider PTY, or
    // a companion terminal's PTY (nested under its owning session). Opening it IS
    // the subscription: connecting an agent socket launches/resumes the provider,
    // exactly as the legacy `Subscribe` did. Registered as the active socket so the
    // macro picker can write to it; cleared on unmount. The byte feed and the
    // first-frame resize are wired further down (after the sizing state exists).
    // Opening a tab's socket launches it if it isn't running (resume is decided
    // server-side: it continues only when it's the sole tab coming up). A
    // dormant tab is never auto-mounted (App renders its card instead), so
    // reaching here for one is an intentional launch.
    const pty = new PtySocket(ptyUrl)
    ptyRef.current = pty
    setActivePtySocket(pty)
    // Constructed BEFORE the addon so the resize coordinator below can be given
    // the socket's own `sendResize`; `connect()` is still the explicit act
    // further down, and nothing reads the registration in between.
    const fit = new FitAddon()
    term.loadAddon(fit)
    // THE RESIZE COORDINATOR: the one owner of `fit.fit()` and of every frame
    // that tells the child its size. Nothing below refits or notifies except
    // through it (see its module doc for the one stated font exception).
    const resize = createResizeCoordinator({
      term,
      fit,
      sendResize: (rows, cols) => pty.sendResize(rows, cols),
      isOwner: () => ownership.read(),
    })

    term.open(container)
    // A virtual keyboard must never autocorrect, autocomplete, autocapitalize, or
    // spellcheck into the PTY stream: a shell has no editable buffer for those to
    // fix, so they only inject garbage. xterm is documented to set some of these
    // on its hidden input, but the defaults are not reliable across xterm versions
    // and mobile browsers (autocorrect in particular still fires), so set all four
    // explicitly rather than trusting the library.
    if (term.textarea) {
      term.textarea.setAttribute("autocomplete", "off")
      term.textarea.setAttribute("autocorrect", "off")
      term.textarea.setAttribute("autocapitalize", "off")
      term.textarea.setAttribute("spellcheck", "false")
    }
    resize.fitAfterOpen()
    termRef.current = term
    fitAddonRef.current = fit
    // Open synchronously against fallback metrics (above), then refit once the
    // bundled faces (and any user-named family) are ready. See
    // `loadTerminalFontsThenRefit` for why this happens AFTER open rather than
    // before it: awaiting fonts before opening would delay the PTY connection
    // on every mount for a benefit (correct first-frame metrics) that only
    // matters on a cold font cache.
    loadTerminalFontsThenRefit(term, termRef, fitAddonRef, fontSize, fontFamily)

    // Record this socket's connection id (the socket's first `connected` frame, and
    // again on every reconnect since the server allocates a fresh id per open) so
    // the `pty.owner` handler can compare a handover's claimer id against ours.
    pty.onConnected = (connectionId) => {
      connId.write(connectionId)
      // Register the id as one of OURS in the store, so the server-published
      // `input_owner` spine field can be compared against this client's own
      // identity by surfaces outside this pane (see `sessionActiveElsewhere`).
      noteOwnPtyConnection(connectionId, true)
      // A take-over requested before our id was known deferred its claim; now that
      // we know our id, perform the resize/claim so the server's resulting
      // `pty.owner` carries an id we recognise as ours.
      if (pendingClaimRef.current) {
        pendingClaimRef.current = false
        // Through the coordinator's gesture hold like every other resize path:
        // a claim that lands mid-touch-gesture waits for the lift rather than
        // refitting under the finger. The size is read when the send actually
        // runs, so a claim deferred across a keyboard collapse claims at the
        // FINAL size. It sends DIRECTLY rather than through the owner-gated
        // recorder, because a claim runs while the verdict still says somebody
        // else owns the PTY and must leave the dedupe record untouched.
        resize.directSend(() => {
          const t = termRef.current
          if (t) pty.sendResize(t.rows, t.cols)
        })
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
      if (!ownership.read()) return
      // A focus report raised by the replay we are applying right now is the
      // viewer volunteering state, not a real focus change: drop it. Real
      // transitions (the user clicking into or away from the pane) happen
      // outside this window and still reach the PTY.
      if (attach.replayInFlight() && isFocusReport(s)) return
      const latch = mods.read()
      const out =
        latch.ctrl || latch.alt ? applyModifiers(s, latch) : s
      if (latch.ctrl || latch.alt) {
        mods.write({ ctrl: false, alt: false })
      }
      pty.sendInput(encoder.encode(out))
    })

    // The OTHER half of xterm's output stream. `onData` carries text; `onBinary`
    // carries a byte-per-code-unit "binary string", and the only thing xterm
    // routes through it is a mouse report in the DEFAULT (X10) encoding, which
    // `CoreMouseService.triggerMouseEvent` sends via `triggerBinaryEvent`
    // whenever the app enabled a tracking mode WITHOUT DECSET 1006 (see
    // `lib/termmouse.ts`). Without this subscription every such report was
    // dropped on the floor, desktop clicks included, so a `?1000`-only TUI was
    // simply unclickable in the web UI. `latin1Bytes`, never `TextEncoder`: the
    // X10 form puts `col + 32` in one byte and UTF-8 would split it in two.
    // Deliberately does NOT run the sticky-modifier transform or clear a latch:
    // a mouse report is not a keystroke.
    const binarySub = term.onBinary((s) => {
      if (!ownership.read()) return
      pty.sendInput(latin1Bytes(s))
    })

    // xterm allows only ONE custom key-event handler, so this single closure owns
    // both the soft-newline chord and the clipboard chords. They match disjoint
    // keys (bare Shift-Enter vs Ctrl-based clipboard chords), so soft-newline is
    // checked first and clipboard classification handles the rest.
    //
    // Shift-Enter inserts a "soft" newline (LF / Ctrl-j) instead of submitting.
    // xterm collapses both Enter and Shift-Enter to a carriage return before
    // `onData` can see them, so the two are indistinguishable at the data layer,
    // we must intercept at the key-event layer instead. `softNewlineAction` owns
    // the decision (chord match, IME guard, ownership gate, latch clear); this
    // closure is the thin applicator that turns that decision into DOM/PTY effects.
    //
    // Clipboard chords: xterm's defaults don't bridge the browser clipboard on
    // Linux/Windows: Ctrl+v emits \x16 to the REMOTE agent (pasting the server's
    // clipboard) and Ctrl+c / a selection never reach the system clipboard. We
    // intercept only the clipboard chords; everything else (Ctrl+c SIGINT, plain
    // typing, mac Control/Cmd) passes through to xterm unchanged. `isMac` is stable
    // for this mount and is resolved above, beside the link handler.
    term.attachCustomKeyEventHandler((e) => {
      const action = softNewlineAction(e, {
        isOwner: ownership.read(),
        ctrlLatched: mods.read().ctrl,
        altLatched: mods.read().alt,
      })
      if (action.handled) {
        // Cancel the key with the same semantics xterm applies to every key it
        // handles: `preventDefault` stops the browser dropping a stray newline into
        // the hidden textarea, `stopPropagation` stops the "handled" key bubbling to
        // window-level shortcut listeners, and returning `false` tells xterm not to
        // encode its own CR.
        e.preventDefault()
        e.stopPropagation()
        // Owner-only write. Consume the latch here (the decision came from
        // `softNewlineAction`), then `writeSoftNewline` replays the scroll/selection
        // side effects our early return skipped, shared with the accessory bar's
        // ⇧↵ key so the two entry points can't drift.
        if (action.send !== null) {
          if (action.clearLatch) mods.write({ ctrl: false, alt: false })
          writeSoftNewline(term, pty)
        }
        return false
      }
      // Clipboard chords (keydown only).
      if (e.type !== "keydown") return true
      const chord = {
        ctrlKey: e.ctrlKey,
        shiftKey: e.shiftKey,
        altKey: e.altKey,
        metaKey: e.metaKey,
        code: e.code,
        keyCode: e.keyCode,
        isMac,
      }
      // Arm the text-paste hatch BEFORE the classification and independently of
      // it: on a Mac `Cmd+Shift+v` classifies as `passthrough` (the whole
      // Cmd-anything branch is deliberately the browser's), so folding this
      // into the classifier would have given the hatch to Linux only. Armed
      // here, consumed by the `paste` listener the browser is about to fire.
      if (forcesTextPaste(chord)) armForcedTextPaste()
      const clip = classifyClipboardKey(chord)
      if (clip === "passthrough") return true
      if (clip === "copy") {
        // The chord is not a browser copy event, so we copy the selection
        // ourselves. preventDefault so the browser/devtools don't also act;
        // return false so xterm doesn't process the chord.
        void copyTermSelection(term, focusTypingSurface)
        e.preventDefault()
        return false
      }
      // clip === "paste": return false WITHOUT preventDefault so xterm emits no
      // \x16 and the browser's default Ctrl+v fires a native `paste` event,
      // which xterm's own handler reads from clipboardData (secure-context-free)
      // and forwards as (bracketed) onData.
      //
      // A NON-OWNER takes the same path, deliberately. Swallowing the chord here
      // used to look like the safe thing, and it was the bug: no native paste
      // event fired, so the capture listener never ran, so an image paste from a
      // viewer was silently inert instead of saying why (and only on Linux and
      // Windows, since `Cmd+v` classifies as passthrough and never reached this
      // branch at all). Nothing is lost by letting it through, because a
      // viewer's TEXT paste still cannot reach the PTY: xterm's own paste
      // handler ends in `triggerDataEvent`, which is the `onData` subscription
      // above, and that returns early for a non-owner. The server's `may_write`
      // denies a non-owner's stdin as well, so the gate is two-deep.
      return false
    })

    // Focus the typing surface on selection so the user can type immediately,
    // with no extra click into the pane. This effect re-runs (and the pane remounts)
    // on every agent OR companion-terminal selection (keyed by [kind, id]), so
    // both cases are covered. Runs after the click that selected the row, so it
    // wins focus. With the mobile compose bar up, the typing surface is the
    // compose textarea (so the soft keyboard types into the buffer from the
    // first moment), otherwise xterm's hidden textarea as always; the routing
    // lives in `focusTypingSurface` (it reads only refs, so this once-created
    // closure calling the mount-time instance is harmless). Skip when we
    // attached as a read-only observer (non-owner): there is nothing to type
    // into, and the take-over placeholder owns the surface instead.
    if (ownership.read()) focusTypingSurface()

    // Copy-on-select (highlight to copy), gated by the `copy_on_select`
    // preference. Runs in the `mouseup` user gesture so the clipboard write is
    // permitted even over plain-HTTP (copyToClipboard falls synchronously to its
    // execCommand path there). Record the left-button-down position so mouseup can
    // tell a drag from a click. `copyOnSelectAction` decides: copy a real local
    // selection; when the user dragged but the app captured the mouse (so xterm
    // forwarded the drag to the host and nothing was selected locally), surface a
    // one-time hint to hold the force-selection modifier; otherwise do nothing.
    // The left-button mousedown position, so `onMouseUp` can tell a drag (a
    // selection attempt) from a plain click and only hint about mouse capture
    // on the former. A closure local: it means nothing outside this pair.
    let mouseDownPos: { x: number; y: number } | null = null
    const onMouseDown = (e: MouseEvent) => {
      if (e.button === 0) mouseDownPos = { x: e.clientX, y: e.clientY }
    }
    const onMouseUp = (e: MouseEvent) => {
      const down = mouseDownPos
      mouseDownPos = null
      const dragged =
        down !== null &&
        Math.hypot(e.clientX - down.x, e.clientY - down.y) >= DRAG_THRESHOLD_PX
      const action = copyOnSelectAction({
        copyOnSelect: live.current.copyOnSelect,
        selection: term.getSelection(),
        dragged,
        mouseTrackingMode: term.modes.mouseTrackingMode,
        hintShown: mouseCaptureHintShown(),
        gesture: "mouse-drag",
      })
      if (action === "copy") {
        void copyTermSelection(term, focusTypingSurface)
      } else if (action === "hint") {
        // The wording, the once-per-PAGE latch and the deliberate absence of a
        // toast id all live in `pageSessionHints`; see its doc for why each is
        // what it is.
        raiseMouseCaptureHint(isMac)
      }
    }
    container.addEventListener("mousedown", onMouseDown)
    container.addEventListener("mouseup", onMouseUp)

    // THE LINK-PRESS MACHINE owns the capture-phase intercept, the hover
    // cache, the one opener and the activation counter the touch probe reads.
    // See its module doc for why the decision is made at PRESS time, why the
    // phase and the `stopPropagation` are load-bearing, and why it abstains
    // entirely under the force-local-selection modifier.
    links.attach(container)

    // Kill xterm's right-click paste. On a mouse right-click xterm's own handler
    // stuffs the current selection into its hidden input textarea (its
    // native-Copy preparation); left there it leaks back into the PTY as a paste.
    // We drive our own clipboard menu, so wipe the textarea on `contextmenu`
    // (which fires right after that handler, before any input event could send
    // it). It only touches xterm's hidden input; the selection MODEL that our
    // menu's Copy reads is untouched, so the highlight and Copy stay intact.
    // Touch is NOT exempt, and used to be. Android fires `contextmenu` on a
    // long press, xterm's listener is on `term.element` INSIDE this container
    // so it runs first, and `preventDefault` further up cannot un-run it. That
    // was harmless while a touch long press never produced a selection; now
    // that it does, skipping the wipe would leave the user's selected text
    // sitting in the textarea, which is precisely the leak this guard exists
    // to close. It also takes focus (`moveTextAreaUnderMouseCursor` focuses the
    // textarea), which would raise the soft keyboard over the selection, so
    // hand focus back on the touch path.
    const onContextMenuPasteGuard = () => {
      if (!term.textarea) return
      term.textarea.value = ""
      if (pointerTypeRef.current === "touch") term.textarea.blur()
    }
    container.addEventListener("contextmenu", onContextMenuPasteGuard)

    // Image paste. CAPTURE, and that is the whole trick: xterm's own paste
    // handler sits on the hidden textarea inside this container, and a capture
    // listener on an ancestor runs before it, so dux decides first and an
    // ordinary text paste is passed through untouched. See `onClipboardPaste`.
    const onPasteCapture = (e: ClipboardEvent) => onClipboardPaste(e)
    container.addEventListener("paste", onPasteCapture, true)

    // TOUCH OVER THE TERMINAL, mapped to the natural mobile model. xterm's text
    // layer sits over its scrollable viewport, so a finger drag on the output
    // never reaches the native scroll (only the slim scrollbar does); dux
    // bridges that by translating a vertical drag into xterm's own
    // `scrollLines()`, the same scrollback the accessory bar's PgUp/PgDn keys
    // move through.
    //
    // The normal buffer has xterm scrollback, so a drag scrolls it locally. The
    // ALT-SCREEN (a full-screen TUI like Claude's renderer) has NO xterm
    // scrollback: the app keeps its own history that never reaches xterm. When
    // such an app has mouse tracking on AND this client owns the PTY, the drag
    // is forwarded to it as replayed wheel events, so it scrolls its own
    // history just as a desktop mouse wheel would. If it has no mouse tracking
    // (or this is a read-only viewer), there is nothing to forward to, so the
    // drag does nothing and the arrow row is the way to move.
    //
    // WHICH gesture this is belongs entirely to `createTouchGesture`; what
    // follows are its clients.
    // THE SELECTION-DRAG MACHINE, fed by the gesture machine below: it is told
    // to begin (a long press fired), to extend (the finger moved) and to end.
    // The painted highlight deliberately outlives the gesture; see the module
    // for the arithmetic, the edge auto-scroll timer and the
    // deliberately-unguarded scrollback-trim limit.
    const selection = createSelectionDrag(term)
    const forwardWheelNow = () =>
      term.buffer.active.type !== "normal" &&
      ownership.read() &&
      term.modes.mouseTrackingMode !== "none"
    const gesture = createTouchGesture({
      // On the alt-screen dux can only act if the app takes mouse input and we
      // own the PTY; otherwise leave the touch alone.
      scrollAllowed: () =>
        term.buffer.active.type === "normal" || forwardWheelNow(),
      onGestureReset: () => {
        selection.end()
        resize.setHolding(false)
      },
      onGestureFinished: () => resize.flushHeld(),
      onLongPress: (touch) => selection.begin(touch),
      onSelectMove: (touch) => selection.extend(touch),
      onScrollStart: () => {
        // The gesture now holds the resize pair; the lift releases it.
        resize.setHolding(true)
        // Reading gesture: get the keyboard out of the way (see `onScroll`).
        // Whichever surface holds it (xterm's hidden textarea or the compose
        // bar's) must let go, or the keyboard stays up over the scrollback.
        term.textarea?.blur()
        composeInputRef.current?.blur()
      },
      onScrollMove: (accumPx, touch) => {
        const rowHeight = container.clientHeight / term.rows
        const { scrollLines, remainderPx } = dragScrollLines(accumPx, rowHeight)
        if (scrollLines === 0) return accumPx
        if (forwardWheelNow()) {
          // Forward to the full-screen app as wheel events at the finger's cell
          // (most apps ignore the position, but dux sends a real in-bounds
          // one). Capped to at most ONE wheel notch per touch-move
          // (`dragWheelReport`): `dragScrollLines` can return a many-row
          // magnitude on a fast flick, and forwarding it whole would emit that
          // many reports as a dense burst in a single frame. A mouse-tracking
          // alt-screen app survives the desktop wheel's
          // one-report-per-discrete-event cadence but not that burst: it
          // corrupts the app's scrollback-pager repaint, and because an
          // alt-screen has no client scrollback and nothing reconnects, the
          // duplicated lines persist. One notch per move reproduces the desktop
          // 1:1 cadence while still tracking the finger across moves.
          //
          // The report itself is produced by xterm, not by dux: the wheel event
          // a real mouse would have delivered is replayed at the finger's point
          // and xterm resolves the cell and applies the encoding the app
          // actually negotiated (see `lib/termmouse.ts`). The bytes come back
          // out through `onData`/`onBinary` above.
          const { notch } = dragWheelReport(accumPx, rowHeight)
          dispatchMouseReplay(
            term.element,
            wheelReplaySteps(notch),
            touch.clientX,
            touch.clientY,
          )
        } else {
          term.scrollLines(scrollLines)
        }
        return remainderPx
      },
      // THE LIFT. A long-press SELECTION copies and returns; it is deliberately
      // not a tap, so it never reaches the redirect and never raises the
      // keyboard over the text the user has just selected. A SCROLL is
      // untouched. Only a TAP reaches the redirect below.
      //
      // Tap-to-focus redirect for the compose bar: xterm grabs focus from a
      // `mousedown` listener on its element (`ev.preventDefault(); this.focus()`,
      // see CoreBrowserTerminal), and on touch that mousedown is the SYNTHETIC
      // one the browser dispatches after `touchend`. So when the compose bar is
      // up and this client owns the input, a plain TAP `preventDefault`s the
      // touchend (suppressing the synthetic mouse events, so xterm never
      // focuses its hidden textarea) and focuses the compose textarea instead:
      // the soft keyboard always opens into the buffer.
      //
      // Swallowing those synthetic mouse events also swallows the CLICK a
      // mouse-tracking app would have received through xterm's mouse pipeline,
      // and full-screen TUIs (menus, buttons) are driven by exactly that click.
      // So when the app has mouse tracking on, the tap is forwarded as a
      // replayed press and release at the tapped cell. This restores the
      // BEHAVIOR the app saw before the redirect existed, not a byte-exact
      // replay of the browser's event pipeline; apps consume the click, not the
      // DOM events.
      //
      // Swallowing them ALSO swallows the only thing that can follow an OSC 8
      // hyperlink: xterm's Linkifier resolves the link from `mousemove` and
      // activates it from `mouseup`. So a tap on a link used to do nothing but
      // raise the keyboard. Before deciding the tap is ordinary, the link
      // machine replays that sequence straight at the Linkifier's element; a
      // hit opens through the same one opener, and `terminalTapAction` then
      // says what the rest of the tap does. A tap that HIT a link forwards no
      // click at all, matching the desktop's capture-phase intercept.
      //
      // Preference off / desktop / non-owner: taps reach xterm untouched.
      onLift: (outcome, e) => {
        if (outcome.wasSelecting) {
          // CANCEL the lift. The browser dispatches its compatibility mouse
          // events after an UNCANCELLED touchend, and all three of them are
          // wrong here: xterm focuses its hidden textarea from that `mousedown`
          // (raising the soft keyboard over the text just selected), xterm's own
          // `_handleSingleClick` resets `selectionStartLength` and wipes the
          // highlight the copy was for, and over a mouse-tracking app the click
          // is forwarded into the TUI, which is the one thing "selects locally,
          // forwards nothing" is supposed to guarantee. A DRAG is incidentally
          // protected because `onTouchMove` cancels; a bare press-and-lift, the
          // primary gesture, is not, and Chrome's own long-press threshold sits
          // above ours, so a press held between the two reads as a selection here
          // and an ordinary tap to the browser.
          e.preventDefault()
          // Copy on LIFT, the touch half of copy-on-select, through the same
          // decision function and the same `ui.copy_on_select` preference as the
          // mouse path. Inside the touchend handler on purpose: that is the user
          // gesture, so `copyToClipboard`'s synchronous execCommand fallback is
          // still permitted over the plain-HTTP origins dux is routinely served
          // on (a Tailscale address).
          //
          // Only the "copy" answer is acted on. The other one, the hold-Shift
          // hint for a mouse the app has captured, is meaningless here: the long
          // press just selected locally without any modifier at all.
          const action = copyOnSelectAction({
            copyOnSelect: live.current.copyOnSelect,
            selection: term.getSelection(),
            dragged: true,
            mouseTrackingMode: term.modes.mouseTrackingMode,
            hintShown: mouseCaptureHintShown(),
            // A finger held still for 400ms is deliberate by construction, so it
            // is exempt from the mouse's one-character misclick floor: a `y`, a
            // flag letter or a digit is an ordinary thing to want out of a
            // terminal, and refusing it highlighted the character and copied
            // nothing.
            gesture: "long-press",
          })
          // No refocus: the selection is the result the user wanted, and pulling
          // focus back to a typing surface would throw the soft keyboard over it.
          if (action === "copy") copyTermSelection(term, () => {})
          return
        }
        if (!outcome.wasTap) return
        // The next tap clears the selection, the way tapping elsewhere dismisses
        // one on any touch platform. Before the redirect's own early returns,
        // because it must happen with the compose bar off or this client not
        // owning the input, neither of which stops a finger selecting text.
        if (term.hasSelection()) term.clearSelection()
        if (!live.current.composeActive || !ownership.read()) return
        const compose = composeInputRef.current
        if (!compose) return
        e.preventDefault()
        const touch = e.changedTouches[0]
        // Runs inside the touchend handler, so the window.open a hit produces is
        // still inside the user gesture and is not treated as a popup.
        const linkActivated = touch
          ? activateLinkAtPoint(
              linkifierElement(term.element),
              touch.clientX,
              touch.clientY,
              links.activations,
            )
          : false
        const { forwardClick, focusCompose } = terminalTapAction({
          linkActivated,
          mouseTracking: term.modes.mouseTrackingMode !== "none",
        })
        if (touch && forwardClick) {
          // Replay the press/release the swallowed synthetic mouse events would
          // have been, at xterm's own mouse-report element, so xterm resolves the
          // cell (its `getMouseReportCoords`, which measures the screen element
          // and its padding against the MEASURED cell size) and applies the
          // protocol and encoding the app negotiated. dux used to compute the
          // cell with a parallel arithmetic and hand-encode SGR unconditionally,
          // which was wrong for every app that enabled a tracking mode without
          // DECSET 1006. See `lib/termmouse.ts`.
          //
          // xterm's own mousedown handler grabs focus for its hidden textarea
          // (`focus({preventScroll: true})`), which is exactly what this redirect
          // exists to prevent, so focus is put back immediately below, onto the
          // compose box.
          //
          // The other branch of that restore is now UNREACHABLE, and deliberately
          // kept as the general rule rather than trimmed to today's single caller:
          // a tap that opened a link no longer forwards anything (see
          // `terminalTapAction`), so there is no xterm focus grab left to undo on
          // that path. Focus ends up in the same place either way.
          const focusedBefore = document.activeElement
          dispatchMouseReplay(
            term.element,
            tapReplaySteps(),
            touch.clientX,
            touch.clientY,
          )
          if (!focusCompose && focusedBefore instanceof HTMLElement) {
            focusedBefore.focus()
          }
        }
        if (focusCompose) compose.focus()
      },
    })
    gesture.attach(container)


    // THE SIZING PLUMBING, all of it, is the resize coordinator's: it
    // subscribes to xterm's own resize event (the ONE place geometry reaches
    // the PTY), takes the mount fit, seeds its dedupe from it, arms the
    // no-first-frame fallback, and starts observing the container.
    resize.start(container)
    // THE ATTACH-AND-REPLAY MACHINE owns the (re)open's repaint: the
    // generation dedupe, the reset, the drain and its held chunks, and the
    // focus-report suppression window. See its module doc for each.
    const attach = createAttachReplay({
      term,
      replayGeneration: () => pty.replayGeneration,
      needsFirstFrameResize: resize.needsFirstFrameResize,
      firstFrameLanded: resize.firstFrameLanded,
    })
    pty.onBytes((bytes) => attach.onBytes(bytes))
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
      // Retire the stale id from the store's own-connection set too: the server
      // has already released anything it owned, so keeping it would make a spine
      // field naming it read as "mine" when it no longer is.
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
      }
      connId.write(null)
      setReconnecting(false)
      // The next binary frame is this open's scrollback replay: arm the repaint
      // handling (generation-drop + resize). Only opens AFTER the first also reset
      // the buffer first, since the first open starts from an empty terminal.
      // Arm the repaint handling for this open (generation dedupe, and the
      // reset a reconnect needs), and tell the coordinator which first-frame
      // plan it takes: the very first open jiggles, a reconnect must NOT (an
      // unchanged size would double-repaint the agent on every mobile
      // reconnect) and sends a single plain resize instead.
      resize.noteOpen(attach.noteOpen().firstOpen)
    }
    // The socket dropped and is retrying: surface the non-blocking reconnect state.
    pty.onReconnecting = () => {
      setReconnecting(true)
      // The dropped socket's connection id is dead server-side (its ownerships
      // were released on disconnect); retire it from the own-connection set so
      // a stale spine field naming it cannot read as "mine", and null the ref
      // like the onOpen and unmount paths do, so the two "is this id mine"
      // trackers never disagree (a take-over in this window then defers its
      // claim to the next `connected` frame instead of writing into a closed
      // socket).
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
        connId.write(null)
      }
    }
    // Connection-state transitions. The one we act on is `failed`: the PTY socket
    // now shares the events socket's 3-attempt cap, so when its budget is spent it
    // STOPS (no more silent behind-the-overlay reattach). Swap the endless
    // "Reconnecting…" cue for an explicit "connection lost, Reconnect" affordance.
    // Any retry (`connecting`) or a successful reopen (`open`) clears it.
    pty.onConn = (connState) => {
      if (connState === "failed") {
        setReconnecting(false)
        setConnectionLost(true)
      } else if (connState === "connecting" || connState === "open") {
        setConnectionLost(false)
      }
    }
    // An extra tab's PTY route can go away out from under this client (another
    // client closed that tab); the closed WebSocket carries no HTTP status the
    // client can read, so a 404-forever route is otherwise indistinguishable
    // from a transient drop and `pty` would retry it forever with no escape (see
    // `isTabGone`). Only extra tabs need this: the session-slot tab's route
    // never goes away (closing it just detaches, it doesn't delete a row), and a
    // companion terminal isn't a tab at all.
    if (kind === "agent" && id !== sessionId) {
      pty.shouldRetry = () => !isTabGone(live.current.sessionTabs ?? [], id)
      pty.onGone = () => {
        handleTabGone(id)
      }
    }
    // Open the socket now that the byte feed and first-frame handling are wired.
    pty.connect()

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
    // Periodic "user is looking at this tab" ping (see viewedPing.ts). While we
    // own input AND the document is visible, tell the server we are watching so
    // the agent's attention flag stays down without keystrokes, mirroring the
    // TUI's per-tick focus stamp. A read-only observer or a backgrounded owner
    // never pings (which would suppress attention for everyone on the shared
    // engine, or on a tab whose PTY socket is merely open). Ownership-gain is
    // handled by the [isOwner] effect below; becoming-visible is handled in
    // `resyncToForeground`; this covers the steady-state case.
    const pingViewed = () => {
      if (
        shouldSendViewed({
          isOwner: ownership.read(),
          visible: document.visibilityState === "visible",
          now: Date.now(),
          visibleSince: visibleSinceRef.current,
          graceMs: live.current.attentionGraceMs,
        })
      ) {
        pty.sendViewed()
      }
    }
    const viewedTimer = setInterval(pingViewed, VIEWED_PING_INTERVAL_MS)

    // Fires the one grace-boundary ping (see viewedPing.ts's module doc),
    // cancelled via clearTimeout if the document goes hidden again first.
    let graceTimer: ReturnType<typeof setTimeout> | undefined
    const resyncToForeground = () => {
      const nowVisible = document.visibilityState === "visible"
      const now = Date.now()
      // A real hidden -> visible flip (not a redundant focus-while-visible
      // signal, and not the unobserved initial-load case) arms the grace.
      const transitioned = prevVisibleRef.current === false && nowVisible
      visibleSinceRef.current = visibleSinceAfterTransition(
        prevVisibleRef.current,
        nowVisible,
        visibleSinceRef.current,
        now,
      )
      prevVisibleRef.current = nowVisible

      if (!nowVisible) {
        clearTimeout(graceTimer)
        return
      }

      clearTimeout(graceTimer)
      const graceMs = live.current.attentionGraceMs
      if (transitioned && graceMs > 0) {
        // Suppress the immediate ping; fire exactly once at the grace
        // boundary instead (cancelled above if hidden again before then).
        graceTimer = setTimeout(pingViewed, graceMs)
      } else {
        // Steady state (already visible, e.g. a window focus event) or grace
        // disabled: ping immediately so the flag drops without waiting for
        // the next interval tick, matching the pre-grace behavior.
        pingViewed()
      }

      // The size half of the return: drain-gated, debounced, and forced past
      // the dedupe, all inside the coordinator.
      resize.resyncToForeground()
    }
    document.addEventListener("visibilitychange", resyncToForeground)
    window.addEventListener("focus", resyncToForeground)

    return () => {
      resize.dispose()
      clearTimeout(graceTimer)
      clearInterval(viewedTimer)
      container.removeEventListener("mousedown", onMouseDown)
      container.removeEventListener("mouseup", onMouseUp)
      links.dispose()
      container.removeEventListener("contextmenu", onContextMenuPasteGuard)
      container.removeEventListener("paste", onPasteCapture, true)
      gesture.dispose()
      document.removeEventListener("visibilitychange", resyncToForeground)
      window.removeEventListener("focus", resyncToForeground)
      dataSub.dispose()
      binarySub.dispose()
      // Close this target's PTY socket (user-initiated: no reconnect) and clear
      // the active-socket registration ONLY if it still points at this one. A
      // focus switch swaps panes; whichever order React runs old-cleanup vs
      // new-effect, the guard ensures we never null out the incoming pane's
      // registration (it has already replaced ours by the time we'd clear it).
      // The socket dies with the pane, and so does its connection id: retire
      // it from the store's own-connection set (the server releases anything
      // it owned the moment the socket closes).
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
        connId.write(null)
      }
      pty.close()
      if (ptyRef.current === pty) ptyRef.current = null
      if (getActivePtySocket() === pty) setActivePtySocket(null)
      termRef.current = null
      fitAddonRef.current = null
      disposeAgentNotifications()
      disposeOsc8Gate.dispose()
      term.dispose()
    }
    // The lifecycle owns the terminal's whole lifetime and re-runs ONLY when the
    // streamed target changes. Every other input reaches its closures through
    // the live-settings container, a channel, or a port, all of which are
    // stable; listing a component-body function here would tear the terminal
    // down and rebuild it on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, id, sessionId, ptyUrl])
}
