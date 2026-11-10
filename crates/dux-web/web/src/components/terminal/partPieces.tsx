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
import { AGENT_PLACEHOLDER, ComposeBar, TERMINAL_PLACEHOLDER } from "@/components/ComposeBar"
import {
  composeBarMode,
  composeBarShown,
  inputMenuSurfaceSwitchOffered,
  terminalKeysApply,
} from "@/lib/composebar"
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
import { ESC, TAB } from "@/lib/termkeys"
import { mobileAccessoryBarVisible, useDux } from "@/lib/store"
import type { DuxState } from "@/lib/store"
import type { PtySocket } from "@/lib/ptySocket"
import { matchOwner, ownerProjectId, ownerSessionId } from "@/lib/terminalOwner"
import { spineListsTerminal, terminalsForOwner } from "@/lib/terminals"
import { DEFAULT_ATTENTION_GRACE_SECONDS } from "@/lib/viewedPing"
import { type AttachCover } from "@/lib/attachCover"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { GlyphSpinner } from "@/components/GlyphSpinner"
import { type TerminalLiveSettings } from "@/components/terminal/liveValues"
import { plainBounce } from "@/components/terminal/plainBounce"
import { HANDOFF_GRACE_MS } from "@/components/terminal/constants"
import { type InputSurface } from "@/components/terminal/inputSurface"
import { type UploadPipeline } from "@/components/terminal/uploadPipeline"
import type { TakeoverIntent } from "@/components/terminal/channels"
import { sessionLabel } from "@/lib/agentWorkspace"
import type { TerminalPartProps } from "@/components/TerminalStack"

type TerminalTargetIds = {
  sessionId: string | null
  projectId: string | null
}

function terminalTargetIds(props: TerminalPartProps): TerminalTargetIds {
  return {
    sessionId:
      props.kind === "agent" ? props.sessionId : ownerSessionId(props.owner),
    projectId:
      props.kind === "terminal" ? ownerProjectId(props.owner) : null,
  }
}

function terminalTargetRecords(
  props: TerminalPartProps,
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
  props: TerminalPartProps,
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
  props: TerminalPartProps,
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
  kind: TerminalPartProps["kind"],
  records: TerminalTargetRecords,
): string | undefined {
  if (kind === "agent") {
    return records.focusedTab?.provider ?? records.session?.provider
  }
  return records.session?.provider
}

function terminalSpineInputOwner(
  kind: TerminalPartProps["kind"],
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
  props: TerminalPartProps,
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

export function terminalInputLayout(input: TerminalInputLayoutInputs) {
  // Two rows, two switches, four legal states: neither is the other's on-switch,
  // and keys with no box is a finger on a virtual Ctrl beside a real keyboard.
  const accessoryBarShown =
    input.isOwner && input.keysApply && input.accessoryBarVisible
  const composeBarShown = input.isOwner && input.composeBarEnabled
  // What the top menu's INPUT group carries for this pane. Always on offer to
  // the owner whatever is up under the terminal, because the way back from direct
  // typing and from hidden keys must not depend on a row that is itself gone.
  const topInputGates = {
    surfaceSwitch: input.isOwner && inputMenuSurfaceSwitchOffered(input.composeMode),
    keysToggle: input.isOwner && input.keysApply,
    composeSurface: composeBarShown,
  }
  return {
    accessoryBarShown,
    composeBarShown,
    topInputGates,
  }
}

// Whether a just-failed socket is still inside the window in which the spine
// removes its pane (a promotion, a closed terminal). Armed during render, so the
// box never paints for one frame on its way to being held, and released by the
// clock or by the socket coming back.
export function useHandoffGrace(failing: boolean): boolean {
  const [holding, setHolding] = useState(false)
  const [wasFailing, setWasFailing] = useState(failing)
  if (failing !== wasFailing) {
    setWasFailing(failing)
    setHolding(failing)
  }
  useEffect(() => {
    if (!holding) return
    const timer = setTimeout(() => setHolding(false), HANDOFF_GRACE_MS)
    return () => clearTimeout(timer)
  }, [holding])
  return holding
}

// True from the first render `value` is true on, for the rest of the mount.
export function useLatched(value: boolean): boolean {
  const [latched, setLatched] = useState(false)
  if (value && !latched) setLatched(true)
  return latched
}

function terminalLiveSettings(
  preferences: TerminalPreferences,
  target: ReturnType<typeof terminalTargetView>,
  terminalListed: { now: boolean; ever: boolean },
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
    terminalListed,
    viewerOverflow,
    composeActive: composeBarEnabled,
  }
}

export function useTerminalPaneSetup(props: TerminalPartProps) {
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
  const listedNow =
    props.kind === "terminal" && spineListsTerminal(spine?.terminals, props.id)
  const terminalListed = { now: listedNow, ever: useLatched(listedNow) }
  // Built here and published in the component body: whether the message box is
  // the typing surface depends on ownership, which resolves after this hook.
  const liveSettingsFor = (composeActive: boolean, onScreen: boolean) => ({
    ...terminalLiveSettings(
      preferences,
      target,
      terminalListed,
      viewerOverflow,
      composeActive,
    ),
    onScreen,
  })

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
  /// Off screen, the part keeps a real box the size of the stack's and is
  /// hidden, never shrunk.
  onScreen: boolean
  /// A press anywhere on the part makes it the typing target.
  onPress: () => void
  kind: TerminalPartProps["kind"]
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
}

export function TerminalPaneSurface({
  onScreen,
  onPress,
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
}: TerminalPaneSurfaceProps) {
  // The pane's box is not itself the clip: the terminal, its covers and the
  // drop overlay are clipped by an inner layer, while the stack's overlay
  // layer sits over the stack's box beside it. (A dormant tab has no pane, and
  // its shell paints the overlay in the column instead.)
  return (
    <div
      className={
        onScreen
          ? "group relative min-h-0 w-full flex-1 bg-background"
          : "group invisible absolute inset-0 bg-background"
      }
      onPointerDownCapture={onPress}
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
      <div className="absolute inset-0 overflow-hidden">
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
      </div>
    </div>
  )
}

function FileDropOverlay({ kind }: { kind: TerminalPartProps["kind"] }) {
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
  kind: TerminalPartProps["kind"]
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
  kind: TerminalPartProps["kind"],
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
  kind: TerminalPartProps["kind"]
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

type TerminalInputRowsProps = {
  accessoryBarShown: boolean
  composeBarShown: boolean
  input: InputSurface
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  kind: TerminalPartProps["kind"]
}

export function TerminalInputRows({
  accessoryBarShown,
  composeBarShown,
  input,
  composeInputRef,
  kind,
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
        />
      ) : null}
    </>
  )
}
