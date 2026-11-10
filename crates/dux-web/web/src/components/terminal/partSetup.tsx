import { useEffect, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"
import type { FitAddon } from "@xterm/addon-fit"
import {
  composeBarMode,
  composeBarShown,
  inputMenuSurfaceSwitchOffered,
  terminalKeysApply,
} from "@/lib/composebar"
import { isFirstTab } from "@/lib/agentTabs"
import { useIsMobile } from "@/hooks/use-mobile"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useTypingSurface } from "@/hooks/use-typing-surface"
import { useFilePicker } from "@/hooks/use-file-picker"
import { mobileAccessoryBarVisible, useDux } from "@/lib/store"
import type { DuxState } from "@/lib/store"
import type { PtySocket } from "@/lib/ptySocket"
import { matchOwner, ownerProjectId, ownerSessionId } from "@/lib/terminalOwner"
import { spineListsTerminal, terminalsForOwner } from "@/lib/terminals"
import { DEFAULT_ATTENTION_GRACE_SECONDS } from "@/lib/viewedPing"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { type TerminalLiveSettings } from "@/components/terminal/liveValues"
import { HANDOFF_GRACE_MS } from "@/components/terminal/constants"
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

