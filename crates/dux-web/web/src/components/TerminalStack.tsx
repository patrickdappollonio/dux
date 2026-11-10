import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react"
import { createPortal } from "react-dom"
import { outsideGate } from "@/lib/gateLayer"
import { inactiveCursorStyle } from "@/lib/composebar"
import { stackTypingTarget } from "@/lib/layoutModel"
import { slotTabEndedInView } from "@/lib/agentTabs"
import { noteTheaterOwnershipLost, settleSlotTabExit } from "@/lib/store"
import type { TerminalOwnerRef } from "@/lib/store"
import { theaterOwnershipStep, theaterOwnershipWatchStart } from "@/lib/theater"
import { attachCover, coverOwnsThePane } from "@/lib/attachCover"
import { replayWaitMs } from "@/lib/connectionTiming"
import { createVisibleClock, type VisibleClock } from "@/lib/visibleClock"
import { useTerminalLiveSettings } from "@/components/terminal/liveValues"
import { useTerminalLifecycle } from "@/components/terminal/useTerminalLifecycle"
import { useTerminalRelayout } from "@/components/terminal/useTerminalRelayout"
import { useTerminalOwnership } from "@/components/terminal/ownership"
import { useViewerGrid } from "@/components/terminal/viewerGrid"
import { REPLAY_WAIT_POLL_MS } from "@/components/terminal/constants"
import { suspendTerminalTabStop } from "@/components/terminal/inputWiring"
import {
  registerPaneCover,
  useAnyPaneCoverOwned,
  usePaneCoverOwned,
} from "@/lib/paneCover"
import { registerPaneMacroTarget } from "@/lib/paneMacroTarget"
import { registerPaneInputGroup } from "@/lib/paneInputGroup"
import {
  focusTypingSurfaceIn,
  nextTypingFocus,
  typingFocusAllowed,
  useInputSurface,
} from "@/components/terminal/inputSurface"
import { useUploadPipeline } from "@/components/terminal/uploadPipeline"
import {
  TerminalCover,
  TerminalInputRows,
  TerminalPaneSurface,
} from "@/components/terminal/partPieces"
import {
  terminalInputLayout,
  useHandoffGrace,
  useLatched,
  useTerminalPaneSetup,
} from "@/components/terminal/partSetup"

/// One part of a stack: one live terminal, with its own socket, ownership,
/// upload sink, paste handling and IME guard.
export type TerminalPartProps = (
  // The streamed target. `id` is the focused tab id for an agent and the terminal
  // id for a terminal; `slotTabId` names the slot tab, never the session id.
  | { kind: "agent"; id: string; sessionId: string; slotTabId?: string }
  | { kind: "terminal"; id: string; owner: TerminalOwnerRef }
) & {
  /// Whether this part is on screen. A part off screen stays mounted in a real
  /// box the size of the stack, so its child keeps a usable grid and showing
  /// it again costs no reattach; it is not looked at, and it never makes the
  /// first claim of a row. Absent means on screen.
  onScreen?: boolean
}

/// What the stack tells its parts: which part typing goes to, how a part
/// becomes that part, and where the one input area is.
type StackContextValue = {
  typingTarget: string | null
  chooseTypingTarget: (ptyId: string) => void
  inputArea: HTMLElement | null
  /// Where a take-over card covering the whole stack goes, when the stack has
  /// more than one part.
  coverArea: HTMLElement | null
}

const StackContext = createContext<StackContextValue | null>(null)

/// A part as the stack is handed it: its props, and the slot it is laid out
/// in. The slot keys the part's component, so a part whose sibling comes or
/// goes (an agent's tab turning into its card and back) keeps its connection.
export type StackPartSpec = TerminalPartProps & {
  slot?: string
  /** Changing it replaces the part's component, as a reconnect asks for. */
  instance?: string
}

/// Lays the rendered parts out, by slot, inside the stack's box. Given the
/// part typing goes to, so a layout can mark the other one.
export type StackArrange = (
  parts: Readonly<Record<string, ReactNode>>,
  typingTarget: string | null,
) => ReactNode

/// An agent and its docked terminal (or any one terminal) rendered as one unit.
/// Each part keeps everything about its own connection; the stack owns what is
/// shared: which part typing goes to, the one input area under the parts, and
/// the one overlay layer over them.
///
/// The typing target is the caller's when it passes one (the agent's stack
/// keeps it in the store, where the header and the top menu read it), and the
/// stack's own otherwise.
export function TerminalStack({
  parts,
  overlay,
  arrange,
  typingTarget: chosenByCaller,
  onChooseTypingTarget,
}: {
  parts: StackPartSpec[]
  overlay?: ReactNode
  arrange?: StackArrange
  typingTarget?: string | null
  onChooseTypingTarget?: (ptyId: string) => void
}) {
  const [chosenHere, setChosenHere] = useState<string | null>(null)
  const controlled = onChooseTypingTarget !== undefined
  const chosen = controlled ? (chosenByCaller ?? null) : chosenHere
  const chooseTypingTarget = controlled ? onChooseTypingTarget : setChosenHere
  const typingTarget = stackTypingTarget(parts, chosen)
  // Held in state so the parts re-render once it exists and can portal their
  // input rows into it.
  const [inputArea, setInputArea] = useState<HTMLElement | null>(null)
  const [coverArea, setCoverArea] = useState<HTMLElement | null>(null)
  const context = useMemo<StackContextValue>(
    () => ({ typingTarget, chooseTypingTarget, inputArea, coverArea }),
    [typingTarget, chooseTypingTarget, inputArea, coverArea],
  )
  // A cover that owns the typing target's part withholds the layer whole; the
  // take-over cards are row-wide, so the parts of a row always agree on them.
  const layerWithheld = usePaneCoverOwned(typingTarget)
  // A cover over the OTHER part (its own socket reconnecting) leaves the
  // target's part usable, so the layer stays and shrinks to that part: a pill
  // resting over a covered terminal could neither be read nor reach it.
  const otherCovered = useAnyPaneCoverOwned(
    parts.map((part) => part.id).filter((id) => id !== typingTarget),
  )
  const boxRef = useRef<HTMLDivElement | null>(null)
  const bounds = usePartBounds(boxRef, otherCovered ? typingTarget : null)
  const rendered: Record<string, ReactNode> = {}
  parts.forEach((part, index) => {
    const { slot, instance, ...props } = part
    const key = slot ?? String(index)
    // Keyed by slot, never by pty id: a part switching tab or terminal keeps
    // its component, as a lone pane always has, unless its caller names a new
    // instance.
    rendered[key] = <TerminalPart key={`${key}:${instance ?? ""}`} {...props} />
  })
  return (
    <StackContext.Provider value={context}>
      <div className="flex h-full w-full flex-col bg-background">
        <div ref={boxRef} className="relative flex min-h-0 w-full flex-1 flex-col">
          {arrange ? arrange(rendered, typingTarget) : Object.values(rendered)}
          {/* A row is driven or watched whole, so a take-over card is one card
            * over every part with one button, painted here by the typing
            * target's part. Above the parts, below the overlay layer, which a
            * card withholds anyway. */}
          {parts.length > 1 ? (
            <div
              ref={setCoverArea}
              data-testid="stack-cover-area"
              className="pointer-events-none absolute inset-0 z-20 empty:hidden *:pointer-events-auto"
            />
          ) : null}
          {/* Withheld whole under a full-pane cover, layer included, so nothing of
            * the pill can paint over a cover or over the band it brings back.
            * The layer admits exactly the one pixel row above the stack, which is
            * the band's hairline the docked flap hangs over, so the phone's pill
            * covers it at the handoff the way the flap does; every other edge
            * stays clipped, so the pill's shadow never spills onto the input
            * rows or a neighbouring panel. Pointer events pass through the layer
            * to the terminals and land only on what it carries. */}
          {layerWithheld || overlay == null ? null : (
            <div
              data-testid="pane-overlay-layer"
              className="pointer-events-none absolute inset-0 z-30 [clip-path:polygon(0_-1px,100%_-1px,100%_100%,0_100%)] *:pointer-events-auto"
              data-bounds={bounds ? "part" : "stack"}
              style={
                bounds
                  ? { ...bounds, right: "auto", bottom: "auto" }
                  : undefined
              }
            >
              {overlay}
            </div>
          )}
        </div>
        {/* The one input area: the typing target's part renders its rows here. */}
        <div ref={setInputArea} data-testid="stack-input-area" className="contents" />
      </div>
    </StackContext.Provider>
  )
}

type PartBounds = { top: number; left: number; width: number; height: number }

/// Where one part's box sits inside the stack's, tracked through resizes, or
/// null when no part is asked for.
function usePartBounds(
  boxRef: RefObject<HTMLDivElement | null>,
  partId: string | null,
): PartBounds | null {
  const [bounds, setBounds] = useState<PartBounds | null>(null)
  useLayoutEffect(() => {
    const box = boxRef.current
    const part =
      partId === null || box === null
        ? null
        : (Array.from(box.querySelectorAll<HTMLElement>("[data-stack-part]")).find(
            (el) => el.dataset.stackPart === partId,
          ) ?? null)
    if (box === null || part === null) {
      setBounds(null)
      return
    }
    const measure = () => {
      const outer = box.getBoundingClientRect()
      const inner = part.getBoundingClientRect()
      setBounds((prev) => {
        const next = {
          top: inner.top - outer.top,
          left: inner.left - outer.left,
          width: inner.width,
          height: inner.height,
        }
        return prev !== null &&
          prev.top === next.top &&
          prev.left === next.left &&
          prev.width === next.width &&
          prev.height === next.height
          ? prev
          : next
      })
    }
    measure()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(box)
    observer.observe(part)
    return () => observer.disconnect()
  }, [boxRef, partId])
  return partId === null ? null : bounds
}

function TerminalPart(props: TerminalPartProps) {
  const { kind, id } = props
  const stack = useContext(StackContext)
  // Outside a stack (never in the app) a part is its own typing target.
  const isTypingTarget = stack === null || stack.typingTarget === id
  const onScreen = props.onScreen !== false
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
    rowConfirmed,
    readRowConfirmed,
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
  //
  // Only the typing target's: the box is rendered for one part of a stack, and
  // the compose redirect, the paste listener and the suspended tab stop follow
  // it.
  const composeSurfaceLive = composeBarEnabled && isOwner && isTypingTarget
  const live = useTerminalLiveSettings(
    liveSettingsFor(composeSurfaceLive, onScreen),
  )

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
  //
  // So does a part off screen whose row the server has not named this page's
  // driver for yet: it renders at the pty's own grid and sends nothing until
  // the part on screen has claimed the row.
  const faithfulWatcher = !isOwner || (!onScreen && !rowConfirmed)
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
    // Composition on the sign-in gate's page is not this pane's: the app stays
    // mounted under it, and these listeners see the gate's events first.
    const start = outsideGate(() => {
      composingRef.current = true
    })
    const end = outsideGate(() => {
      composingRef.current = false
      setCompositionEnded((n) => n + 1)
    })
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
    topInputGates,
  } = terminalInputLayout({
    isOwner,
    composeMode,
    keysApply,
    accessoryBarVisible,
    composeBarEnabled,
  })

  const everReady = useLatched(hasOutput)
  const onScreenChangeRef = useRef<((onScreen: boolean) => void) | null>(null)
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
    readRowConfirmed,
    onScreenChangeRef,
  })

  // Coming on screen and going off it, after mount: the socket opened naming
  // the visibility it had then (`ptySocket.ts`), so only a change is news.
  const onScreenSeenRef = useRef(onScreen)
  useEffect(() => {
    if (onScreenSeenRef.current === onScreen) return
    onScreenSeenRef.current = onScreen
    onScreenChangeRef.current?.(onScreen)
  }, [onScreen])

  // The pane's one automatic typing focus: once per attach, and never before the
  // ownership verdict and the replay are in, or while an IME composition runs.
  useEffect(() => {
    const decision = nextTypingFocus({
      // Only the typing target's part moves focus by itself, so a second part
      // finishing its replay later never pulls focus off the part chosen.
      allowed:
        isTypingTarget &&
        typingFocusAllowed({
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
    isTypingTarget,
    composeBarEnabled,
    handshakeSeen,
    replayApplied,
    compositionEnded,
    kind,
    id,
    composeInputRef,
    termRef,
  ])
  // The compose bar renders outside the terminal container, so the container's
  // capture listener cannot see a paste that lands in the box.
  //
  // The box mounts in the stack's input area one commit after the area exists,
  // so the area is a dependency too.
  const inputArea = stack?.inputArea ?? null
  const coverArea = stack?.coverArea ?? null
  useEffect(() => {
    if (!composeSurfaceLive) return
    const el = composeInputRef.current
    if (el === null) return
    const handler = (e: ClipboardEvent) => upload.onClipboardPaste(e)
    el.addEventListener("paste", handler)
    return () => el.removeEventListener("paste", handler)
    // `onClipboardPaste` reads refs and the live bootstrap at call time, so
    // re-registering on every identity change would buy nothing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeSurfaceLive, inputArea])

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
    composeSurface: topComposeSurface,
  } = topInputGates
  useEffect(() => {
    if (!isOwner) return
    return registerPaneInputGroup(id, {
      surfaceSwitch: topSurfaceSwitch,
      keysToggle: topKeysToggle,
      composeSurface: topComposeSurface,
    })
  }, [id, isOwner, topSurfaceSwitch, topKeysToggle, topComposeSurface])

  // Report the slot tab stopping cleanly with the whole agent leaving `active`:
  // the user stays on the agent and the tab rests on its idle screen. A badly
  // ended run is not reported, since its card is the diagnosis surface.
  const sessionStatus = session?.status
  const agentSessionId = props.kind === "agent" ? props.sessionId : null
  useEffect(() => {
    if (
      agentSessionId !== null &&
      slotTabEndedInView(isSessionSlotTab, everReady, sessionStatus, lastRunFailed)
    ) {
      settleSlotTabExit(agentSessionId, id)
    }
  }, [agentSessionId, id, isSessionSlotTab, everReady, sessionStatus, lastRunFailed])

  // There is one periodic client frame and one timer behind it (`lib/heartbeat.ts`);
  // gaining ownership retimes that timer rather than adding a second sender.

  // A failed socket may mean the pane is about to leave rather than that the
  // connection is at fault: an agent tab's provider exiting hands its slot to a
  // sibling, and a terminal's shell exiting or its being closed elsewhere takes
  // the terminal and this pane with it. Either way the spine's update follows
  // the socket's failure by a moment, so the box waits for it.
  const handoffGrace = useHandoffGrace(connectionLost)
  const cover = attachCover({
    socket: connectionLost ? "failed" : reconnecting ? "connecting" : "open",
    replayApplied,
    everReady,
    offline,
    waitExpired: replayWaitExpired,
    isOwner,
    firstAttach: appliedEpoch === null,
    handoffGrace,
  })

  // Publish whether the cover speaks for the whole pane, under this pane's pty
  // id, for the chrome outside it. The phone's header comes back on this while
  // theater stays on, since the pill it would otherwise offer is withheld here.
  // A layout effect, so a mounted pane's true verdict is in the registry before
  // the first paint and the shell never paints one frame of the wrong chrome.
  // The window while the lazy chunk resolves remains, and costs nothing: no
  // pane is painted then either.
  const coverOwnsPane = coverOwnsThePane(cover)
  useLayoutEffect(() => registerPaneCover(id, coverOwnsPane), [id, coverOwnsPane])

  // Where a picked macro goes, published under this pane's pty id for the Macros
  // controls outside it. Only the owner gets a way to write: under the take-over
  // card the control is disabled, and nothing else may type for it either. While
  // the bar renders, a macro is spliced into the compose DRAFT at the caret
  // instead of being written to the PTY. A layout effect, like the cover above,
  // so the control is never painted enabled over a card for a frame.
  const takeoverCard = cover.kind === "card"
  const composeSinkLive = composeBarEnabled && isOwner
  useLayoutEffect(
    () =>
      registerPaneMacroTarget(id, {
        takeoverCard,
        send: isOwner ? (bytes) => ptyRef.current?.sendInput(bytes) ?? false : null,
        compose: composeSinkLive
          ? {
              insert: input.insertComposeText,
              target: () => composeInputRef.current,
            }
          : null,
        // Resolved at call time, so a compose bar coming or going needs no
        // republish.
        typingSurface: () =>
          live.current.composeActive && composeInputRef.current
            ? composeInputRef.current
            : (termRef.current?.textarea ?? null),
      }),
    // `insertComposeText` reads only refs and the stable setter, so listing it
    // would republish on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [id, takeoverCard, isOwner, composeSinkLive],
  )

  const surface = (
    <TerminalPaneSurface
      partId={id}
      cardInStack={coverArea !== null}
      onScreen={onScreen}
      onPress={() => stack?.chooseTypingTarget(id)}
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
    />
  )

  // The typing target's rows go in the stack's one input area, for the input
  // owner alone; every other part publishes nothing there.
  const rows =
    isTypingTarget && isOwner ? (
      <TerminalInputRows
        accessoryBarShown={accessoryBarShown}
        composeBarShown={composeBarShownHere}
        input={input}
        composeInputRef={composeInputRef}
        kind={kind}
      />
    ) : null
  const area = stack?.inputArea ?? null
  const stackCard =
    coverArea !== null && isTypingTarget && cover.kind === "card"
      ? createPortal(
          <TerminalCover
            cover={cover}
            kind={kind}
            providerName={providerName}
            ptyRef={ptyRef}
            takeoverIntent={takeoverIntent}
            takeoverLabel={takeoverLabel}
            ownerPresent={ownerPresent}
            takeOver={takeOver}
          />,
          coverArea,
        )
      : null
  return (
    <>
      {surface}
      {stackCard}
      {rows === null ? null : area === null ? (stack === null ? rows : null) : createPortal(rows, area)}
    </>
  )
}

