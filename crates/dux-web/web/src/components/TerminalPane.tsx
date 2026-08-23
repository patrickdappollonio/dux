import { useEffect, useLayoutEffect, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"
import type { FitAddon } from "@xterm/addon-fit"
import { MonitorSmartphone } from "lucide-react"
import { AccessoryBar } from "@/components/AccessoryBar"
import {
  AGENT_PLACEHOLDER,
  ComposeBar,
  TERMINAL_PLACEHOLDER,
} from "@/components/ComposeBar"
import { InputMenu } from "@/components/InputMenu"
import {
  composeBarMode,
  composeBarShown,
  inactiveCursorStyle,
  inputMenuSurfaceSwitchOffered,
  touchSurfacesApply,
  typingSurfaceToggleOffered,
} from "@/lib/composebar"
import {
  getComposeInsertSink,
  setComposeInsertSink,
} from "@/lib/composeInsert"
import {
  peekTerminalFocusTarget,
  setTerminalFocusTarget,
} from "@/lib/terminalFocus"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { useIsMobile } from "@/hooks/use-mobile"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useTypingSurface } from "@/hooks/use-typing-surface"
import { useFilePicker } from "@/hooks/use-file-picker"
import { inputMenuHasItems } from "@/lib/inputMenu"
import { setTypingSurface } from "@/lib/typingSurface"
import { ESC, TAB } from "@/lib/termkeys"
import {
  ejectSelectionForReconnect,
  mobileAccessoryBarVisible,
  mobileTopBarVisible,
  useDux,
} from "@/lib/store"
import type { TerminalOwnerRef } from "@/lib/store"
import type { PtySocket } from "@/lib/ptySocket"
import { matchOwner, ownerProjectId, ownerSessionId } from "@/lib/terminalOwner"
import {
  clampTerminalFontSize,
  loadTerminalFontsThenRefit,
  terminalFontFamily,
} from "@/lib/terminalFont"
import { terminalsForOwner } from "@/lib/terminals"
import {
  DEFAULT_ATTENTION_GRACE_SECONDS,
  shouldSendViewed,
} from "@/lib/viewedPing"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { BrailleSpinner } from "@/components/BrailleSpinner"
import {
  useTerminalLiveSettings,
  type TerminalLiveSettings,
} from "@/components/terminal/liveValues"
import { useTerminalLifecycle } from "@/components/terminal/useTerminalLifecycle"
import { useTerminalOwnership } from "@/components/terminal/ownership"
import {
  useViewerGrid,
} from "@/components/terminal/viewerGrid"
import { xtermScrollbarWidth } from "@/components/terminal/constants"
import { viewerFontFit } from "@/lib/viewerFit"
import {
  focusTypingSurfaceIn,
  useInputSurface,
} from "@/components/terminal/inputSurface"
import { useUploadPipeline } from "@/components/terminal/uploadPipeline"
import { sessionLabel } from "@/lib/agentWorkspace"

type TerminalPaneProps =
  // The streamed target: an agent tab, or a companion terminal of either owner.
  // `id` is the FOCUSED TAB id for an agent (the session-slot tab's equals
  // `sessionId`; an extra tab's does not) and the terminal id for a terminal.
  // The owner (session id for an agent, `TerminalOwnerRef` for a terminal) is
  // passed explicitly: it builds the nested PTY socket URL and the macro
  // target, and the spine may not yet list a just-created terminal when this
  // pane first mounts.
  | { kind: "agent"; id: string; sessionId: string }
  | { kind: "terminal"; id: string; owner: TerminalOwnerRef }

export function TerminalPane(props: TerminalPaneProps) {
  const { kind, id } = props
  // The owning session id, when there is one: the agent's own session, or a
  // session-owned terminal's parent. A PROJECT terminal has none (null); every
  // session-scoped branch below must tolerate that. These two are LOSSY on
  // purpose and are used only to look an owner record up by id; anything that
  // has to SAY something about the owner (the notification title below) matches
  // on it exhaustively instead, because a pair of nulls is indistinguishable
  // from an owner nothing here understands.
  const sessionId =
    props.kind === "agent" ? props.sessionId : ownerSessionId(props.owner)
  const projectId =
    props.kind === "terminal" ? ownerProjectId(props.owner) : null
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
  const termRef = useRef<Terminal | null>(null)
  // The FitAddon of the open terminal, kept in a ref (and cleared alongside
  // termRef) so the live font-settings effect below can refit after changing
  // the xterm options in place.
  const fitAddonRef = useRef<FitAddon | null>(null)
  // The dedicated PTY socket for the focused target. Created by the lifecycle
  // hook and read by the input surface (defined at component scope) so its
  // sends reach the same socket xterm's `onData` does.
  const ptyRef = useRef<PtySocket | null>(null)
  const isMobile = useIsMobile()
  // Is touch the primary pointer? Gates the TYPING SURFACES (see below);
  // `isMobile` stays the width signal for layout and sizing.
  const isCoarsePointer = useIsCoarsePointer()
  // Which typing surface this device was last left on, or null while the
  // pointer capability answers. Transient, per-device, never configuration.
  const typingSurface = useTypingSurface()
  // The hidden `<input type="file">` behind "Attach a file…", and the call that
  // opens it. `pickerInput` is rendered with the bars at the bottom of this
  // file; `openFilePicker` must be called straight from the activating click,
  // or the browser's user activation is spent and no dialog appears.
  const { input: pickerInput, open: openFilePicker } = useFilePicker()

  // Drag-and-drop of a file onto the pane. `dragDepth` counts enter/leave pairs
  // because dragging across a child element fires a `dragleave` for the parent;
  // a plain boolean would flicker the overlay off over every internal boundary.
  const [dragActive, setDragActive] = useState(false)
  const dragDepthRef = useRef(0)

  const duxState = useDux()
  const { spine, bootstrap, offline, conn } = duxState

  // The two `ui.terminal_font_*` preferences (web UI only), read reactively so
  // a live change (Preferences dialog) refonts the open terminal below.
  const terminalFontFamilySetting = bootstrap?.terminal_font_family ?? ""
  const terminalFontSizeSetting = bootstrap?.terminal_font_size ?? 14
  // Whether dropping a file onto this pane does anything at all. `[server]
  // file_drop_max_bytes = 0` switches the feature off, so the whole drag
  // surface goes with it (see `paneAcceptsFileDrag`).
  //
  // NOT YET KNOWN is NOT ENABLED. Bootstrap and the workspace load in parallel,
  // so the pane renders before the bootstrap document arrives, and an older
  // server never sends the field at all. Defaulting that window to ON matched
  // the config default but offered a feature dux could not yet say it had: with
  // the setting switched off, a drag landing in that window still showed the
  // overlay and still uploaded. There is nothing to lose by waiting, because
  // the window closes in one fetch and the drag surface simply appears then.
  const fileDropEnabled = (bootstrap?.file_drop_max_bytes ?? 0) > 0
  // `ui.upload_pasted_text_chars`: how long a TEXT paste has to be before dux
  // saves it as a document and pastes its path instead of typing it. Read as
  // OFF when absent: an older server never published it, and a paste that
  // quietly becomes a file before dux can say the feature exists is a surprise,
  // not a convenience. It only ever reaches an AGENT pane.
  const pastedTextChars = bootstrap?.upload_pasted_text_chars ?? 0

  // THE TYPING SURFACES: the accessory keys and the compose bar.
  //
  // TWO ORTHOGONAL QUESTIONS, and this is the whole rule. WIDTH decides the
  // LAYOUT: how much room is there, so which shell you get, which is what
  // `isMobile` and the mobile column further down answer. THE POINTER decides
  // the TYPING SURFACE: is a finger doing the typing, so does the text need a
  // buffer where autocorrect, swipe and an IME have something to work with. A
  // tablet in landscape gets the DESKTOP layout because it has the room AND
  // needs the buffered input because a finger is still typing, so both bars
  // render inside the desktop shell too. They travel with the pointer, not with
  // the mobile column. `pointer: coarse` also does not change with orientation,
  // which is what stopped a rotation from swapping the typing surface
  // mid-session.
  //
  // The `always`/`never` modes exist because the capability check provably
  // cannot finish the job (see `hooks/use-coarse-pointer.ts` for the
  // measurements), and `typingSurface` is the transient per-device toggle that
  // resolves the same ambiguity in the moment; `composeBarShown` is where the
  // setting-wins rule lives.
  const composeMode = composeBarMode(bootstrap?.compose_bar)
  const composeBarEnabled = composeBarShown(
    composeMode,
    isCoarsePointer,
    typingSurface
  )
  // Do the touch surfaces belong on this device at all? Gates the ACCESSORY
  // bar, and with it the toggle that turns the compose bar on and off.
  const touchSurfaces = touchSurfacesApply(composeMode, isCoarsePointer)
  const surfaceToggleOffered = typingSurfaceToggleOffered(
    composeMode,
    isCoarsePointer
  )
  // The two hideable-bar preferences (`ui.mobile_top_bar`,
  // `ui.mobile_accessory_bar`), resolved through their optimistic overrides.
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  const topBarVisible = mobileTopBarVisible(duxState)

  // Always resolve the owning session by `sessionId` (for an agent, `id` is the
  // FOCUSED TAB id — the session-slot tab's equals the session id, but an extra
  // tab's does not, so a lookup by `id` would miss). A project terminal has no
  // session; it resolves its owning PROJECT instead. The focused tab, when this
  // is an agent, drives the provider label / readiness / exit gating.
  const session =
    sessionId !== null ? spine?.sessions.find((s) => s.id === sessionId) : undefined
  const project =
    projectId !== null ? spine?.projects.find((p) => p.id === projectId) : undefined
  const focusedTab =
    kind === "agent" ? session?.tabs.find((t) => t.id === id) : undefined
  // Title for a bridged desktop notification: the agent's name (or its branch),
  // or the owning project's name for a project terminal.
  //
  // A TERMINAL is named after its OWNER through an exhaustive match, not off the
  // nullable `projectId`/`sessionId` pair above: those collapse an unrecognized
  // owner into two nulls, and this expression then called the terminal "Agent",
  // which is both wrong and unfixable from here. The fallback inside each arm
  // covers an owner id the spine no longer carries, which is a lookup miss, not
  // an unhandled kind.
  const notifyTitle =
    props.kind === "terminal"
      ? matchOwner(props.owner, {
          // A session-owned terminal is named after its AGENT, exactly as
          // before; the generic fallback stays "Agent" for the same reason.
          session: () => (session ? sessionLabel(session) : "Agent"),
          project: () => project?.name || "Terminal",
          // No owner to be named after, so the generic noun is the whole name.
          standalone: () => "Terminal",
        })
      : session
        ? sessionLabel(session)
        : "Agent"
  const isSessionSlotTab = kind === "agent" && id === sessionId
  // A terminal's siblings: the terminals sharing its owner, selected out of the
  // spine's flat collection rather than read off whichever parent it used to be
  // nested under.
  const ownedTerminals =
    props.kind === "terminal"
      ? terminalsForOwner(spine?.terminals ?? [], props.owner)
      : undefined
  const hasOutput =
    kind === "agent"
      ? (focusedTab?.has_output ?? session?.has_output ?? false)
      : (ownedTerminals?.find((t) => t.id === id)?.has_output ?? false)
  const providerName =
    kind === "agent" ? (focusedTab?.provider ?? session?.provider) : session?.provider

  // The compose textarea, owned by ComposeBar but targeted from here: the
  // tap-to-focus redirect and the scroll-gesture keyboard dismissal both need
  // to focus/blur it from outside the component.
  const composeInputRef = useRef<HTMLTextAreaElement | null>(null)
  // True when even the floor font cannot fit the agent's grid in this window.
  // The terminal then overflows deliberately and the host becomes pannable;
  // see the relayout effect. Declared before the live-settings container
  // because the touch gesture's scroll gate reads it through there.
  const [viewerOverflow, setViewerOverflow] = useState(false)
  // THE LIVE-SETTINGS CONTAINER. One snapshot, one synchronising effect, read
  // at call time by every long-lived closure the lifecycle creates. It replaces
  // the sixteen individual ref mirrors (and their sixteen effects) this pane
  // used to carry, and it is deliberately READ-ONLY: the three values the
  // wiring WRITES are named channels, each owned by the unit below that writes
  // it. Declared before the lifecycle hook so its synchronisation runs first on
  // every commit.
  const liveSettings: TerminalLiveSettings = {
    scrollbackLines: bootstrap?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES,
    copyOnSelect: bootstrap?.copy_on_select ?? true,
    fontFamily: terminalFontFamilySetting,
    fontSize: terminalFontSizeSetting,
    fileDropEnabled,
    pastedTextChars,
    attentionGraceMs:
      (bootstrap?.attention_grace_seconds ?? DEFAULT_ATTENTION_GRACE_SECONDS) *
      1000,
    webNotifications: bootstrap?.web_notifications ?? true,
    hyperlinks: bootstrap?.hyperlinks ?? true,
    clipboardPassthrough: bootstrap?.clipboard_passthrough ?? "focused",
    notifyTitle,
    providerName,
    configuredDropPaste: bootstrap?.provider_drop_paste,
    launchedDropPaste: focusedTab?.drop_paste,
    sessionTabs: session?.tabs,
    viewerOverflow,
    // Deliberately the RENDERED value, published one commit later like every
    // other field: see the field's doc for why both mismatch directions are
    // harmless.
    composeActive: composeBarEnabled,
  }
  const live = useTerminalLiveSettings(liveSettings)

  // THE ONE FOCUS-ROUTING RULE, bound to this pane's three handles. It is a
  // standalone function in the input surface rather than a method of it,
  // because two things need it BEFORE that hook has run: the ownership
  // machine's take-over, and the lifecycle's focus-on-mount. Every refocus in
  // the pane goes through this one binding.
  const focusTypingSurface = () =>
    focusTypingSurfaceIn({ live, composeInputRef, termRef })

  // True while the PTY socket has dropped and is retrying (non-blocking), or
  // while a take-over is deliberately bouncing it. Drives a "Reconnecting…"
  // overlay that re-arms even after `everReady` has latched, so a mid-session
  // disconnect is visible rather than the terminal silently freezing. Cleared on
  // the next (re)open. Input typed while disconnected is dropped by the socket's
  // readyState guard; this overlay is the signal that it would be. Declared
  // ABOVE the ownership machine because the take-over bounce raises it: a
  // deliberate `connect()` fires no `onReconnecting` of its own.
  const [reconnecting, setReconnecting] = useState(false)

  // THE OWNERSHIP MACHINE: the four states, the seven transition sites, the
  // verdict channel, the connection identity and the take-over intent, all in
  // one module. See `terminal/ownership.ts`.
  const {
    isOwner,
    ownership,
    connId,
    takeoverIntent,
    seedFromConnected,
    takeoverLabel,
    ownerPresent,
    connectionLost,
    setConnectionLost,
    takeOver,
  } = useTerminalOwnership({
    id,
    kind,
    conn,
    ptyRef,
    focusTypingSurface,
    setReconnecting,
  })

  // THE VIEWER-GRID MACHINE: the honest badge and the bounce-heal. One PTY has
  // one authoritative grid, the owner's, and a viewer rendering the same bytes
  // at a different size is rendering wrapped and clamped output into a local
  // scrollback nothing else will ever clean up. It says so, and it heals by
  // re-attaching, never by resizing the PTY (that is the silent steal).
  const viewerGrid = useViewerGrid({
    ptyRef,
    ownership,
    takeoverIntent,
    setReconnecting,
  })
  // THE FAITHFUL VIEW, presentation half. A watcher renders at the PTY's grid,
  // full stop: there is no preference behind this any more. The coordinator
  // derives the same answer for itself off the verdict channel, because it must
  // be right synchronously; this is the render's copy of it.
  const faithfulWatcher = !isOwner
  // The grid to render, broken out so the relayout effect depends on the
  // NUMBERS rather than on the object identity the machine hands back.
  const remoteRows = viewerGrid.remoteGrid?.rows ?? 0
  const remoteCols = viewerGrid.remoteGrid?.cols ?? 0
  // The mount-scoped port onto the coordinator's grid adoption, installed by
  // the lifecycle (a viewer re-grid is a coordinator act, never a side effect
  // of a font change), and the pane's own relayout, which the coordinator's
  // ResizeObserver calls in place of the fit it does not run.
  const viewerRegridRef = useRef<(() => void) | null>(null)
  const viewerRelayoutRef = useRef<(() => void) | null>(null)
  // Whether the LAST relayout ran the faithful branch. A PROMOTION out of it (a
  // take-over, or self-succeeding after a blip) can change neither the family
  // nor the size, and the else branch below fits only on those, so leaving the
  // branch has to be a third reason to fit; without it the freshly promoted
  // owner stays at the grid it adopted as a watcher forever.
  const lastRelayoutFaithfulRef = useRef(false)

  // THE INPUT SURFACE: the compose Send, the accessory sends, the sticky
  // modifier latches and the draft splice.
  const input = useInputSurface({
    live,
    composeInputRef,
    termRef,
    ptyRef,
    ownership,
  })
  const { ctrl, alt, composeText, setComposeText } = input

  // THE UPLOAD PIPELINE: the three-gesture file journey (drop, paste, picker),
  // its sinks, its batch loop and its one toast.
  const upload = useUploadPipeline({
    id,
    kind,
    live,
    ownership,
    connId,
    termRef,
    ptyRef,
    composeInputRef,
    insertComposeText: input.insertComposeText,
    openFilePicker,
    isOwner,
    isMobile,
    fileDropEnabled,
  })

  // THE RELAYOUT: the one place that decides what font the OPEN terminal wears
  // and how the picture is presented, in both modes.
  //
  // OWNER (and the legacy fit-my-window watcher): the job of the live
  // font-preference effect this replaced, with two deliberate improvements
  // over it: the font load below is kicked off only when the FAMILY actually
  // changed (a size change moves no faces, so fetching on it bought nothing),
  // and the whole thing runs as a layout effect (measured, see below) where
  // the old effect ran after paint. Live-apply a font preference change by
  // setting the xterm options in place and refitting, so rows/cols track the
  // new cell metrics and the re-grid flows to the PTY through xterm's own
  // resize event. A user-named family may not have loaded when the option is
  // set, so refit once more after the browser fetches it; the guard inside
  // `loadTerminalFontsThenRefit` keeps that late refit off a successor
  // terminal after a remount.
  //
  // FAITHFUL WATCHER: the grid is the PTY's, not this window's, so there is no
  // fit at all. The font shrinks instead, to the largest half-pixel size at
  // which the agent's own rows and columns fit here (`lib/viewerFit.ts` owns
  // that arithmetic and its floor), and the grid is re-asserted through the
  // coordinator afterwards. Below the floor the terminal is left overflowing
  // and the host is made pannable, which keeps the picture correct where
  // shrinking further would only make it illegible.
  //
  // A LAYOUT EFFECT because it MEASURES: it reads the host's box and the
  // rendered cell, and doing that after paint would show one frame of the
  // agent's full-size grid every time a watcher adopts a new one. On mount it
  // runs before the lifecycle has created the terminal and is a no-op, exactly
  // as the font effect it replaced was.
  useLayoutEffect(() => {
    const relayout = () => {
      const term = termRef.current
      const container = containerRef.current
      const host = hostRef.current
      if (!term || !container || !host) return
      const family = terminalFontFamily(terminalFontFamilySetting)
      const prefSize = clampTerminalFontSize(terminalFontSizeSetting)
      // The pannable-overflow styles belong to the faithful branch alone;
      // clearing them here means a promotion or a preference flip cannot leave
      // a stale pixel size pinned on the container.
      const clearOverflow = () => {
        setViewerOverflow(false)
        container.style.removeProperty("width")
        container.style.removeProperty("height")
      }
      // Nothing to be faithful TO until the wire has reported a grid, so an
      // older server (or a pty it could not read) keeps the old behavior
      // rather than rendering at a guess.
      const faithful = faithfulWatcher && remoteRows > 0 && remoteCols > 0
      // Which branch the LAST relayout ran, updated on every run so a flip is
      // seen exactly once whichever caller (the effect, the coordinator's
      // observer, the font load) runs next.
      const wasFaithful = lastRelayoutFaithfulRef.current
      lastRelayoutFaithfulRef.current = faithful
      let size = prefSize
      if (faithful) {
        // The cell, measured at whatever font is on screen right now. Cell
        // metrics are font-relative, so one measurement answers for every
        // candidate size. `.xterm-screen` rather than the container, for the
        // reason the selection machine measures it too: the container is wider
        // by the scrollbar gutter. If a grid adoption in this same pass has
        // not reached the DOM yet the ratio is momentarily off by that grid's
        // change, which the next layout signal corrects; it can only ever be a
        // slightly wrong font, never a wrong grid.
        const screen = term.element?.querySelector(".xterm-screen")
        const rect = screen?.getBoundingClientRect()
        const cell =
          rect && term.cols > 0 && term.rows > 0
            ? { width: rect.width / term.cols, height: rect.height / term.rows }
            : { width: 0, height: 0 }
        const style = getComputedStyle(host)
        const padX =
          parseFloat(style.paddingLeft) + parseFloat(style.paddingRight)
        const padY =
          parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
        const gutter = xtermScrollbarWidth()
        const fitted = viewerFontFit({
          // Measured off the HOST, never off the container: the container is
          // what this effect resizes in the overflow case, and measuring it
          // would feed its own output back in.
          available: {
            width: host.clientWidth - padX - gutter,
            height: host.clientHeight - padY,
          },
          grid: { rows: remoteRows, cols: remoteCols },
          cell,
          referenceFontSize:
            typeof term.options.fontSize === "number"
              ? term.options.fontSize
              : prefSize,
          maxFontSize: prefSize,
        })
        size = fitted.fontSize
        if (fitted.overflows) {
          setViewerOverflow(true)
          // Give the overflow a real scroll area rather than hoping one
          // appears: the container is sized to the grid, the host scrolls it.
          container.style.width = `${fitted.width + gutter}px`
          container.style.height = `${fitted.height}px`
        } else {
          clearOverflow()
        }
      } else {
        clearOverflow()
      }
      const familyChanged = term.options.fontFamily !== family
      const sizeChanged = term.options.fontSize !== size
      if (familyChanged) term.options.fontFamily = family
      if (sizeChanged) term.options.fontSize = size
      if (faithful) {
        // The cell metrics just moved, so re-assert the adopted grid: xterm
        // leaves rows/cols alone across a font change, but a fit anywhere else
        // could have moved them and this is the cheap idempotent guard against
        // ever rendering a watcher at the wrong grid.
        viewerRegridRef.current?.()
      } else if (familyChanged || sizeChanged || wasFaithful) {
        // The stated font exception to "only the coordinator fits" (see its
        // module doc): the metrics have moved and the canvas would otherwise
        // be wrong. `wasFaithful` covers the one leaving-the-faithful-branch
        // case the other two miss: a promotion whose shrunk size already equals
        // the preference, with the family untouched, still leaves the terminal
        // standing at the adopted remote grid, and only a fit brings it back to
        // this container's.
        fitAddonRef.current?.fit()
      }
      if (familyChanged) {
        loadTerminalFontsThenRefit(
          term,
          termRef,
          // Late-bound on purpose: the faces can land after another relayout
          // has replaced this closure, and the refit must run the CURRENT
          // rules, not the ones in force when the fetch started.
          () => viewerRelayoutRef.current?.(),
          size,
          family,
        )
      }
    }
    viewerRelayoutRef.current = relayout
    relayout()
    return () => {
      // Only retire our own registration, the pane's standard guard.
      if (viewerRelayoutRef.current === relayout) viewerRelayoutRef.current = null
    }
  }, [
    terminalFontFamilySetting,
    terminalFontSizeSetting,
    faithfulWatcher,
    remoteRows,
    remoteCols,
  ])

  // Retire any in-flight drag the moment the feature stops being available.
  // The gate refuses events for a disabled feature, so once it closes there is
  // no matching `dragleave` or `drop` left to clear the overlay, and it would
  // sit on screen until the pane unmounted. This is reachable in the ordinary
  // case: the bootstrap document can land, saying the feature is off, while a
  // drag is already over the pane.
  //
  // This is the documented "adjust state when a prop changes" pattern (a
  // comparison against the previously seen value, resolved during render)
  // rather than an effect. An effect setting state synchronously runs AFTER
  // the commit, so the stale overlay is painted once and then removed, and it
  // costs a second render pass to do it; this form is caught before the
  // browser sees anything. It reads BOTH directions of the flip on purpose: a
  // feature switched off and then back on must not revive a drag that ended
  // while it was off, since no drag event would ever arrive to clear it.
  const [dropEnabledSeen, setDropEnabledSeen] = useState(fileDropEnabled)
  if (dropEnabledSeen !== fileDropEnabled) {
    setDropEnabledSeen(fileDropEnabled)
    setDragActive(false)
  }
  // The depth counter only means anything while a drag is actually active, so
  // it is pinned back to zero whenever one is not. Without this a count left
  // over from a retired drag would demand that many extra `dragleave`s before
  // the next overlay would close. It lives in an effect rather than in the
  // branch above because a ref must not be written during render.
  useEffect(() => {
    if (!dragActive) dragDepthRef.current = 0
  }, [dragActive])
  // Keep an OPEN terminal's unfocused-caret style in step with the typing
  // surface. The Box/Direct toggle and a preference flip both change the
  // answer mid-session, and xterm options are mutable in place (verified
  // against the installed 6.0.0: only `cols` and `rows` are read-only), so
  // this never touches the terminal's identity. Before the lifecycle effect has
  // run `termRef` is null and this is a no-op; the mount reads the same helper
  // through the container, so a remount opens with the right value.
  useEffect(() => {
    const term = termRef.current
    if (!term) return
    term.options.cursorInactiveStyle = inactiveCursorStyle(composeBarEnabled)
  }, [composeBarEnabled])
  // Tracks the attention-grace hidden -> visible transition (see
  // `visibleSinceAfterTransition` in viewedPing.ts). Refs, not state, so both
  // the lifecycle's visibility listeners and the ownership-gain effect below
  // read/update the same value without re-running the lifecycle.
  // Per-component (not module-level): each mounted pane listens to its own
  // visibilitychange/focus events, so tracking per-component is correct.
  // `undefined` means "no transition observed" (covers initial load).
  const visibleSinceRef = useRef<number | undefined>(undefined)
  const prevVisibleRef = useRef<boolean | undefined>(undefined)
  // There is deliberately no `status_clear_seconds` mirror here. The upload
  // path is entered from a lifecycle listener as well as from a JSX handler,
  // and the listener closes over the MOUNT render, where the bootstrap document
  // has usually not arrived: a value read from the render closure pinned every
  // clipboard-paste toast to the pre-bootstrap default for the life of the
  // pane. `lib/notify.ts` reads the window at raise time, so there is nothing
  // here to capture and nothing to capture stale.

  // The pointer type of the most recent press on the host. Android Chrome fires
  // `contextmenu` on a touch LONG-PRESS, which is dux's own text-selection
  // gesture; right-click paste only fires for a mouse/pen press, so a touch
  // long-press selects text instead of pasting. This per-interaction
  // signal is exact where an `isMobile` width check is not (a touchscreen laptop
  // with a mouse must still get right-click paste).
  const pointerTypeRef = useRef("")

  // WHICH INPUT ROWS EXIST. The accessory keys and the message box each answer
  // for themselves; the menu's own row exists only when neither does.
  const accessoryBarShown = isOwner && touchSurfaces && accessoryBarVisible
  const composeBarShownHere = isOwner && composeBarEnabled

  // THE INPUT ⋯ MENU'S CONTENTS, computed before anything renders, because an
  // ⋯ that opens an empty popup is worse than no ⋯ and the empty state is
  // reachable (a fine pointer whose stored choice put the message box up, with
  // uploads switched off: every gate below is false).
  //
  // A NON-OWNER gets the view toggles only, and only the top-bar one. Attach
  // and the typing surface are input, which a viewer does not have; the keys
  // toggle would be a write with no visible effect on their own screen (their
  // keys never render) that re-hides the OWNER's keys. What is left is the
  // pre-existing dead end this closes: a viewer who hid the top bar from the
  // header menu had hidden the menu with it.
  const inputMenuGates = {
    attach: isOwner && fileDropEnabled,
    surfaceSwitch:
      isOwner &&
      inputMenuSurfaceSwitchOffered(composeMode, isCoarsePointer, typingSurface),
    keysToggle: isOwner && touchSurfaces,
    topBarToggle: isMobile,
  }
  const menuHasItems = inputMenuHasItems(inputMenuGates)

  // THE MENU'S OWN ROW, the third anchor. It renders only when NEITHER bar did,
  // so exactly one ⋯ is ever on screen (the state that used to produce two:
  // keys up, message box off, top bar hidden).
  //
  // "Neither bar" is necessary but not sufficient. The menu belongs to the
  // VIRTUAL INPUT, so its own row appears where that input lives: on the touch
  // surfaces (phone or coarse-pointer tablet, desktop shell included, since
  // both bar preferences are stored server-side and hiding the keys from a
  // phone hides them on the tablet too), or on the phone whose top bar is
  // hidden, which is the chrome-free screen the PWA has no browser Back button
  // to escape from. A fine-pointer desktop grows no new row: its path to the
  // same upload is the agent and terminal row menus.
  const inputMenuRow =
    !accessoryBarShown &&
    !composeBarShownHere &&
    menuHasItems &&
    (isOwner ? touchSurfaces || (isMobile && !topBarVisible) : isMobile && !topBarVisible)

  // IS THIS PANE A COLUMN? True whenever something renders BELOW the terminal:
  // the mobile shell always is one, and any layout showing the touch bars
  // becomes one, desktop included. It decides the pane's own flex role, so the
  // terminal is the flexible row when it has company and simply fills its
  // parent when it does not.
  //
  // The menu's own row counts as company: with both bars down it is the ONLY
  // thing below the terminal, and leaving it out here made the desktop shell
  // drop the row that carries the way back.
  const inColumn =
    isMobile || accessoryBarShown || composeBarShownHere || inputMenuRow

  // Latch readiness: once the PTY has emitted output we keep the spinner hidden,
  // even if a later view model reports `has_output: false` (e.g. an exited
  // agent). Adjusting state during render is the React-sanctioned latch pattern
  // — the guard makes it run at most once, so it can't cascade.
  const [everReady, setEverReady] = useState(false)
  if (hasOutput && !everReady) {
    setEverReady(true)
  }
  // THE ONE LIFECYCLE OWNER. It creates the terminal and the socket, wires
  // every listener the pair needs, and tears both down, re-running only when
  // the streamed target changes. Everything it reads travels through the
  // container, a channel, or one of the ports below.
  useTerminalLifecycle(props, {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    ptyRef,
    composeInputRef,
    pointerTypeRef,
    visibleSinceRef,
    prevVisibleRef,
    takeoverIntent,
    viewerRegridRef,
    viewerRelayoutRef,
    live,
    mods: input.mods,
    ownership,
    connId,
    seedOwnershipFromConnected: seedFromConnected,
    noteRemotePtyGrid: viewerGrid.noteRemoteGrid,
    noteLocalGrid: viewerGrid.noteLocalGrid,
    noteSocketOpen: viewerGrid.noteSocketOpen,
    focusTypingSurface,
    onClipboardPaste: (e) => upload.onClipboardPaste(e),
    armForcedTextPaste: () => upload.armForcedTextPaste(),
    setReconnecting,
    setConnectionLost,
  })

  // THE SURVIVING EFFECTS, inventoried in one place because "one lifecycle
  // owner, and nothing smuggled past it" is only checkable once the exceptions
  // are a list. Every effect below is a registration or a genuine reaction
  // whose lifetime is narrower than the
  // terminal's, which is why it is not folded into the lifecycle hook. Each
  // module-scope registration retires only its OWN registration, because on a
  // focus switch React's old-cleanup / new-effect order is not guaranteed.
  //
  //   IN THIS FILE
  //   [fontFamily, fontSize]              live-apply a font preference change
  //   [dragActive]                        pin the drag-depth counter to zero
  //   [composeBarEnabled]                 the open terminal's inactive caret
  //   [isOwner, composeBarEnabled]        focus the freshly mounted compose box
  //   [composeBarEnabled, isOwner]        the compose-insert sink
  //   [live]                              the terminal focus target
  //   [composeBarEnabled, isOwner]        the compose textarea's paste listener
  //   [isSessionSlotTab, everReady, ...]  eject to the welcome screen on exit
  //   [isOwner]                           the viewed ping on gaining ownership
  //
  //   IN THE UNITS
  //   (every commit)                      the live-settings snapshot
  //   [composeText]                       the draft splice's caret placement
  //   [id]                                the `pty.owner` handover subscription
  //   [kind, id, isOwner, connectionLost] the ownership ledger verdict
  //   [id, isOwner, fileDropEnabled]      the attach capability
  //   [kind, id, sessionId, ptyUrl]       THE LIFECYCLE ITSELF
  //
  // The typing surfaces render only while this client owns the input, so on
  // regaining ownership the compose bar mounts in the SAME commit that flips
  // `isOwner`; `takeOver`'s own `focusTypingSurface()` call runs before that
  // commit (the compose ref is still null) and falls back to xterm. This
  // effect lands after the commit and moves the keyboard into the freshly
  // mounted compose box. Idempotent on the initial mobile mount, a no-op on
  // desktop (`composeBarEnabled` is mobile-gated).
  useEffect(() => {
    if (isOwner && composeBarEnabled) composeInputRef.current?.focus()
  }, [isOwner, composeBarEnabled])
  // While the compose bar is actually rendered (mobile, `ui.compose_bar` on,
  // input owner — the same gate as the render below), register the
  // compose-insert sink the store's `runMacro` routes a picked macro through:
  // the macro's RAW text is spliced into the DRAFT at the caret, an editable
  // message the user reviews and Sends, never an immediate PTY write. The
  // module-scope hand-off exists because the mobile macro picker lives in the
  // terminal screen's header (MobileShell), outside this pane — see
  // `composeInsert.ts`. The sink retires the moment the bar stops rendering
  // (viewer demotion, preference flip, unmount), restoring the direct
  // macro-to-PTY path everywhere the bar is not the typing surface.
  useEffect(() => {
    if (!(composeBarEnabled && isOwner)) return
    const sink = {
      insert: input.insertComposeText,
      target: () => composeInputRef.current,
    }
    setComposeInsertSink(sink)
    return () => {
      // Only retire our own registration: a successor pane may already have
      // replaced it (the same guard `setActivePtySocket` cleanup uses).
      if (getComposeInsertSink() === sink) setComposeInsertSink(null)
    }
    // `insertComposeText` is a component-body function that reads only refs and
    // the (stable) state setter, so a new identity every render says nothing new
    // and listing it would re-register the sink on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeBarEnabled, isOwner])
  // The desktop macro picker now lives in the header (`InsetHeader`), outside
  // this pane, so it cannot reach xterm to hand Base UI a close-focus target.
  // Register the pane's typing surface for it, on the same module-scope
  // hand-off idiom as the compose sink above (and with the same
  // only-retire-your-own-registration guard). The pane resolves the surface at
  // CALL time through `focusTypingSurface`'s own rule, so a picker close lands
  // where typing already was rather than where it was when this effect ran.
  useEffect(() => {
    const target = () =>
      live.current.composeActive && composeInputRef.current
        ? composeInputRef.current
        : (termRef.current?.textarea ?? null)
    setTerminalFocusTarget(target)
    return () => {
      if (peekTerminalFocusTarget() === target) setTerminalFocusTarget(null)
    }
  }, [live])
  // The compose textarea's own image-paste listener. The compose bar renders
  // OUTSIDE the terminal container (it is a sibling row of the mobile shell),
  // so the container's capture listener cannot see a paste that lands in it,
  // and on a phone the compose box is where a paste lands: the tap redirect
  // puts focus there. Registered on the element rather than passed as an
  // `onPaste` prop so `ComposeBar` stays presentational, and no capture phase
  // is needed because this IS the target. The same gate as the render below,
  // so the listener exists exactly while the box does.
  useEffect(() => {
    if (!(composeBarEnabled && isOwner)) return
    const el = composeInputRef.current
    if (el === null) return
    const handler = (e: ClipboardEvent) => upload.onClipboardPaste(e)
    el.addEventListener("paste", handler)
    return () => el.removeEventListener("paste", handler)
    // Same reason as the sink above: `onClipboardPaste` reads refs and the live
    // bootstrap document at call time, so re-registering the listener whenever
    // its identity changes would buy nothing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeBarEnabled, isOwner])

  // Mirror the TUI's exit behavior: when the agent we were attached to stops
  // running (it produced output in this pane, then its session left `active`
  // — the exit prune marks it detached), reset the center pane back to the
  // welcome screen. The "Agent exited" toast explains why. A fresh selection
  // of the detached agent remounts this pane and relaunches it.
  // Reset to the welcome screen only when the session-slot tab we were attached to
  // stops and the whole agent left `active` (status is any-tab-active). Gate on
  // `isSessionSlotTab`: an extra tab's own exit just turns it dormant via the spine
  // (handled in `App`, its card rendered there), so we don't eject the user from
  // here in that case.
  const sessionStatus = session?.status
  useEffect(() => {
    if (isSessionSlotTab && everReady && sessionStatus && sessionStatus !== "active") {
      // Marked as OUR eject (not a user navigation) so a re-armed reconnect
      // deep-link can tell it apart from a deliberate home nav and restore the
      // route once this agent finishes resuming. See `ejectSelectionForReconnect`.
      ejectSelectionForReconnect()
    }
  }, [isSessionSlotTab, everReady, sessionStatus])

  // Gaining ownership is a fresh "looking at it" moment: ping the server at once
  // (when visible) so the agent's attention flag drops immediately, rather than
  // waiting up to one interval for the periodic viewed ping in the mount effect.
  useEffect(() => {
    if (
      shouldSendViewed({
        isOwner,
        visible: document.visibilityState === "visible",
        now: Date.now(),
        visibleSince: visibleSinceRef.current,
        graceMs: live.current.attentionGraceMs,
      })
    ) {
      ptyRef.current?.sendViewed()
    }
    // The trigger is the ownership TRANSITION and nothing else; `live` is a
    // stable container read at call time, and listing it would re-fire the ping
    // on every commit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOwner])

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
  // the pane covers every host: the desktop ResizablePanel and the mobile
  // viewport-pinned root. The overlays (macro popover, readiness card) are
  // absolutely positioned inside these bounds, so clipping never affects them.
  const pane = (
    <div
      className={
        // Inside a column (the mobile shell, or ANY layout carrying the touch
        // bars below the terminal) the pane is the flexible row; standing alone
        // it simply fills its parent.
        inColumn
          ? "group relative min-h-0 w-full flex-1 overflow-hidden bg-background"
          : "group relative h-full w-full overflow-hidden bg-background"
      }
      onDragEnter={(e) => {
        if (!upload.paneAcceptsFileDrag(e)) return
        e.preventDefault()
        dragDepthRef.current += 1
        setDragActive(true)
      }}
      onDragOver={(e) => {
        if (!upload.paneAcceptsFileDrag(e)) return
        // Without preventDefault on dragover the browser refuses the drop and
        // navigates to the file instead, which loses the whole page.
        e.preventDefault()
        e.dataTransfer.dropEffect = "copy"
      }}
      onDragLeave={(e) => {
        if (!upload.paneAcceptsFileDrag(e)) return
        dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
        if (dragDepthRef.current === 0) setDragActive(false)
      }}
      onDrop={(e) => {
        if (!upload.paneAcceptsFileDrag(e)) return
        e.preventDefault()
        dragDepthRef.current = 0
        setDragActive(false)
        // Desktop only (`paneAcceptsFileDrag` refuses a drag on a phone), so this
        // always resolves to the terminal today. It still asks rather than
        // assuming, so a drop and a paste can never disagree about where a
        // path belongs.
        void upload.runUpload(
          Array.from(e.dataTransfer.files),
          upload.activeUploadSink(),
        )
      }}
    >
      {/* The drop target. Shown only while a file is actually over the pane and
          only to whoever holds input, because a viewer cannot paste the path
          afterwards. It names what will happen and where the file will land;
          the terminal's real folder is discovered server-side at upload time, so
          the promise here is deliberately about WHICH folder rather than its
          path. Pointer-events-none so it cannot swallow the drop it is
          advertising. */}
      {dragActive ? (
        <div
          data-testid="file-drop-overlay"
          className="pointer-events-none absolute inset-2 z-20 flex flex-col items-center justify-center gap-1 rounded-lg border-2 border-dashed border-primary bg-background/90 p-4 text-center"
        >
          <p className="text-sm font-medium text-foreground">
            Drop to save the file and paste its path
          </p>
          <p className="text-xs text-muted-foreground">
            {props.kind === "agent"
              ? "It lands in this agent's upload folder, hidden from git and removed with the agent."
              : "It lands in the folder this terminal is currently in."}
          </p>
        </div>
      ) : null}
      {/* Padding lives on the host, NOT the measured element below — see the
          hostRef comment: border-box computed heights include padding, and
          FitAddon would mint a phantom row/column from it. A mouse/pen right-click
          pastes the clipboard; a TOUCH long-press (which fires `contextmenu` on
          Android) is dux's own text-selection gesture, so its menu is
          suppressed instead. See `pointerTypeRef`. */}
      <div
        ref={hostRef}
        // PANNABLE ONLY WHEN IT HAS TO BE. A faithful watcher whose window
        // cannot hold the agent's grid even at the floor font gets the
        // terminal at its true size and scrolls to the rest of it, which is
        // the honest answer where shrinking further would be an illegible one.
        // In every other state this is the same fixed, unscrollable host it
        // has always been, and the pane above it stays the clip boundary.
        className={
          viewerOverflow ? "h-full w-full overflow-auto p-2" : "h-full w-full p-2"
        }
        onPointerDown={(e) => {
          pointerTypeRef.current = e.pointerType
        }}
        onContextMenu={(e) => {
          // Touch long-press. dux OWNS this gesture now (it selects the word
          // under the finger), so suppress whatever menu the platform wanted to
          // raise over it: Android fires `contextmenu` on a long press, and a
          // menu appearing on top of a selection the user is dragging is the
          // gesture failing. It must NOT fall through to the paste below: a
          // long press is not a right-click.
          if (pointerTypeRef.current === "touch") {
            e.preventDefault()
            return
          }
          // Mouse/pen right-click pastes the clipboard (classic terminal model,
          // no menu). preventDefault suppresses the native browser menu; the
          // contextmenu textarea-wipe (mount effect) kills xterm's own right-click
          // selection-stuffing so only the clipboard is pasted.
          e.preventDefault()
          input.onRightClickPaste()
        }}
      >
        {/* data-testid: the touch-gesture and tap-redirect tests dispatch real
            TouchEvents on this exact node (the element the gesture listeners
            are registered on); there is no accessible role to query it by. */}
        {/* `-webkit-touch-callout: none` is load-bearing on iOS: without it a
            long press raises Safari's own magnifier loupe and share menu over
            the gesture dux is using to select text. It costs nothing on the
            platforms that ignore it. */}
        <div
          ref={containerRef}
          data-testid="terminal-container"
          className="h-full w-full [-webkit-touch-callout:none]"
        />
      </div>
      {/* The picker's hidden input. It lives INSIDE the pane rather than with
          the bars because the bars are conditional and this must not be: the
          row menus can attach through a pane that renders no input row at all
          (a desktop with a mouse), and the click that opens the dialog has to
          reach a mounted element synchronously. It renders nothing and is out
          of the accessibility tree; see `useFilePicker`. */}
      {pickerInput}
      {/* The desktop macro trigger used to float HERE, as an
          absolutely-positioned overlay over the PTY text. It now lives in the
          center pane's top bar (`InsetHeader`), parked on this pane's right
          edge, so it no longer covers the terminal's own output and reads as one
          family with the header's other controls. The mobile entry point is
          unchanged: the terminal screen's header icon button (MobileShell).
          Focus still returns to this pane's typing surface on close, through the
          `terminalFocus` registration above rather than a prop. */}
      {/* Readiness / reconnect overlay. Non-blocking (pointer-events-none) so it
          never steals input. Shows while the PTY is still starting up (before its
          first output latches `everReady`) OR whenever the socket has dropped and
          is reconnecting — the latter re-arms even after `everReady`, so a
          mid-session disconnect is visible instead of a silently frozen terminal.
          Reconnect text wins when both apply.

          But when the WHOLE app is offline (the events socket is down, so every
          PTY dropped too), the app-wide `OfflineOverlay` already owns that signal
          — and it deliberately leaves the grayscaled UI visible behind it, where a
          per-pane "Reconnecting…" spinner would show through and double up. So
          suppress the reconnect variant while globally offline; the initial
          startup spinner (`!everReady`) is unrelated and still shows. */}
      {connectionLost && !offline ? (
        // Hard stop: the PTY socket exhausted its reconnect budget and gave up.
        // Surface an explicit Reconnect affordance. Reconnect imperatively on THIS
        // pane's own socket: `pty.connect()` resets the attempt budget and reopens
        // in place, preserving the xterm buffer and working uniformly for an agent
        // tab OR a companion terminal. (A `terminalEpoch` bump would NOT: it only
        // feeds the pane key for `kind === "agent"`, so it is a no-op for a
        // companion terminal and would leave its Reconnect button dead.) The
        // resulting `onConn("connecting")`/`"open"` clears `connectionLost`.
        <div className="absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-3 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <span className="text-sm text-muted-foreground">
              Connection lost.
            </span>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => ptyRef.current?.connect()}
            >
              Reconnect
            </Button>
          </div>
        </div>
      ) : !everReady || (reconnecting && !offline) ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <BrailleSpinner className="text-primary" />
            <span className="text-sm text-muted-foreground">
              {reconnecting && !offline
                ? "Reconnecting…"
                : kind === "agent"
                  ? `Starting ${providerName ?? "agent"}…`
                  : "Launching terminal…"}
            </span>
          </div>
        </div>
      ) : null}
      {/* THE TAKE-OVER CARD, full-pane and solid on purpose. When another
          device drives this PTY we replace the editable terminal with a
          take-over placeholder: a solid bg-background overlay so it reads as
          "instead of" the terminal rather than a banner over it.

          DELIBERATE, not a rendering shield. The faithful view keeps the
          picture underneath clean (the watcher renders at the PTY's own grid,
          so the local scrollback never records wrapped garbage), but the card
          is a statement about control, not about pixels: a device with a
          DIFFERENT viewport size is driving this PTY, and taking over
          retargets the PTY's size to this device. Covering the pane says that
          plainly where a strip along one edge would not. The xterm stays
          mounted underneath, still receiving output, so reclaiming is instant;
          it is covered and its input is gated off, and the take-over remains a
          fresh attach.

          It yields to the connection-lost affordance above. This card paints
          solid over the whole pane and renders AFTER those overlays, so a
          non-owner whose socket has died would otherwise see only "Take over"
          and never the Reconnect button: the health of the connection would be
          invisible behind exactly the surface that needs it. Suppressing the
          card here (rather than lifting the overlays' z-order) keeps one state
          on screen at a time; raising the overlays instead would leave this
          solid card painted underneath a floating Reconnect box, reading as two
          stacked answers to one question. The condition mirrors the overlay's
          own `connectionLost && !offline`, so when the app-wide offline overlay
          owns the signal the card stays exactly as it was. */}
      {!isOwner && !(connectionLost && !offline) ? (
        <div className="absolute inset-0 z-20 flex items-center justify-center bg-background p-4">
          <Card className="w-full max-w-sm text-center">
            <CardHeader className="items-center gap-3">
              <MonitorSmartphone className="size-8 text-muted-foreground" />
              {/* THREE TITLES, because there are three truths and the card used
                  to tell only two of them. "Nobody is driving" is the one the
                  owner-cleared broadcast made reachable: the device that was
                  driving has disconnected, and nobody claims passively, so
                  every viewer, foregrounded or not, keeps the card and this
                  title until a deliberate act (the arc's own test asserts the
                  foregrounded case). Saying "Active on another device" there
                  would name a browser tab that has closed. */}
              <CardTitle>
                {takeoverLabel
                  ? `Open on ${takeoverLabel}`
                  : ownerPresent
                    ? "Active on another device"
                    : "Nobody is driving"}
              </CardTitle>
              <CardDescription>
                {ownerPresent
                  ? "Only one device can type at a time."
                  : "Whoever was driving has disconnected."}{" "}
                Take over to drive this{" "}
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

  // NOTHING BELOW THE TERMINAL: the pane stands alone exactly as it always did
  // on a desktop with a mouse.
  if (!inColumn) return pane

  // A column root so the terminal host (flex-1 min-h-0) and the bars (shrink-0)
  // stack. In the MOBILE shell the MobileApp root pins the whole thing to the
  // visual viewport (and interactive-widget=resizes-content shrinks the layout
  // viewport for the soft keyboard), so this column just fills its parent and
  // the accessory bar sits on the keyboard, with no per-pane keyboard sizing.
  //
  // In the DESKTOP shell the pane fills a ResizablePanel of a fixed height, so
  // the bars take their height OUT of the terminal rather than growing the
  // page: the terminal shrinks by the bars' height and the panel geometry is
  // untouched. That is the right trade and it is the same one the phone makes.
  // The bars are only up when a finger is doing the typing, the soft keyboard
  // is about to take far more room than they do, and a user who wants the rows
  // back has the toggle in the accessory bar. Nothing here reaches out to the
  // panel: the pane's own ResizeObserver refits and debounce-resizes the PTY
  // when this column reflows, so the PTY learns its new size through the path
  // it already used. (The web UI has no fullscreen mode; see the CLAUDE.md web
  // tenet.)
  // THE ONE INPUT ⋯, built once and placed by the anchor matrix below. It is
  // the same element in every state, so the menu cannot drift between anchors,
  // and building it here (rather than three times inline) is what makes
  // "exactly one instance renders" readable at the call sites.
  const inputMenu = (
    <InputMenu
      gates={inputMenuGates}
      onAttach={upload.attachFromPicker}
      composeSurface={composeBarEnabled}
    />
  )

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {pane}
      {/* Typing surfaces render only for the input OWNER. When another device
          drives this PTY, the take-over card (inside `pane`) is this client's
          only interaction: hiding the accessory keys and the compose bar
          removes any surface that could even stage input at a session this
          device does not drive. The per-write owner gates (`sendSeq`,
          `sendCompose`) stay behind this as defense in depth, and the bars
          reappear the moment ownership returns.

          The input ⋯ menu is NOT owner-gated, because its view toggles are not
          input: a viewer who hid the phone's top bar from the header menu hid
          the menu with it, and had no way back at all. Their menu carries that
          one item; see `inputMenuGates`. */}
      {isOwner ? (
        <>
          {/* The accessory bar is additionally gated on the
              `ui.mobile_accessory_bar` preference (default on): hiding it
              returns its two key rows to the terminal. The input ⋯ menu (which
              is on screen in every bar state) and Preferences bring it back.

              ANCHOR: when the message box is off, this row is the bottom-most
              input row, so it carries the menu. When the box is up, the compose
              row below carries it instead and this passes nothing. */}
          {accessoryBarShown ? (
            <AccessoryBar
              onEsc={() => input.sendSeq(ESC)}
              onTab={() => input.sendSeq(TAB)}
              onNewline={input.sendNewline}
              onArrow={input.onArrow}
              onScroll={input.onScroll}
              ctrl={ctrl}
              alt={alt}
              onToggleCtrl={input.toggleCtrl}
              onToggleAlt={input.toggleAlt}
              composeSurface={surfaceToggleOffered ? composeBarEnabled : undefined}
              onToggleSurface={
                surfaceToggleOffered
                  ? () =>
                      setTypingSurface(composeBarEnabled ? "direct" : "compose")
                  : undefined
              }
              inputMenu={
                !composeBarShownHere && menuHasItems ? inputMenu : undefined
              }
            />
          ) : null}
          {/* The compose bar: the row below the accessory bar's two key rows,
              so the typing surface sits directly on the soft keyboard. When it
              is off nothing renders and the tap-to-focus redirect stays
              dormant, so the terminal behaves exactly as it did before the bar
              existed. The draft value lives in this pane's state, so losing and
              regaining ownership keeps an in-progress draft.

              ANCHOR: whenever this row exists it is the bottom-most input row,
              so it carries the menu in its leading slot. */}
          {composeBarShownHere ? (
            <ComposeBar
              value={composeText}
              onChange={setComposeText}
              onSend={input.sendCompose}
              inputRef={composeInputRef}
              // A physical keyboard's Escape or F-key pressed while the
              // compose box is focused: the same bytes on the same write
              // path as the accessory bar's Esc key (`sendSeq` owns the
              // ownership gate and the modifier latch), because a hardware
              // Esc is the physical twin of tapping that key. Which keys
              // qualify is the pure `composeHardwareKeyForwards` rule.
              onForwardKey={input.sendSeq}
              // WHAT THIS SURFACE IS FOR, off the pane's own kind: an agent
              // pane is a conversation with a CLI, every terminal pane is a
              // shell, and `kind` is the discriminator that already answers
              // that (a terminal's OWNER kind, session/project/standalone,
              // varies the spawn directory, not the activity, so all three
              // get the shell wording).
              placeholder={
                kind === "agent" ? AGENT_PLACEHOLDER : TERMINAL_PLACEHOLDER
              }
              leading={menuHasItems ? inputMenu : undefined}
            />
          ) : null}
        </>
      ) : null}
      {/* THE MENU'S OWN ROW, the third anchor: neither bar rendered, so
          without this the terminal screen would be completely chrome-free, and
          the app ships as a standalone PWA where no browser Back button
          exists. It is the way back to the keys in the DESKTOP shell too, on a
          coarse pointer: the accessory keys belong to that shell as well, so a
          hidden key row is just as much a dead end there. See `inputMenuRow`
          for why "neither bar" alone is not enough to put it on screen. */}
      {inputMenuRow ? (
        <div className="flex shrink-0 items-end gap-1.5 border-t bg-background px-1 py-1">
          {inputMenu}
        </div>
      ) : null}
    </div>
  )
}
