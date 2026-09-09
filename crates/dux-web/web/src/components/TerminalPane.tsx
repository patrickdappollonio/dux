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
import { exitEjectsToWelcome, isFirstTab } from "@/lib/agentTabs"
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

// Painted over the terminal's own box, which is the only positioning context
// that excludes the compose row and the key rows sitting under the terminal.
type TerminalPaneOverlayProp = { overlay?: ReactNode }

type TerminalPaneProps = (
  // The streamed target. `id` is the focused tab id for an agent and the terminal
  // id for a terminal; `slotTabId` names the slot tab, never the session id.
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
    lastRunFailed,
  } = useTerminalPaneSetup(props)

  // True while the PTY socket is retrying, or while a take-over bounces it.
  // Declared above the ownership machine, which raises it on a deliberate bounce.
  const [reconnecting, setReconnecting] = useState(false)

  // The cover comes down only when the replay for the CURRENT epoch has been
  // parsed, never merely because the socket opened. `null` means no open yet.
  const [attachEpoch, setAttachEpoch] = useState<number | null>(null)
  const [appliedEpoch, setAppliedEpoch] = useState<number | null>(null)
  const replayApplied = attachEpoch !== null && appliedEpoch === attachEpoch

  // The replay wait counts accumulated VISIBLE time (`lib/visibleClock.ts`): a
  // wall clock expires while the tab sits hidden. Reset on every attach epoch.
  const replayClockRef = useRef<VisibleClock | null>(null)
  if (replayClockRef.current === null) {
    replayClockRef.current = createVisibleClock()
  }
  const [replayWaitExpired, setReplayWaitExpired] = useState(false)
  useEffect(() => {
    const clock = replayClockRef.current
    return () => clock?.dispose()
  }, [])
  // Polled rather than timed: visible time is what is being waited on, and a
  // `setTimeout` cannot measure it. Runs only while a cover has no screen behind it.
  useEffect(() => {
    // The flag is cleared by `noteAttachEpoch`, where a new wait begins: clearing
    // it here would be a setState in an effect body and a cascading render.
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

  // The ownership states, the verdict channel, the connection identity and the
  // take-over intent all live in one module, `terminal/ownership.ts`.
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
    // Who the spine says drives this pty: the only thing that can correct a device
    // name kept across an events-socket outage, for a terminal as well as a tab.
    spineInputOwner,
    ptyRef,
    setReconnecting,
  })

  // Is the message box the typing surface in this pane right now? The bar renders
  // for the input owner alone, so a watcher with a stored choice has no box.
  const composeSurfaceLive = composeBarEnabled && isOwner
  const live = useTerminalLiveSettings(liveSettingsFor(composeSurfaceLive))

  // Every refocus in the pane goes through this one binding. It is standalone
  // because the take-over and the lifecycle need it before the input hook runs.
  const focusTypingSurface = () =>
    focusTypingSurfaceIn({ live, composeInputRef, termRef })

  // Losing ownership leaves theater and forgets the mode, on the TRANSITION only
  // and never before the handshake's first honest verdict (`theaterOwnershipStep`).
  const ownerWatchRef = useRef(theaterOwnershipWatchStart)
  useEffect(() => {
    const step = theaterOwnershipStep(ownerWatchRef.current, {
      handshakeSeen,
      isOwner,
    })
    ownerWatchRef.current = step.state
    if (step.lost) noteTheaterOwnershipLost(kind, id)
  }, [handshakeSeen, isOwner, kind, id])

  // One PTY has one authoritative grid, the owner's. A diverged viewer heals by
  // re-attaching, never by resizing the PTY, which would be a silent steal.
  const viewerGrid = useViewerGrid({
    ptyRef,
    ownership,
    takeoverIntent,
    setReconnecting,
  })
  // A watcher renders at the PTY's grid, with no preference behind it. The
  // coordinator derives the same answer off the verdict channel, synchronously.
  const faithfulWatcher = !isOwner
  // The grid to render, broken out so the relayout effect depends on the
  // NUMBERS rather than on the object identity the machine hands back.
  const remoteRows = viewerGrid.remoteGrid?.rows ?? 0
  const remoteCols = viewerGrid.remoteGrid?.cols ?? 0
  // The mount-scoped port onto the coordinator's grid adoption: a viewer re-grid
  // is a coordinator act, never a side effect of a font change.
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

  // The upload pipeline: the file journey every gesture (drop, paste, picker)
  // shares, its sinks, its batch loop and its one toast.
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

  // Retire an in-flight drag on either flip of file-drop availability: the gate
  // refuses the `dragleave` or `drop` that would otherwise clear the overlay.
  const [dropEnabledSeen, setDropEnabledSeen] = useState(fileDropEnabled)
  if (dropEnabledSeen !== fileDropEnabled) {
    setDropEnabledSeen(fileDropEnabled)
    setDragActive(false)
  }
  // The depth counter means nothing outside an active drag, so it is pinned to
  // zero; in an effect, because a ref must not be written during render.
  useEffect(() => {
    if (!dragActive) dragDepthRef.current = 0
  }, [dragActive, dragDepthRef])
  // The typing surface changes mid-session, and xterm options are mutable in
  // place (installed 6.0.0: only `cols` and `rows` are read-only).
  useEffect(() => {
    const term = termRef.current
    if (!term) return
    term.options.cursorInactiveStyle = inactiveCursorStyle(composeSurfaceLive)
  }, [composeSurfaceLive, termRef])
  // Tracks the attention-grace hidden -> visible transition (see
  // `visibleSinceAfterTransition` in viewedPing.ts). `undefined` means none seen.
  const visibleSinceRef = useRef<number | undefined>(undefined)
  const prevVisibleRef = useRef<boolean | undefined>(undefined)

  // Android Chrome fires `contextmenu` on a touch long-press, which is the
  // selection gesture, so right-click paste is gated on a mouse or pen press.
  const pointerTypeRef = useRef("")

  // Moving focus mid-composition destroys half-typed CJK text, so every automatic
  // focus move is gated on this. At the document: either textarea may hold it.
  const composingRef = useRef(false)
  // Which attach the pane has already moved the keyboard for, as
  // `kind:id:composeBarEnabled`. Null when this pane does not own the pty.
  const focusedForRef = useRef<string | null>(null)
  // Bumped when a composition ends, so an automatic focus move it blocked is
  // retried rather than dropped.
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
  // The one lifecycle owner: it creates and tears down the terminal and socket,
  // re-running only when the streamed target changes.
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

  // The pane's one automatic typing focus: once per attach, and never before the
  // ownership verdict and the replay are in, or while an IME composition runs.
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
    focusTypingSurface()
    // `focusTypingSurface` reads only refs, so listing it would re-focus on every
    // commit; `compositionEnded` is listed so a deferred move is retried.
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
  // While the bar renders, a picked macro is spliced into the compose DRAFT at
  // the caret instead of being written to the PTY (`composeInsert.ts`).
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
    // `insertComposeText` reads only refs and the stable setter, so listing it
    // would re-register the sink on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeBarEnabled, isOwner])
  // The desktop macro picker lives in the header, outside this pane, so it needs
  // a registered close-focus target; the surface is resolved at call time.
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
  // The compose bar renders outside the terminal container, so the container's
  // capture listener cannot see a paste that lands in the box.
  useEffect(() => {
    if (!(composeBarEnabled && isOwner)) return
    const el = composeInputRef.current
    if (el === null) return
    const handler = (e: ClipboardEvent) => upload.onClipboardPaste(e)
    el.addEventListener("paste", handler)
    return () => el.removeEventListener("paste", handler)
    // `onClipboardPaste` reads refs and the live bootstrap at call time, so
    // re-registering on every identity change would buy nothing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeBarEnabled, isOwner])

  // The focus guard would bounce a Shift-Tab out of the box straight back, so
  // xterm's tab stop is suspended: the tab order is what a pointer does not consult.
  useEffect(() => {
    if (!composeSurfaceLive) return
    return suspendTerminalTabStop(termRef.current?.textarea)
    // `termRef` is filled by the lifecycle effect above; it is listed for the
    // linter rather than because its identity changes.
  }, [composeSurfaceLive, termRef])

  // The pane's INPUT group for whichever top menu is over it, published only
  // while this pane owns the input. Deps stay primitive: gates is rebuilt each render.
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

  // Eject to the welcome screen only when the slot tab stops and the whole agent
  // leaves `active`; a badly ended run stays put as its diagnosis surface.
  const sessionStatus = session?.status
  useEffect(() => {
    if (exitEjectsToWelcome(isSessionSlotTab, everReady, sessionStatus, lastRunFailed)) {
      // Marked as OUR eject so a re-armed reconnect deep-link can tell it from a
      // deliberate home nav and restore the route.
      ejectSelectionForReconnect()
    }
  }, [isSessionSlotTab, everReady, sessionStatus, lastRunFailed])

  // There is one periodic client frame and one timer behind it (`lib/heartbeat.ts`);
  // gaining ownership retimes that timer rather than adding a second sender.

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
    // Slot-ness comes from the session record, never an id comparison: the slot
    // tab's id is generated. Before the spine arrives the safe answer is false.
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
  // Two rows, two switches, four legal states: neither is the other's on-switch,
  // and keys with no box is a finger on a virtual Ctrl beside a real keyboard.
  const accessoryBarShown =
    input.isOwner && input.keysApply && input.accessoryBarVisible
  const composeBarShown = input.isOwner && input.composeBarEnabled
  // The bottom `⋯` lives inside the virtual input, so it carries only what is
  // local to those rows and exists exactly while any bottom row does.
  const bottomBarShown = accessoryBarShown || composeBarShown
  const inputMenuGates = {
    surfaceSwitch:
      input.isOwner &&
      inputMenuSurfaceSwitchOffered(input.composeMode) &&
      bottomBarShown,
    keysToggle: input.isOwner && input.keysApply && bottomBarShown,
  }
  const menuHasItems = inputMenuHasItems(inputMenuGates)
  // The top menu's INPUT group carries a control exactly while the bottom `⋯`
  // does not, so the same row is never in two menus at once.
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
  // Built here and published in the component body: whether the message box is
  // the typing surface depends on ownership, which resolves after this hook.
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
    // The server's verdict on this pane's own tab: its last run ended badly, so
    // the dormant card is what the user should meet when the process goes.
    lastRunFailed: target.focusedTab?.last_run_failed === true,
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

/// Whether the cover speaks for the whole pane. A card or a box is full-pane and
/// opaque, so the theater pill is withheld; the transparent spinner keeps it.
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
      // The message box is the only other row, so hiding the keys leaves nothing
      // below exactly when the box is not up.
      keysHideLeavesNothingBelow={!composeBarShown}
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
