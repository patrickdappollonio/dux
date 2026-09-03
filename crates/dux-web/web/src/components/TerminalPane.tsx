import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react"
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
  bottomBarSurvivesDirect,
  composeBarMode,
  composeBarShown,
  inactiveCursorStyle,
  inputMenuSurfaceSwitchOffered,
  terminalKeysApply,
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
import { isFirstTab } from "@/lib/agentTabs"
import { useIsMobile } from "@/hooks/use-mobile"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useTypingSurface } from "@/hooks/use-typing-surface"
import { useFilePicker } from "@/hooks/use-file-picker"
import { inputMenuHasItems, type InputMenuGates } from "@/lib/inputMenu"
import { ESC, TAB } from "@/lib/termkeys"
import {
  ejectSelectionForReconnect,
  mobileAccessoryBarVisible,
  noteTheaterOwnershipLost,
  useDux,
} from "@/lib/store"
import type { DuxState, TerminalOwnerRef } from "@/lib/store"
import {
  theaterOwnershipStep,
  theaterOwnershipWatchStart,
} from "@/lib/theater"
import type { PtySocket } from "@/lib/ptySocket"
import { matchOwner, ownerProjectId, ownerSessionId } from "@/lib/terminalOwner"
import { terminalsForOwner } from "@/lib/terminals"
import { DEFAULT_ATTENTION_GRACE_SECONDS } from "@/lib/viewedPing"
import { attachCover, type AttachCover } from "@/lib/attachCover"
import { assertNever } from "@/lib/assertNever"
import { replayWaitMs } from "@/lib/connectionTiming"
import { createVisibleClock, type VisibleClock } from "@/lib/visibleClock"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { GlyphSpinner } from "@/components/GlyphSpinner"
import {
  useTerminalLiveSettings,
  type TerminalLiveSettings,
} from "@/components/terminal/liveValues"
import { useTerminalLifecycle } from "@/components/terminal/useTerminalLifecycle"
import { useTerminalRelayout } from "@/components/terminal/useTerminalRelayout"
import { useTerminalOwnership } from "@/components/terminal/ownership"
import { plainBounce } from "@/components/terminal/plainBounce"
import {
  useViewerGrid,
} from "@/components/terminal/viewerGrid"
import { REPLAY_WAIT_POLL_MS } from "@/components/terminal/constants"
import { suspendTerminalTabStop } from "@/components/terminal/inputWiring"
import { registerPaneInputGroup } from "@/lib/paneInputGroup"
import {
  focusTypingSurfaceIn,
  nextTypingFocus,
  typingFocusAllowed,
  useInputSurface,
  type InputSurface,
} from "@/components/terminal/inputSurface"
import {
  useUploadPipeline,
  type UploadPipeline,
} from "@/components/terminal/uploadPipeline"
import type { TakeoverIntent } from "@/components/terminal/channels"
import { sessionLabel } from "@/lib/agentWorkspace"

// WHAT THE PANE PAINTS OVER ITS OWN TERMINAL. Today that is the floating
// theater pill, and the reason it comes in as a prop rather than being mounted
// by the shells beside the pane is geometric: the pane column holds the compose
// row and the terminal keys UNDER the terminal, so anything positioned against
// that column lands on the Send button. The terminal's own box is the only
// positioning context that means "over the terminal, and over nothing else".
type TerminalPaneOverlayProp = { overlay?: ReactNode }

type TerminalPaneProps = (
  // The streamed target: an agent tab, or a companion terminal of either owner.
  // `id` is the FOCUSED TAB id for an agent and the terminal id for a terminal.
  // The owner (session id for an agent, `TerminalOwnerRef` for a terminal) is
  // passed explicitly: it builds the nested PTY socket URL and the macro
  // target, and the spine may not yet list a just-created terminal when this
  // pane first mounts.
  //
  // `slotTabId` is the agent's slot tab as the spine names it, a generated id
  // the session merely points at. Slot-ness is decided against it, never
  // against the session id, so it must be passed by every caller that has the
  // spine; it is absent only while the spine has not arrived.
  | { kind: "agent"; id: string; sessionId: string; slotTabId?: string }
  | { kind: "terminal"; id: string; owner: TerminalOwnerRef }
) &
  TerminalPaneOverlayProp

export function TerminalPane(props: TerminalPaneProps) {
  const { kind, id } = props
  const {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    ptyRef,
    isMobile,
    pickerInput,
    openFilePicker,
    dragActive,
    setDragActive,
    dragDepthRef,
    offline,
    conn,
    terminalFontFamilySetting,
    terminalFontSizeSetting,
    fileDropEnabled,
    composeMode,
    composeBarEnabled,
    keysApply,
    directLeavesNothingBelow,
    accessoryBarVisible,
    session,
    hasOutput,
    providerName,
    spineInputOwner,
    composeInputRef,
    viewerOverflow,
    setViewerOverflow,
    liveSettingsFor,
    isSessionSlotTab,
  } = useTerminalPaneSetup(props)

  // True while the PTY socket has dropped and is retrying (non-blocking), or
  // while a take-over is deliberately bouncing it. Drives a "Reconnecting…"
  // overlay that re-arms even after `everReady` has latched, so a mid-session
  // disconnect is visible rather than the terminal silently freezing. Cleared on
  // the next (re)open. Input typed while disconnected is dropped by the socket's
  // readyState guard; this overlay is the signal that it would be. Declared
  // ABOVE the ownership machine because the take-over bounce raises it: a
  // deliberate `connect()` fires no `onReconnecting` of its own.
  const [reconnecting, setReconnecting] = useState(false)

  // THE ATTACH EPOCH AND ITS SCREEN. Every open of the socket mints an epoch
  // (see `terminal/attachReplay.ts`); the cover comes down only when the replay
  // for the CURRENT one has been parsed, and never merely because the socket
  // opened. `null` means no open has happened yet, which is covered too.
  const [attachEpoch, setAttachEpoch] = useState<number | null>(null)
  const [appliedEpoch, setAppliedEpoch] = useState<number | null>(null)
  const replayApplied = attachEpoch !== null && appliedEpoch === attachEpoch

  // THE REPLAY WAIT, in ACCUMULATED VISIBLE TIME (see `lib/visibleClock.ts`): a
  // hidden tab is throttled and a suspended one resumes believing hours passed,
  // so a wall-clock wait would offer a Reconnect button to a phone the moment it
  // came out of a pocket. Reset on every attach epoch, because each open's
  // patience starts from zero.
  const replayClockRef = useRef<VisibleClock | null>(null)
  if (replayClockRef.current === null) {
    replayClockRef.current = createVisibleClock()
  }
  const [replayWaitExpired, setReplayWaitExpired] = useState(false)
  useEffect(() => {
    const clock = replayClockRef.current
    return () => clock?.dispose()
  }, [])
  // Poll the visible clock while a cover is up with no screen behind it. A poll
  // rather than a timer because the quantity being waited on is visible time,
  // which a `setTimeout` cannot measure; the interval is a second, so the box
  // appears within a second of the configured wait. It runs only while there is
  // something to wait for, so a settled pane pays nothing.
  useEffect(() => {
    // Nothing to wait for. The flag is not cleared here (that would be a
    // setState inside an effect body, and a cascading render); it is cleared by
    // `noteAttachEpoch`, which is the moment a NEW wait begins, and the cover
    // ignores it entirely while the replay is applied.
    if (replayApplied) return
    const waitMs = replayWaitMs()
    // A configured zero disables the wait entirely: the cover stays up
    // indefinitely rather than ever offering the box.
    if (waitMs <= 0) return
    const check = () => {
      const elapsed = replayClockRef.current?.elapsedMs() ?? 0
      if (elapsed >= waitMs) setReplayWaitExpired(true)
    }
    check()
    const timer = setInterval(check, REPLAY_WAIT_POLL_MS)
    return () => clearInterval(timer)
  }, [replayApplied, attachEpoch])

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
    notePtyConn,
    handshakeSeen,
    takeOver,
  } = useTerminalOwnership({
    id,
    kind,
    conn,
    // Who the SPINE says drives this pty. It is the only thing that can correct
    // a device name kept across an events-socket outage, and it exists for a
    // companion terminal as well as an agent tab, so both cards get it.
    spineInputOwner,
    ptyRef,
    setReconnecting,
  })

  // THE LIVE-SETTINGS SNAPSHOT, published here rather than inside the setup
  // hook because `composeActive` is OWNER-GATED and the verdict is only known
  // now. The typing surfaces render for the input owner alone, so a watcher
  // with a stored `compose` choice has no message box: publishing `true` there
  // armed the focus guard over a pane with nothing to redirect into (every
  // redirect a no-op) and painted the permanently-focused caret xterm shows
  // while the box holds the keyboard. Still ahead of the relayout hook and of
  // the lifecycle, which is the ordering this container's contract requires.
  // IS THE MESSAGE BOX THE TYPING SURFACE IN THIS PANE, RIGHT NOW? The bar
  // renders for the input owner alone, so a watcher with a stored `compose`
  // choice has no box at all: everything downstream of this flag has to know
  // that, or it acts on a surface that is not on screen.
  const composeSurfaceLive = composeBarEnabled && isOwner
  const live = useTerminalLiveSettings(liveSettingsFor(composeSurfaceLive))

  // THE ONE FOCUS-ROUTING RULE, bound to this pane's three handles. It is a
  // standalone function in the input surface rather than a method of it,
  // because two things need it BEFORE the input-surface hook has run: the
  // ownership machine's take-over, and the lifecycle's focus-on-mount. Every
  // refocus in the pane goes through this one binding.
  const focusTypingSurface = () =>
    focusTypingSurfaceIn({ live, composeInputRef, termRef })

  // LOSING OWNERSHIP LEAVES THEATER, and forgets that this pane was ever in it.
  // Another device is driving now and the take-over card is about to cover the
  // pane: deciding what to do about that wants the tabs, the pull-request band
  // and the header back in view, and a covered pane has not earned the whole
  // screen. Only the LAYOUT comes back; losing ownership stays as sticky as it
  // ever was, and re-entering theater afterwards is a fresh press.
  //
  // On the TRANSITION only, never on the verdict itself. A backgrounded tab
  // attaches as a watcher without stealing anything, and clearing the memory of
  // a pane merely being observed would quietly undo a mode the user set up on
  // the device that is driving.
  //
  // AND NOT BEFORE THE FIRST HONEST VERDICT. `isOwner` is a foreground guess
  // until the handshake answers, so a watcher's first real answer looked exactly
  // like a demotion: a shared theater link opened on a device somebody else is
  // driving entered the mode, "lost" ownership it never had, and cleared the
  // pane's memory on the way out. `theaterOwnershipStep` is where that rule
  // lives, so it is testable without a socket.
  const ownerWatchRef = useRef(theaterOwnershipWatchStart)
  useEffect(() => {
    const step = theaterOwnershipStep(ownerWatchRef.current, {
      handshakeSeen,
      isOwner,
    })
    ownerWatchRef.current = step.state
    if (step.lost) noteTheaterOwnershipLost(kind, id)
  }, [handshakeSeen, isOwner, kind, id])

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
  // The same idiom for the owner's side: the relayout's font refit is a
  // coordinator act too, so it goes through this rather than the fit addon.
  const ownerRefitRef = useRef<(() => void) | null>(null)

  // THE INPUT SURFACE: the compose Send, the accessory sends, the sticky
  // modifier latches and the draft splice.
  const input = useInputSurface({
    live,
    composeInputRef,
    termRef,
    ptyRef,
    ownership,
    // The pane's own target id keys the draft in the store, so each agent tab
    // and each terminal keeps its own unsent message across a remount.
    targetId: props.id,
  })

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

  const viewerRelayoutRef = useTerminalRelayout({
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    viewerRegridRef,
    ownerRefitRef,
    setViewerOverflow,
    fontFamilySetting: terminalFontFamilySetting,
    fontSizeSetting: terminalFontSizeSetting,
    faithfulWatcher,
    remoteRows,
    remoteCols,
  })

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
  }, [dragActive, dragDepthRef])
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
    term.options.cursorInactiveStyle = inactiveCursorStyle(composeSurfaceLive)
  }, [composeSurfaceLive, termRef])
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

  // IS AN IME COMPOSITION IN FLIGHT? Moving focus mid-composition destroys the
  // half-typed CJK text and the candidate popup with it, so every automatic
  // focus move in this pane is gated on this being false. Tracked at the
  // document, because the composition may be in xterm's hidden textarea or in
  // the compose box and both are inside this pane.
  const composingRef = useRef(false)
  // Which attach the pane has already moved the keyboard for, as
  // `kind:id:composeBarEnabled`. Null when this pane does not own the pty.
  const focusedForRef = useRef<string | null>(null)
  // Bumped when a composition ENDS, so an automatic focus move the composition
  // blocked is retried rather than dropped. Without it the guard read the ref
  // once and the effect never ran again: a composition in flight when the replay
  // landed cancelled that pane's one focus move permanently.
  const [compositionEnded, setCompositionEnded] = useState(0)
  useEffect(() => {
    const start = () => {
      composingRef.current = true
    }
    const end = () => {
      composingRef.current = false
      setCompositionEnded((n) => n + 1)
    }
    document.addEventListener("compositionstart", start, true)
    document.addEventListener("compositionend", end, true)
    return () => {
      document.removeEventListener("compositionstart", start, true)
      document.removeEventListener("compositionend", end, true)
    }
  }, [])

  const {
    accessoryBarShown,
    composeBarShown: composeBarShownHere,
    inputMenuGates,
    menuHasItems,
    topInputGates,
  } = terminalInputLayout({
    isOwner,
    composeMode,
    keysApply,
    accessoryBarVisible,
    composeBarEnabled,
  })

  const everReady = useEverReady(hasOutput)
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
    ownerRefitRef,
    viewerRelayoutRef,
    live,
    mods: input.mods,
    ownership,
    connId,
    seedOwnershipFromConnected: seedFromConnected,
    noteRemotePtyGrid: viewerGrid.noteRemoteGrid,
    noteLocalGrid: viewerGrid.noteLocalGrid,
    noteSocketOpen: viewerGrid.noteSocketOpen,
    noteAttachEpoch: (epoch) => {
      setAttachEpoch(epoch)
      setReplayWaitExpired(false)
    },
    noteReplayApplied: (epoch) => setAppliedEpoch(epoch),
    notePtyConn,
    focusTypingSurface,
    onClipboardPaste: (e) => upload.onClipboardPaste(e),
    armForcedTextPaste: () => upload.armForcedTextPaste(),
    setReconnecting,
    resetReplayWait: () => replayClockRef.current?.reset(),
    isOwnerRendered: isOwner,
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
  //   [composeSurfaceLive]                the open terminal's inactive caret
  //   [isOwner, composeBarEnabled]        focus the freshly mounted compose box
  //   [composeBarEnabled, isOwner]        the compose-insert sink
  //   [live]                              the terminal focus target
  //   [composeBarEnabled, isOwner]        the compose textarea's paste listener
  //   [composeSurfaceLive]                xterm's tab stop, while the box is up
  //   [isOwner, ...the top gates]         the INPUT group the top menus show
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
  //
  // AND NOT BEFORE THE PANE HAS RECONCILED. Focusing summons the soft keyboard,
  // and doing that over a pane that is still resolving ownership or still
  // waiting for its screen puts a keyboard over a placeholder: the take-over
  // used to focus optimistically the moment it was pressed, a whole reconnect
  // and replay ahead of anything to type into. Both facts must be in before the
  // keyboard comes up, and an IME composition in flight is never interrupted by
  // it. The two rules are the pure `typingFocusAllowed` (may we?) and
  // `nextTypingFocus` (do we owe one?).
  //
  // ONCE PER ATTACH, NOT ONCE PER EPOCH. `replayApplied` goes false and back to
  // true on EVERY socket reopen, so an effect keyed on it alone fired on every
  // background reconnect: on a desktop that pulled focus out of whatever the user
  // was typing in, and on a phone it raised the soft keyboard unbidden. The pane
  // remembers what it has already focused for, so a reconnect onto the SAME
  // target with the SAME ownership and the SAME typing surface is silent, while
  // a target switch, a regained ownership, or the compose bar appearing still
  // moves the keyboard exactly once.
  useEffect(() => {
    const decision = nextTypingFocus({
      allowed: typingFocusAllowed({
        isOwner,
        ownershipConfirmed: handshakeSeen,
        replayApplied,
        composing: composingRef.current,
      }),
      isOwner,
      attach: `${kind}:${id}:${composeBarEnabled}`,
      focusedFor: focusedForRef.current,
    })
    focusedForRef.current = decision.focusedFor
    if (!decision.focus) return
    // THE ONE AUTOMATIC FOCUS MOVE IN THE PANE, for both surfaces. The routing
    // (compose textarea when the box is up, xterm's hidden textarea otherwise)
    // lives in `focusTypingSurface`, and the mount-time focus that used to sit
    // in the lifecycle is gone: two focus paths meant two rules.
    focusTypingSurface()
    // `focusTypingSurface` reads only refs, so its identity says nothing new and
    // listing it would re-focus on every commit. `compositionEnded` is listed so
    // a move an IME composition deferred is retried when it finishes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    isOwner,
    composeBarEnabled,
    handshakeSeen,
    replayApplied,
    compositionEnded,
    kind,
    id,
    composeInputRef,
    termRef,
  ])
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
  }, [live, composeInputRef, termRef])
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

  // THE KEYBOARD WAY OUT OF THE PANE while the box holds the typing surface.
  // The focus guard sends every focus landing in the terminal container to the
  // message box, which turned xterm's `tabindex="0"` helper textarea into a
  // trap: Shift-Tab out of the box landed there and was bounced straight back,
  // so backward navigation out of the pane was impossible. Taking it out of the
  // tab order is the answer that a pointer does not consult, so a click into
  // the terminal still hands focus to the box (see `suspendTerminalTabStop`).
  // Same gate as the box itself, and restored the moment it goes away.
  useEffect(() => {
    if (!composeSurfaceLive) return
    return suspendTerminalTabStop(termRef.current?.textarea)
    // The terminal is created by the lifecycle effect declared above this one,
    // so `termRef` is filled by the time this runs; it is listed for the linter
    // rather than because its identity ever changes.
  }, [composeSurfaceLive, termRef])

  // THE PANE'S INPUT GROUP, for whichever TOP menu is on screen over it: the
  // phone's merged pane menu, the phone's agentless terminal header, the
  // sidebar row's menu on a computer, the floating pill's one menu (see
  // `lib/paneInputGroup.ts`). None of them is inside the pane, and only the
  // pane knows the answers.
  //
  // Published only while this pane OWNS the input, so a mounted viewer pane
  // cannot shadow a mounted owner pane's answers for the same agent.
  //
  // Every dependency is a primitive: the gates object is rebuilt on each render,
  // and a registration keyed on its identity would publish on every commit,
  // re-render every open menu and come straight back round.
  const {
    surfaceSwitch: topSurfaceSwitch,
    keysToggle: topKeysToggle,
  } = topInputGates
  useEffect(() => {
    if (!isOwner) return
    return registerPaneInputGroup(id, {
      surfaceSwitch: topSurfaceSwitch,
      keysToggle: topKeysToggle,
    })
  }, [id, isOwner, topSurfaceSwitch, topKeysToggle])

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

  // There is deliberately no extra ping on gaining ownership any more. There is
  // exactly ONE periodic client frame and one timer behind it (see
  // `lib/heartbeat.ts`), and its cadence is already the viewed ping's 2s in
  // precisely the state a gain lands in (owner and visible). The gain RETIMES
  // that one timer rather than adding a second sender: the lifecycle resyncs the
  // beat when this verdict flips, so the gap already armed under the slow
  // cadence is cleared instead of waited out.

  const cover = attachCover({
    socket: connectionLost ? "failed" : reconnecting ? "connecting" : "open",
    replayApplied,
    everReady,
    offline,
    waitExpired: replayWaitExpired,
    isOwner,
    firstAttach: appliedEpoch === null,
  })

  const pane = (
    <TerminalPaneSurface
      kind={kind}
      dragActive={dragActive}
      setDragActive={setDragActive}
      dragDepthRef={dragDepthRef}
      viewerOverflow={viewerOverflow}
      hostRef={hostRef}
      pointerTypeRef={pointerTypeRef}
      containerRef={containerRef}
      pickerInput={pickerInput}
      cover={cover}
      upload={upload}
      input={input}
      providerName={providerName}
      ptyRef={ptyRef}
      takeoverIntent={takeoverIntent}
      takeoverLabel={takeoverLabel}
      ownerPresent={ownerPresent}
      takeOver={takeOver}
      overlay={props.overlay}
    />
  )

  return (
    <TerminalPaneLayout
      pane={pane}
      isOwner={isOwner}
      accessoryBarShown={accessoryBarShown}
      composeBarShown={composeBarShownHere}
      inputMenuGates={inputMenuGates}
      menuHasItems={menuHasItems}
      composeBarEnabled={composeBarEnabled}
      directLeavesNothingBelow={directLeavesNothingBelow}
      input={input}
      composeInputRef={composeInputRef}
      kind={kind}
    />
  )
}

type TerminalTargetIds = {
  sessionId: string | null
  projectId: string | null
}

function terminalTargetIds(props: TerminalPaneProps): TerminalTargetIds {
  return {
    sessionId:
      props.kind === "agent" ? props.sessionId : ownerSessionId(props.owner),
    projectId:
      props.kind === "terminal" ? ownerProjectId(props.owner) : null,
  }
}

function terminalTargetRecords(
  props: TerminalPaneProps,
  spine: DuxState["spine"],
  ids: TerminalTargetIds,
) {
  const session =
    ids.sessionId === null
      ? undefined
      : spine?.sessions.find((candidate) => candidate.id === ids.sessionId)
  const project =
    ids.projectId === null
      ? undefined
      : spine?.projects.find((candidate) => candidate.id === ids.projectId)
  const focusedTab =
    props.kind === "agent"
      ? session?.tabs.find((candidate) => candidate.id === props.id)
      : undefined
  const ownedTerminals =
    props.kind === "terminal"
      ? terminalsForOwner(spine?.terminals ?? [], props.owner)
      : undefined
  const spineTerminal =
    props.kind === "terminal"
      ? ownedTerminals?.find((candidate) => candidate.id === props.id)
      : undefined
  return { session, project, focusedTab, ownedTerminals, spineTerminal }
}

type TerminalTargetRecords = ReturnType<typeof terminalTargetRecords>

function terminalNotifyTitle(
  props: TerminalPaneProps,
  records: TerminalTargetRecords,
): string {
  if (props.kind === "agent") {
    return records.session ? sessionLabel(records.session) : "Agent"
  }
  return matchOwner(props.owner, {
    session: () =>
      records.session ? sessionLabel(records.session) : "Agent",
    project: () => records.project?.name || "Terminal",
    standalone: () => "Terminal",
  })
}

function terminalHasOutput(
  props: TerminalPaneProps,
  records: TerminalTargetRecords,
): boolean {
  if (props.kind === "agent") {
    return records.focusedTab?.has_output ?? records.session?.has_output ?? false
  }
  return (
    records.ownedTerminals?.find((terminal) => terminal.id === props.id)
      ?.has_output ?? false
  )
}

function terminalProviderName(
  kind: TerminalPaneProps["kind"],
  records: TerminalTargetRecords,
): string | undefined {
  if (kind === "agent") {
    return records.focusedTab?.provider ?? records.session?.provider
  }
  return records.session?.provider
}

function terminalSpineInputOwner(
  kind: TerminalPaneProps["kind"],
  records: TerminalTargetRecords,
): string | null | undefined {
  if (kind === "agent") {
    if (records.focusedTab === undefined) return undefined
    return records.focusedTab.input_owner ?? null
  }
  if (records.spineTerminal === undefined) return undefined
  return records.spineTerminal.input_owner ?? null
}

function terminalTargetView(
  props: TerminalPaneProps,
  spine: DuxState["spine"],
  ids: TerminalTargetIds,
) {
  const records = terminalTargetRecords(props, spine, ids)
  return {
    ...records,
    notifyTitle: terminalNotifyTitle(props, records),
    hasOutput: terminalHasOutput(props, records),
    providerName: terminalProviderName(props.kind, records),
    spineInputOwner: terminalSpineInputOwner(props.kind, records),
    // Slot-ness comes from the session record the spine published, never from
    // an id comparison: the slot tab's id is generated and is not the session
    // id. Before the spine arrives there is no session, and the answer is the
    // safe `false` (this only gates ejecting the user to the welcome screen).
    isSessionSlotTab:
      props.kind === "agent" &&
      !!records.session &&
      isFirstTab(records.session, props.id),
  }
}

function terminalBasePreferences(bootstrap: DuxState["bootstrap"]) {
  return {
    fontFamily: bootstrap?.terminal_font_family ?? "",
    fontSize: bootstrap?.terminal_font_size ?? 14,
    fileDropEnabled: (bootstrap?.file_drop_max_bytes ?? 0) > 0,
    pastedTextChars: bootstrap?.upload_pasted_text_chars ?? 0,
    scrollbackLines:
      bootstrap?.agent_scrollback_lines ?? DEFAULT_SCROLLBACK_LINES,
    copyOnSelect: bootstrap?.copy_on_select ?? true,
  }
}

function terminalLivePreferences(bootstrap: DuxState["bootstrap"]) {
  return {
    attentionGraceMs:
      (bootstrap?.attention_grace_seconds ??
        DEFAULT_ATTENTION_GRACE_SECONDS) * 1000,
    webNotifications: bootstrap?.web_notifications ?? true,
    hyperlinks: bootstrap?.hyperlinks ?? true,
    clipboardPassthrough: bootstrap?.clipboard_passthrough ?? "focused",
    configuredDropPaste: bootstrap?.provider_drop_paste,
  }
}

function terminalPreferences(bootstrap: DuxState["bootstrap"]) {
  return {
    ...terminalBasePreferences(bootstrap),
    ...terminalLivePreferences(bootstrap),
  }
}

type TerminalPreferences = ReturnType<typeof terminalPreferences>

function terminalTouchSettings(
  duxState: DuxState,
  isCoarsePointer: boolean,
  typingSurface: ReturnType<typeof useTypingSurface>,
) {
  const composeMode = composeBarMode(duxState.bootstrap?.compose_bar)
  return {
    composeMode,
    composeBarEnabled: composeBarShown(
      composeMode,
      isCoarsePointer,
      typingSurface,
    ),
    keysApply: terminalKeysApply(
      composeMode,
      isCoarsePointer,
      typingSurface,
    ),
    directLeavesNothingBelow: !bottomBarSurvivesDirect(
      composeMode,
      isCoarsePointer,
      mobileAccessoryBarVisible(duxState),
    ),
    accessoryBarVisible: mobileAccessoryBarVisible(duxState),
  }
}

type TerminalInputLayoutInputs = {
  isOwner: boolean
  composeMode: ReturnType<typeof composeBarMode>
  /// Do the terminal keys belong under this terminal at all on this device
  /// (see `terminalKeysApply`)? Independent of the message box.
  keysApply: boolean
  accessoryBarVisible: boolean
  composeBarEnabled: boolean
}

function terminalInputLayout(input: TerminalInputLayoutInputs) {
  // TWO ROWS, TWO SWITCHES, FOUR LEGAL STATES. The key row answers "keys a soft
  // keyboard cannot produce", the message box answers "where the text is
  // composed", and neither is the other's on-switch: keys with no box is how a
  // finger holds a virtual Ctrl while a physical keyboard types the letter.
  const accessoryBarShown =
    input.isOwner && input.keysApply && input.accessoryBarVisible
  const composeBarShown = input.isOwner && input.composeBarEnabled
  // THE BOTTOM `⋯`, which lives INSIDE the virtual input and nowhere else. It
  // carries only what is local to the rows around it: the surface switch, in
  // BOTH directions, and the keys toggle. "Attach a file…" moved up to the top
  // menu's INPUT group, which is on screen whether or not these rows are, and
  // the theater exit went with it: this menu is not a permanent surface any
  // more, so it cannot be anybody's guaranteed way back. Neither is a gate
  // here any more either, so no caller can reintroduce one by passing `true`.
  // It exists while ANY bottom row does, which is what makes it the way back
  // for a pane left with the keys alone; there is deliberately no minimal row
  // of its own, so a pane with neither row has nothing under its terminal and
  // the top menu is the way back instead.
  const bottomBarShown = accessoryBarShown || composeBarShown
  const inputMenuGates = {
    surfaceSwitch:
      input.isOwner &&
      inputMenuSurfaceSwitchOffered(input.composeMode) &&
      bottomBarShown,
    keysToggle: input.isOwner && input.keysApply && bottomBarShown,
  }
  const menuHasItems = inputMenuHasItems(inputMenuGates)
  // THE TOP MENU'S INPUT GROUP, published for whichever menu is over this pane
  // (see `lib/paneInputGroup.ts`). It carries a control exactly while the
  // bottom `⋯` does NOT, so the same row is never in two menus at once: the
  // bottom one is the way OUT of the virtual input while it is up, and this one
  // is the way BACK once it is gone. "Attach a file…" is not here because it is
  // not a gate: the menu borrows the pane's own attach capability, published
  // under this same pty id on exactly the same condition.
  const topInputGates = {
    surfaceSwitch:
      input.isOwner &&
      inputMenuSurfaceSwitchOffered(input.composeMode) &&
      !bottomBarShown,
    keysToggle: input.isOwner && input.keysApply && !bottomBarShown,
  }
  return {
    accessoryBarShown,
    composeBarShown,
    inputMenuGates,
    menuHasItems,
    topInputGates,
  }
}

function useEverReady(hasOutput: boolean): boolean {
  const [everReady, setEverReady] = useState(false)
  if (hasOutput && !everReady) setEverReady(true)
  return everReady
}

function terminalLiveSettings(
  preferences: TerminalPreferences,
  target: ReturnType<typeof terminalTargetView>,
  viewerOverflow: boolean,
  composeBarEnabled: boolean,
): TerminalLiveSettings {
  return {
    scrollbackLines: preferences.scrollbackLines,
    copyOnSelect: preferences.copyOnSelect,
    fontFamily: preferences.fontFamily,
    fontSize: preferences.fontSize,
    fileDropEnabled: preferences.fileDropEnabled,
    pastedTextChars: preferences.pastedTextChars,
    attentionGraceMs: preferences.attentionGraceMs,
    webNotifications: preferences.webNotifications,
    hyperlinks: preferences.hyperlinks,
    clipboardPassthrough: preferences.clipboardPassthrough,
    notifyTitle: target.notifyTitle,
    providerName: target.providerName,
    configuredDropPaste: preferences.configuredDropPaste,
    launchedDropPaste: target.focusedTab?.drop_paste,
    sessionTabs: target.session?.tabs,
    viewerOverflow,
    composeActive: composeBarEnabled,
  }
}

function useTerminalPaneSetup(props: TerminalPaneProps) {
  const ids = terminalTargetIds(props)
  const hostRef = useRef<HTMLDivElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const ptyRef = useRef<PtySocket | null>(null)
  const isMobile = useIsMobile()
  const isCoarsePointer = useIsCoarsePointer()
  const typingSurface = useTypingSurface()
  const { input: pickerInput, open: openFilePicker } = useFilePicker()
  const [dragActive, setDragActive] = useState(false)
  const dragDepthRef = useRef(0)
  const duxState = useDux()
  const { spine, bootstrap, offline, conn } = duxState
  const preferences = terminalPreferences(bootstrap)
  const touch = terminalTouchSettings(
    duxState,
    isCoarsePointer,
    typingSurface,
  )
  const target = terminalTargetView(props, spine, ids)
  const composeInputRef = useRef<HTMLTextAreaElement | null>(null)
  const [viewerOverflow, setViewerOverflow] = useState(false)
  // The snapshot is BUILT here and PUBLISHED in the component body, because one
  // of its fields is not known yet: whether the message box is actually the
  // typing surface depends on owning the input, and the ownership machine runs
  // after this hook. See the publish site for what that gate is worth.
  const liveSettingsFor = (composeActive: boolean) =>
    terminalLiveSettings(preferences, target, viewerOverflow, composeActive)

  return {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    ptyRef,
    isMobile,
    pickerInput,
    openFilePicker,
    dragActive,
    setDragActive,
    dragDepthRef,
    offline,
    conn,
    terminalFontFamilySetting: preferences.fontFamily,
    terminalFontSizeSetting: preferences.fontSize,
    fileDropEnabled: preferences.fileDropEnabled,
    ...touch,
    session: target.session,
    hasOutput: target.hasOutput,
    providerName: target.providerName,
    spineInputOwner: target.spineInputOwner,
    composeInputRef,
    viewerOverflow,
    setViewerOverflow,
    liveSettingsFor,
    isSessionSlotTab: target.isSessionSlotTab,
  }
}

type TerminalPaneSurfaceProps = {
  kind: TerminalPaneProps["kind"]
  dragActive: boolean
  setDragActive: (active: boolean) => void
  dragDepthRef: RefObject<number>
  viewerOverflow: boolean
  hostRef: RefObject<HTMLDivElement | null>
  pointerTypeRef: RefObject<string>
  containerRef: RefObject<HTMLDivElement | null>
  pickerInput: ReactNode
  cover: AttachCover
  upload: UploadPipeline
  input: InputSurface
  providerName?: string
  ptyRef: RefObject<PtySocket | null>
  takeoverIntent: TakeoverIntent
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
  overlay: ReactNode
}

function TerminalPaneSurface({
  kind,
  dragActive,
  setDragActive,
  dragDepthRef,
  viewerOverflow,
  hostRef,
  pointerTypeRef,
  containerRef,
  pickerInput,
  cover,
  upload,
  input,
  providerName,
  ptyRef,
  takeoverIntent,
  takeoverLabel,
  ownerPresent,
  takeOver,
  overlay,
}: TerminalPaneSurfaceProps) {
  return (
    <div
      className="group relative min-h-0 w-full flex-1 overflow-hidden bg-background"
      onDragEnter={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        event.preventDefault()
        dragDepthRef.current += 1
        setDragActive(true)
      }}
      onDragOver={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        // Browsers refuse a drop unless dragover cancels their default navigation.
        event.preventDefault()
        event.dataTransfer.dropEffect = "copy"
      }}
      onDragLeave={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
        if (dragDepthRef.current === 0) setDragActive(false)
      }}
      onDrop={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        event.preventDefault()
        dragDepthRef.current = 0
        setDragActive(false)
        void upload.runUpload(
          Array.from(event.dataTransfer.files),
          upload.activeUploadSink(),
        )
      }}
    >
      {dragActive ? <FileDropOverlay kind={kind} /> : null}
      <div
        ref={hostRef}
        className={
          viewerOverflow ? "h-full w-full overflow-auto p-2" : "h-full w-full p-2"
        }
        onPointerDown={(event) => {
          pointerTypeRef.current = event.pointerType
        }}
        onContextMenu={(event) => {
          if (pointerTypeRef.current === "touch") {
            event.preventDefault()
            return
          }
          event.preventDefault()
          input.onRightClickPaste()
        }}
      >
        <div
          ref={containerRef}
          data-testid="terminal-container"
          className="h-full w-full [-webkit-touch-callout:none]"
        />
      </div>
      {pickerInput}
      <TerminalCover
        cover={cover}
        kind={kind}
        providerName={providerName}
        ptyRef={ptyRef}
        takeoverIntent={takeoverIntent}
        takeoverLabel={takeoverLabel}
        ownerPresent={ownerPresent}
        takeOver={takeOver}
      />
      {coverOwnsThePane(cover) ? null : overlay}
    </div>
  )
}

/// Does the cover speak for the whole pane, or is it a cue over one the user
/// still has?
///
/// The floating overlay (theater's pill) is chrome for a surface somebody is
/// using. A CARD says another device is driving this pty and a BOX says the
/// picture is gone: both are full-pane, opaque, and carry the one control that
/// answers them, so a pill on top of either offers a drag over a surface there
/// is nothing to uncover on, and its one-time hint teaches the gesture to
/// somebody who cannot see what it is for. The SPINNER is deliberately the other
/// way: a small, transparent, pointer-transparent cue over a pane that is still
/// this user's, and a blip must not take the way out of theater with it.
function coverOwnsThePane(cover: AttachCover): boolean {
  switch (cover.kind) {
    case "card":
    case "box":
      return true
    case "spinner":
    case "none":
      return false
    default:
      return assertNever(cover)
  }
}

function FileDropOverlay({ kind }: { kind: TerminalPaneProps["kind"] }) {
  return (
    <div
      data-testid="file-drop-overlay"
      className="pointer-events-none absolute inset-2 z-20 flex flex-col items-center justify-center gap-1 rounded-lg border-2 border-dashed border-primary bg-background/90 p-4 text-center"
    >
      <p className="text-sm font-medium text-foreground">
        Drop to save the file and paste its path
      </p>
      <p className="text-xs text-muted-foreground">
        {kind === "agent"
          ? "It lands in this agent's upload folder, hidden from git and removed with the agent."
          : "It lands in the folder this terminal is currently in."}
      </p>
    </div>
  )
}

type TerminalCoverProps = {
  cover: AttachCover
  kind: TerminalPaneProps["kind"]
  providerName?: string
  ptyRef: RefObject<PtySocket | null>
  takeoverIntent: TakeoverIntent
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
}

function TerminalCover(props: TerminalCoverProps) {
  switch (props.cover.kind) {
    case "none":
      return null
    case "box":
      return (
        <div className="absolute inset-0 z-20 flex items-center justify-center bg-background">
          <div className="flex items-center gap-3 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <span className="text-sm text-muted-foreground">
              {props.cover.reason === "lost"
                ? "Connection lost."
                : "Still waiting for the terminal's screen."}
            </span>
            <Button
              size="sm"
              variant="secondary"
              onClick={() =>
                plainBounce(props.ptyRef.current, props.takeoverIntent)
              }
            >
              Reconnect
            </Button>
          </div>
        </div>
      )
    case "spinner":
      return (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <GlyphSpinner className="text-primary" />
            <span className="text-sm text-muted-foreground">
              {terminalCoverText(props.cover.wording, props.kind, props.providerName)}
            </span>
          </div>
        </div>
      )
    case "card":
      return (
        <TakeoverCard
          kind={props.kind}
          takeoverLabel={props.takeoverLabel}
          ownerPresent={props.ownerPresent}
          takeOver={props.takeOver}
        />
      )
  }
}

function terminalCoverText(
  wording: "starting" | "attaching" | "reconnecting",
  kind: TerminalPaneProps["kind"],
  providerName?: string,
): string {
  if (wording === "reconnecting") return "Reconnecting…"
  if (wording === "attaching") return "Attaching…"
  return kind === "agent"
    ? "Starting " + (providerName ?? "agent") + "…"
    : "Launching terminal…"
}

function TakeoverCard({
  kind,
  takeoverLabel,
  ownerPresent,
  takeOver,
}: {
  kind: TerminalPaneProps["kind"]
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
}) {
  const title = takeoverLabel
    ? "Active on " + takeoverLabel
    : ownerPresent
      ? "Active on another device"
      : "Running in the background"
  return (
    <div className="absolute inset-0 z-20 flex items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm text-center">
        <CardHeader className="items-center gap-3">
          <MonitorSmartphone className="size-8 text-muted-foreground" />
          <CardTitle>{title}</CardTitle>
          <CardDescription>
            {ownerPresent ? (
              <>
                Only one device can type at a time. Take over to drive this{" "}
                {kind === "agent" ? "agent" : "terminal"} from here.
              </>
            ) : (
              <>
                The device driving this {kind === "agent" ? "agent" : "terminal"}{" "}
                disconnected, so we kept the{" "}
                {kind === "agent" ? "agent" : "terminal"} running in the
                background to avoid losing any progress. Take over to drive it
                from here.
              </>
            )}
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
  )
}

type TerminalPaneLayoutProps = {
  pane: ReactNode
  isOwner: boolean
  accessoryBarShown: boolean
  composeBarShown: boolean
  inputMenuGates: InputMenuGates
  menuHasItems: boolean
  composeBarEnabled: boolean
  directLeavesNothingBelow: boolean
  input: InputSurface
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  kind: TerminalPaneProps["kind"]
}

function TerminalPaneLayout({
  pane,
  isOwner,
  accessoryBarShown,
  composeBarShown,
  inputMenuGates,
  menuHasItems,
  composeBarEnabled,
  directLeavesNothingBelow,
  input,
  composeInputRef,
  kind,
}: TerminalPaneLayoutProps) {
  const inputMenu = (
    <InputMenu
      gates={inputMenuGates}
      composeSurface={composeBarEnabled}
      directLeavesNothingBelow={directLeavesNothingBelow}
    />
  )

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {pane}
      {isOwner ? (
        <TerminalInputRows
          accessoryBarShown={accessoryBarShown}
          composeBarShown={composeBarShown}
          menuHasItems={menuHasItems}
          input={input}
          composeInputRef={composeInputRef}
          kind={kind}
          inputMenu={inputMenu}
        />
      ) : null}
    </div>
  )
}

type TerminalInputRowsProps = {
  accessoryBarShown: boolean
  composeBarShown: boolean
  menuHasItems: boolean
  input: InputSurface
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  kind: TerminalPaneProps["kind"]
  inputMenu: ReactNode
}

function TerminalInputRows({
  accessoryBarShown,
  composeBarShown,
  menuHasItems,
  input,
  composeInputRef,
  kind,
  inputMenu,
}: TerminalInputRowsProps) {
  return (
    <>
      {accessoryBarShown ? (
        <AccessoryBar
          onEsc={() => input.sendSeq(ESC)}
          onTab={() => input.sendSeq(TAB)}
          onNewline={input.sendNewline}
          onArrow={input.onArrow}
          onScroll={input.onScroll}
          ctrl={input.ctrl}
          alt={input.alt}
          onToggleCtrl={input.toggleCtrl}
          onToggleAlt={input.toggleAlt}
          inputMenu={!composeBarShown && menuHasItems ? inputMenu : undefined}
        />
      ) : null}
      {composeBarShown ? (
        <ComposeBar
          value={input.composeText}
          onChange={input.setComposeText}
          onSend={input.sendCompose}
          inputRef={composeInputRef}
          onForwardKey={input.sendSeq}
          placeholder={
            kind === "agent" ? AGENT_PLACEHOLDER : TERMINAL_PLACEHOLDER
          }
          leading={menuHasItems ? inputMenu : undefined}
        />
      ) : null}
    </>
  )
}
