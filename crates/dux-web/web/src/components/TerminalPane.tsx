import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"
import { MonitorSmartphone } from "lucide-react"
import { toast } from "sonner"
import { AccessoryBar } from "@/components/AccessoryBar"
import type { ScrollDir } from "@/components/AccessoryBar"
import { ComposeBar, RestoreBarsButton } from "@/components/ComposeBar"
import {
  COMPOSE_SUBMIT_DELAY_MS,
  composeSendTooLarge,
  composeSendWrites,
  insertIntoComposeDraft,
} from "@/lib/composebar"
import {
  getComposeInsertSink,
  setComposeInsertSink,
} from "@/lib/composeInsert"
import {
  type ConfiguredDropPaste,
  type DropContext,
  type DropOutcome,
  dropToastFor,
  dragDropPasteFor,
  dropRefusalReason,
  pasteExceedsAttachmentLimit,
  pastePayload,
  tooLongToAttachReason,
} from "@/lib/fileDrop"
import { FileDropApiError, uploadDroppedFile } from "@/lib/fileDropApi"
import { showBusyToast } from "@/lib/busyToast"
import { showFinalToast } from "@/lib/finalToast"
import { MacroPopover } from "@/components/MacroPopover"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { useIsMobile } from "@/hooks/use-mobile"
import { dragScrollLines, dragWheelReport } from "@/lib/viewport"
import { firstFrameResizePlan } from "@/lib/firstFrameResize"
import { copyToClipboard } from "@/lib/clipboard"
import { isApplePlatform } from "@/lib/platform"
import {
  applyModifiers,
  arrowSeq,
  classifyClipboardKey,
  copyOnSelectAction,
  ESC,
  LF,
  linkActivateAction,
  pageKeySeq,
  sgrClickSeq,
  sgrWheelSeq,
  softNewlineAction,
  TAB,
} from "@/lib/termkeys"
import {
  ejectSelectionForReconnect,
  handleTabGone,
  mobileAccessoryBarVisible,
  mobileTopBarVisible,
  noteAgentPtyOwnership,
  noteOwnPtyConnection,
  restoreMobileBars,
  useDux,
} from "@/lib/store"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import { isTabGone } from "@/lib/agentTabs"
import {
  nextAppliedGeneration,
  shouldApplyReplay,
} from "@/lib/replayGeneration"
import {
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  tabPtyUrl,
  terminalSocketUrl,
} from "@/lib/ptySocket"
import { matchOwner, ownerProjectId, ownerSessionId } from "@/lib/terminalOwner"
import { terminalsForOwner } from "@/lib/terminals"
import {
  isForeground,
  isOwnerAfterHandover,
  onPtyOwner,
} from "@/lib/ptyOwnership"
import { deviceLabel } from "@/lib/deviceLabel"
import {
  DEFAULT_ATTENTION_GRACE_SECONDS,
  VIEWED_PING_INTERVAL_MS,
  shouldSendViewed,
  visibleSinceAfterTransition,
} from "@/lib/viewedPing"
import { DEFAULT_SCROLLBACK_LINES } from "@/lib/types"
import { suppressViewerReports } from "@/lib/suppressViewerReports"
import { registerAgentNotifications } from "@/lib/agentNotifications"
import { BrailleSpinner } from "@/components/BrailleSpinner"

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

// Bytes written straight to the PTY (bypassing xterm's data pipeline), plus the
// view side effects a typed key would get through that pipeline: snap to the
// live edge and drop any stale selection so the user sees where the input
// landed. Module-scoped and shared so every entry point that writes input
// directly (the physical Shift-Enter handler, the accessory bar's ⇧↵ key, and
// the mobile compose bar's Send) lands identically and can't drift apart.
// Latch handling is left to each caller (they decide it from different rules).
function writeInputWithLandingEffects(
  term: Terminal | null,
  pty: PtySocket | null,
  bytes: Uint8Array,
): void {
  term?.scrollToBottom()
  term?.clearSelection()
  pty?.sendInput(bytes)
}

// A soft newline (LF / Ctrl-j): the shared landing-effects write with the one
// fixed LF byte, kept as its own named helper so the two soft-newline entry
// points (physical Shift-Enter and the accessory bar's ⇧↵ key) stay in step.
const LF_BYTES = new TextEncoder().encode(LF)
function writeSoftNewline(term: Terminal | null, pty: PtySocket | null): void {
  writeInputWithLandingEffects(term, pty, LF_BYTES)
}

// Whether the "app captured the mouse, hold the modifier to select" hint has been
// shown. Module level so it fires at most once per page session, surviving the pane
// remounts that happen on every agent/tab switch (a per-component ref would reset).
let mouseCaptureHintShown = false

// The pointer must move at least this many CSS px between mousedown and mouseup to
// count as a drag (a selection attempt) rather than a click. Guards the mouse-capture
// hint from firing on a plain click into a mouse-reporting app.
const DRAG_THRESHOLD_PX = 4

// Desktop wheel speed for LOCAL scrollback scrolling: xterm's scrollSensitivity
// multiplier, set to 3 so one wheel notch moves three lines' worth instead of
// one (matching the TUI's MOUSE_WHEEL_LINES). Verified against the installed
// xterm 6 source: the Viewport passes this option to its scrollable element as
// `mouseWheelScrollSensitivity` (local viewport scrolling only), and that local
// wheel handling is DISABLED entirely while an app in the PTY captures the
// wheel; the wheel-REPORT path to a mouse-tracking app sends one report per
// wheel event regardless of this value, so app forwarding stays 1:1 per tick.
// The touch drag path (dragScrollLines) is finger-proportional and unaffected.
const WHEEL_SCROLL_SENSITIVITY = 3

// How long the container must hold still before its new size goes to the PTY.
// A PTY resize is a SIGWINCH — a full child repaint — so it is debounced to one
// send with the final dimensions (the ResizeObserver below), and the SAME delay
// is reused when a touch-scroll gesture ends with a held resize to flush (see
// resizeHeldByGesture): the flush is a settle window like any other, giving the
// keyboard/URL-bar animation that held it time to finish collapsing.
const RESIZE_SEND_DEBOUNCE_MS = 200

// The sonner id ONE file drop lives on: its per-file spinners and its single
// report at the end. One id per drop is what makes the final REPLACE that
// drop's spinner in place instead of stacking a second toast beneath it, and it
// is what lets the final retire that spinner's leak guard without either side
// knowing about the other.
//
// It is minted per drop rather than being a module constant. Uploads within one
// drop are sequential, but a drop is not atomic, so two quick drops overlap:
// with a shared id, drop A's final landed on the id and was then painted over by
// drop B's next per-file spinner, and the user lost A's report entirely. That
// report is often the error naming which files were refused, which is the one
// the user most needs. A counter rather than a random id, so the ordering stays
// legible.
let fileDropSeq = 0
function nextFileDropToastId(): string {
  fileDropSeq += 1
  return `file-drop-${fileDropSeq}`
}

// Copy the terminal's current selection to the clipboard and toast the result.
// `copyToClipboard` writes via the async Clipboard API in a secure context and
// falls back SYNCHRONOUSLY to an execCommand hidden-textarea over plain-HTTP, so
// calling this from inside a user gesture (mouseup, keydown, menu click) keeps
// the write permitted even over a Tailscale plain-HTTP origin. The deduped toast
// id makes rapid copy-on-select replace the toast rather than stack it. Module
// level so it's a stable reference shared by the mount effect and the menu.
// `refocus` restores focus once the copy settles: the call sites pass the
// pane's `focusTypingSurface` so focus lands on the ACTIVE typing surface (the
// compose textarea when the mobile compose bar is up, xterm otherwise) rather
// than being hardwired to `term.focus()`.
function copyTermSelection(term: Terminal, refocus: () => void): void {
  const sel = term.getSelection()
  if (!sel) return
  void copyToClipboard(sel)
    .then((ok) =>
      ok
        ? toast.success("Copied to clipboard", { id: "term-copy" })
        : toast.error("Couldn't copy to clipboard", { id: "term-copy" }),
    )
    .finally(refocus)
}

// Paste the BROWSER clipboard into the terminal via the async Clipboard API.
// `readText` needs a secure context (HTTPS/localhost) and THROWS synchronously
// when `navigator.clipboard` is undefined (plain-HTTP) or `readText` is missing
// (Firefox web content), so we must guard the call — a bare `.catch` cannot
// catch a synchronous throw. The plain-HTTP/Ctrl-v path (handled by xterm's
// native paste event) stays the secure-context-free fallback. `term.paste`
// applies bracketed-paste (DECSET 2004) and newline normalization.
// `refocus` mirrors `copyTermSelection`'s: the call site passes
// `focusTypingSurface` so focus returns to the active typing surface.
function pasteIntoTerm(term: Terminal, refocus: () => void): void {
  const read = navigator.clipboard?.readText?.()
  if (!read) {
    toast.error("Couldn't read clipboard — use Ctrl+v to paste", { id: "term-paste" })
    refocus()
    return
  }
  void read
    .then((text) => term.paste(text))
    .catch(() =>
      toast.error("Couldn't read clipboard — use Ctrl+v to paste", { id: "term-paste" }),
    )
    .finally(refocus)
}

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
  // The PTY socket URL for THIS target. For an agent, the session-slot tab
  // (`id === sessionId`) uses the session PTY route and an extra tab its own
  // nested route; a terminal uses its owner's nested route (session- or
  // project-scoped). Computed at render (it derives purely from the props) and
  // consumed by the mount effect below.
  const ptyUrl =
    props.kind === "agent"
      ? props.id === props.sessionId
        ? agentPtyUrl(props.sessionId)
        : tabPtyUrl(props.sessionId, props.id)
      : terminalSocketUrl(props.owner, props.id)
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
  // The dedicated PTY socket for the focused target. Created in the wiring effect
  // and read by the accessory-bar key handlers (defined at component scope) so
  // they send stdin to the same socket xterm's `onData` does.
  const ptyRef = useRef<PtySocket | null>(null)
  const isMobile = useIsMobile()

  // Drag-and-drop of a file onto the pane. `dragDepth` counts enter/leave pairs
  // because dragging across a child element fires a `dragleave` for the parent;
  // a plain boolean would flicker the overlay off over every internal boundary.
  const [dragActive, setDragActive] = useState(false)
  const dragDepthRef = useRef(0)

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

  const duxState = useDux()
  const { spine, bootstrap, offline, conn } = duxState
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
  // Whether selecting text auto-copies it (the `ui.copy_on_select` preference,
  // default on). Read via a ref inside the stable mount-effect `mouseup` handler
  // so toggling it never recreates the terminal; the fallback (true) applies only
  // before the first bootstrap fetch lands.
  const copyOnSelectRef = useRef(bootstrap?.copy_on_select ?? true)
  useEffect(() => {
    copyOnSelectRef.current = bootstrap?.copy_on_select ?? true
  }, [bootstrap?.copy_on_select])
  // The mobile compose bar (the `ui.compose_bar` preference, default on): the
  // phone's typing surface, a buffered textarea below the accessory bar whose
  // Send delivers the message in one write. Rendering reads the reactive value;
  // the ref mirrors "the compose bar is up" (mobile AND preference on) for the
  // stable mount-effect closures (the tap-to-focus redirect below), which would
  // otherwise capture a stale value. When the preference is off, nothing
  // renders and no focus behavior changes, exactly today's tap-focuses-xterm.
  const composeBarEnabled = isMobile && (bootstrap?.compose_bar ?? true)
  // The two hideable-bar preferences (`ui.mobile_top_bar`,
  // `ui.mobile_accessory_bar`), resolved through their optimistic overrides.
  // The accessory value gates the AccessoryBar render below (beside the
  // existing owner gate, for EVERY terminal kind, since all of them share
  // this pane); the pair decides whether the compose bar shows its restore
  // button, the escape hatch that brings BOTH bars back in one tap.
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  const anyMobileBarHidden =
    !mobileTopBarVisible(duxState) || !accessoryBarVisible
  // Whether dropping a file onto this pane does anything at all. `[server]
  // file_drop_max_bytes = 0` switches the feature off, so the whole drag
  // surface goes with it (see `dragCarriesFiles`). Read reactively rather than
  // through a ref: the drag handlers are rendered props, not mount-effect
  // closures, so they see the current value.
  //
  // NOT YET KNOWN is NOT ENABLED. Bootstrap and the workspace load in parallel,
  // so the pane renders before the bootstrap document arrives, and an older
  // server never sends the field at all. Defaulting that window to ON matched
  // the config default but offered a feature dux could not yet say it had: with
  // the setting switched off, a drag landing in that window still showed the
  // overlay and still uploaded. There is nothing to lose by waiting, because
  // the window closes in one fetch and the drag surface simply appears then.
  const fileDropEnabled = (bootstrap?.file_drop_max_bytes ?? 0) > 0
  // The per-provider drop-paste config, mirrored into a ref so the drop loop can
  // read the CURRENT map rather than the one that happened to be in the closure
  // when the drag landed. A drop's uploads run one at a time and a multi-file
  // drop is not quick, so a `config.changed` refetch really can land in the
  // middle of one; the plan is resolved immediately before each paste, so a
  // reload takes effect from the next file onward instead of being ignored for
  // the rest of the drop.
  const configuredDropPasteRef = useRef<ConfiguredDropPaste>(
    bootstrap?.provider_drop_paste,
  )
  useEffect(() => {
    configuredDropPasteRef.current = bootstrap?.provider_drop_paste
  }, [bootstrap?.provider_drop_paste])
  // Retire any in-flight drag the moment the feature stops being available.
  // The gate refuses events for a disabled feature, so once it closes there is
  // no matching `dragleave` or `drop` left to clear the overlay, and it would
  // sit on screen until the pane unmounted. This is reachable in the ordinary
  // case: the bootstrap document can land, saying the feature is off, while a
  // drag is already over the pane.
  useEffect(() => {
    if (fileDropEnabled) return
    dragDepthRef.current = 0
    setDragActive(false)
  }, [fileDropEnabled])
  // The ref lags the rendered value by one commit (it is synced in an effect),
  // so an event firing inside that window can see the previous state. Both
  // mismatch directions degrade gracefully: a stale `false` falls through to
  // `term.focus()` (today's behavior), a stale `true` at worst redirects one
  // tap into a bar that just unmounted (the focus call no-ops on a null ref).
  const composeActiveRef = useRef(composeBarEnabled)
  useEffect(() => {
    composeActiveRef.current = composeBarEnabled
  }, [composeBarEnabled])
  // The compose textarea, owned by ComposeBar but targeted from here: the
  // tap-to-focus redirect and the scroll-gesture keyboard dismissal both need
  // to focus/blur it from outside the component.
  const composeInputRef = useRef<HTMLTextAreaElement | null>(null)
  // The compose BUFFER lives here, not in ComposeBar, precisely so the bar can
  // unmount (a preference flip, a rotation past the mobile breakpoint) without
  // destroying in-progress text; the bar is a controlled input over this state.
  const [composeText, setComposeText] = useState("")
  // Where the caret should land after a programmatic draft splice (a picked
  // macro inserting into the draft). A controlled textarea re-renders on the
  // value change and the browser parks the caret at the end of the new value,
  // so the splice records its intended caret here and this effect applies it
  // in the same commit the new draft text reaches the DOM. Null means "no
  // pending placement" — ordinary typing never goes through this.
  const pendingComposeCaretRef = useRef<number | null>(null)
  useEffect(() => {
    const caret = pendingComposeCaretRef.current
    if (caret === null) return
    pendingComposeCaretRef.current = null
    composeInputRef.current?.setSelectionRange(caret, caret)
  }, [composeText])
  // The `ui.attention_grace_seconds` preference (default 3s), converted to ms.
  // Read via a ref inside the stable mount-effect visibility handlers so
  // changing it never recreates the terminal; the fallback (3s) applies only
  // before the first bootstrap fetch lands.
  const attentionGraceMsRef = useRef(
    (bootstrap?.attention_grace_seconds ?? DEFAULT_ATTENTION_GRACE_SECONDS) *
      1000
  )
  useEffect(() => {
    attentionGraceMsRef.current =
      (bootstrap?.attention_grace_seconds ?? DEFAULT_ATTENTION_GRACE_SECONDS) *
      1000
  }, [bootstrap?.attention_grace_seconds])
  // Tracks the attention-grace hidden -> visible transition (see
  // `visibleSinceAfterTransition` in viewedPing.ts). Refs, not state, so both
  // the mount effect's visibility listeners and the ownership-gain effect
  // below read/update the same value without re-running the mount effect.
  // Per-component (not module-level): each mounted pane listens to its own
  // visibilitychange/focus events, so tracking per-component is correct.
  // `undefined` means "no transition observed" (covers initial load).
  const visibleSinceRef = useRef<number | undefined>(undefined)
  const prevVisibleRef = useRef<boolean | undefined>(undefined)
  // Whether agent notification sequences bridge to a browser Notification (the
  // `capabilities.web_notifications` bit, default on). Read lazily in the OSC
  // handlers so toggling it never recreates the terminal.
  const webNotificationsRef = useRef(bootstrap?.web_notifications ?? true)
  useEffect(() => {
    webNotificationsRef.current = bootstrap?.web_notifications ?? true
  }, [bootstrap?.web_notifications])
  // Whether OSC 8 hyperlinks are clickable (the `capabilities.hyperlinks` bit,
  // default on). Read lazily by the xterm linkHandler so toggling it never
  // recreates the terminal.
  const hyperlinksRef = useRef(bootstrap?.hyperlinks ?? true)
  useEffect(() => {
    hyperlinksRef.current = bootstrap?.hyperlinks ?? true
  }, [bootstrap?.hyperlinks])
  // The clipboard passthrough mode (the `capabilities.clipboard_passthrough` bit,
  // default "focused"). Read lazily by the OSC 52 handler so toggling it never
  // recreates the terminal; "off" makes the handler consume without writing. The
  // server has already resolved the `capabilities.passthrough` master switch into
  // this value, so it is the one thing to read for the clipboard.
  const clipboardPassthroughRef = useRef(bootstrap?.clipboard_passthrough ?? "focused")
  useEffect(() => {
    clipboardPassthroughRef.current = bootstrap?.clipboard_passthrough ?? "focused"
  }, [bootstrap?.clipboard_passthrough])
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
  // or the owning project's name for a project terminal, read lazily in the OSC
  // handler so a rename never recreates the terminal.
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
          session: () => session?.title || session?.branch_name || "Agent",
          project: () => project?.name || "Terminal",
          // No owner to be named after, so the generic noun is the whole name.
          standalone: () => "Terminal",
        })
      : session?.title || session?.branch_name || "Agent"
  const notifyTitleRef = useRef(notifyTitle)
  useEffect(() => {
    notifyTitleRef.current = notifyTitle
  }, [notifyTitle])
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
  // Mirrored for the same reason as the configured map: the drop loop is async
  // and its closure would otherwise pin whichever provider was running when the
  // drag landed, so a retarget or a relaunch mid-drop would be missed.
  const providerNameRef = useRef(providerName)
  useEffect(() => {
    providerNameRef.current = providerName
  }, [providerName])
  // What the focused tab's LIVE process launched with, off the SPINE, so it
  // tracks launches and terminations rather than going stale until the next
  // config refetch. `undefined` for a dormant tab and for a terminal, both of
  // which fall back to the configured map above.
  const launchedDropPasteRef = useRef(focusedTab?.drop_paste)
  useEffect(() => {
    launchedDropPasteRef.current = focusedTab?.drop_paste
  }, [focusedTab?.drop_paste])
  // Kept current for the mount effect's PTY-gone check (an extra tab's socket
  // must stop reconnecting once its tab is no longer in the spine — see
  // `isTabGone`) WITHOUT being a dependency of that effect, which would tear
  // down and recreate the socket on every spine refresh.
  const sessionTabsRef = useRef(session?.tabs)
  useEffect(() => {
    sessionTabsRef.current = session?.tabs
  }, [session?.tabs])
  // The macro popover's target. For an agent the streamed id is the FOCUSED TAB
  // id; for a terminal it is the terminal id. Mirrors the store's
  // `SelectedTarget` shape so the popover filters macros by the focused surface
  // and runs against the right PTY.
  const macroTarget: SelectedTarget =
    props.kind === "agent"
      ? { kind: "agent", sessionId: props.sessionId, tabId: id }
      : { kind: "terminal", terminalId: id, owner: props.owner }
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
  // True once this PTY socket has EXHAUSTED its reconnect budget and emitted
  // `failed` (the shared cap now applies to PTY sockets too, so a dead server no
  // longer silently reattaches behind a stuck overlay). Distinct from
  // `reconnecting` (still retrying): this is a hard stop that surfaces an explicit
  // Reconnect affordance. Cleared on the next connecting/open. Only meaningful
  // when the app is NOT globally offline — the app-wide OfflineOverlay owns that
  // case and its Retry already remounts this pane.
  const [connectionLost, setConnectionLost] = useState(false)

  // The pointer type of the most recent press on the host. Android Chrome fires
  // `contextmenu` on a touch LONG-PRESS, which would hijack the terminal's native
  // long-press-to-select; right-click paste only fires for a mouse/pen press, so
  // a touch long-press still hands off to native selection. This per-interaction
  // signal is exact where an `isMobile` width check is not (a touchscreen laptop
  // with a mouse must still get right-click paste).
  const pointerTypeRef = useRef("")
  // The left-button mousedown position, so `onMouseUp` can tell a drag (a selection
  // attempt) from a plain click and only hint about mouse-capture on the former.
  const mouseDownPosRef = useRef<{ x: number; y: number } | null>(null)

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
  // The other device's raw `User-Agent`, captured from the `pty.owner` handover that
  // demoted us to the read-only placeholder. Parsed into a human label ("Chrome on
  // macOS") for the take-over modal, and cleared the moment we regain ownership.
  const [takeoverDevice, setTakeoverDevice] = useState<string | null>(null)
  // Mirror of `isOwner` for the stable mount-effect closures (onData, the resize
  // senders) to read synchronously. Kept in sync only at the mutation points
  // (a take-over and the handover handler), never written during render.
  const isOwnerRef = useRef(isOwner)
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
      insert: (text: string) => {
        // The textarea's selection is read up front, once: the functional
        // updater below may run more than once (StrictMode), and it must
        // splice the same way each time. A missing element or selection falls
        // back to appending (insertIntoComposeDraft treats null as "append").
        const el = composeInputRef.current
        const selectionStart = el === null ? null : el.selectionStart
        const selectionEnd = el === null ? null : el.selectionEnd
        setComposeText((prev) => {
          const { next, caret } = insertIntoComposeDraft(
            prev,
            selectionStart,
            selectionEnd,
            text,
          )
          // A ref write inside the updater is idempotent: the same inputs
          // yield the same caret on a re-run. The caret-placement effect above
          // applies it once the new draft value reaches the DOM.
          pendingComposeCaretRef.current = caret
          return next
        })
        // The draft the macro just joined is where editing continues; the
        // active typing surface here IS the compose textarea.
        focusTypingSurface()
      },
      target: () => composeInputRef.current,
    }
    setComposeInsertSink(sink)
    return () => {
      // Only retire our own registration: a successor pane may already have
      // replaced it (the same guard `setActivePtySocket` cleanup uses).
      if (getComposeInsertSink() === sink) setComposeInsertSink(null)
    }
  }, [composeBarEnabled, isOwner])
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

  // Drop the specific device name whenever the events socket is not open. A
  // `pty.owner` handover is only delivered live over `/ws/events`; there is NO
  // replay on reconnect, so if ownership changes while our events socket is down we
  // would otherwise keep naming a now-wrong device. The generic "Active on another
  // device" copy is never wrong, so we fall back to it across any outage; a real
  // handover after reconnect repopulates the name. Clearing on the render-phase
  // transition (React's "adjust state when input changes" pattern) rather than an
  // effect avoids the extra commit-then-clear render pass.
  const [prevConn, setPrevConn] = useState(conn)
  if (conn !== prevConn) {
    setPrevConn(conn)
    if (conn !== "open") setTakeoverDevice(null)
  }

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
      linkHandler: {
        activate: (event, uri) => {
          const action = linkActivateAction(
            { button: event.button, detail: event.detail },
            { hyperlinks: hyperlinksRef.current, uri },
          )
          if (action !== "open") return
          window.open(uri, "_blank", "noopener,noreferrer")
        },
      },
    })
    // This xterm is a VIEWER of a PTY that dux-core's alacritty_terminal already
    // drives and answers device/color queries for. Stop it from also answering
    // (and injecting duplicate replies back into the shared PTY via onData); see
    // suppressViewerReports. Install before open so it is armed before any byte.
    suppressViewerReports(term)
    // Bridge the agent's notification/clipboard OSC sequences to the browser,
    // mirroring the TUI host passthrough. Registered next to suppressViewerReports
    // so both viewer hooks are armed before the first byte.
    const disposeAgentNotifications = registerAgentNotifications(term, {
      enabled: () => webNotificationsRef.current,
      title: () => notifyTitleRef.current,
      clipboardMode: () => clipboardPassthroughRef.current,
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
      () => !hyperlinksRef.current,
    )
    const fit = new FitAddon()
    term.loadAddon(fit)
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
    fit.fit()
    termRef.current = term

    // The dedicated PTY socket for THIS target: the agent's main provider PTY, or
    // a companion terminal's PTY (nested under its owning session). Opening it IS
    // the subscription — connecting an agent socket launches/resumes the provider,
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
    // Record this socket's connection id (the socket's first `connected` frame, and
    // again on every reconnect since the server allocates a fresh id per open) so
    // the `pty.owner` handler can compare a handover's claimer id against ours.
    pty.onConnected = (connId) => {
      myConnIdRef.current = connId
      // Register the id as one of OURS in the store, so the server-published
      // `input_owner` spine field can be compared against this client's own
      // identity by surfaces outside this pane (see `sessionActiveElsewhere`).
      noteOwnPtyConnection(connId, true)
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

    // xterm allows only ONE custom key-event handler, so this single closure owns
    // both the soft-newline chord and the clipboard chords. They match disjoint
    // keys (bare Shift-Enter vs Ctrl-based clipboard chords), so soft-newline is
    // checked first and clipboard classification handles the rest.
    //
    // Shift-Enter inserts a "soft" newline (LF / Ctrl-j) instead of submitting.
    // xterm collapses both Enter and Shift-Enter to a carriage return before
    // `onData` can see them, so the two are indistinguishable at the data layer —
    // we must intercept at the key-event layer instead. `softNewlineAction` owns
    // the decision (chord match, IME guard, ownership gate, latch clear); this
    // closure is the thin applicator that turns that decision into DOM/PTY effects.
    //
    // Clipboard chords: xterm's defaults don't bridge the browser clipboard on
    // Linux/Windows — Ctrl+v emits \x16 to the REMOTE agent (pasting the server's
    // clipboard) and Ctrl+c / a selection never reach the system clipboard. We
    // intercept only the clipboard chords; everything else (Ctrl+c SIGINT, plain
    // typing, mac Control/Cmd) passes through to xterm unchanged. `isMac` is stable
    // for this mount.
    const isMac = isApplePlatform()
    term.attachCustomKeyEventHandler((e) => {
      const action = softNewlineAction(e, {
        isOwner: isOwnerRef.current,
        ctrlLatched: modsRef.current.ctrl,
        altLatched: modsRef.current.alt,
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
        // side effects our early return skipped — shared with the accessory bar's
        // ⇧↵ key so the two entry points can't drift.
        if (action.send !== null) {
          if (action.clearLatch) setMods({ ctrl: false, alt: false })
          writeSoftNewline(term, pty)
        }
        return false
      }
      // Clipboard chords (keydown only).
      if (e.type !== "keydown") return true
      const clip = classifyClipboardKey({
        ctrlKey: e.ctrlKey,
        shiftKey: e.shiftKey,
        altKey: e.altKey,
        metaKey: e.metaKey,
        code: e.code,
        keyCode: e.keyCode,
        isMac,
      })
      if (clip === "passthrough") return true
      if (clip === "copy") {
        // The chord is not a browser copy event, so we copy the selection
        // ourselves. preventDefault so the browser/devtools don't also act;
        // return false so xterm doesn't process the chord.
        copyTermSelection(term, focusTypingSurface)
        e.preventDefault()
        return false
      }
      // clip === "paste":
      if (!isOwnerRef.current) {
        // Read-only viewer: swallow at the source so no native paste event fires.
        e.preventDefault()
        return false
      }
      // Owner: return false WITHOUT preventDefault so xterm emits no \x16 and the
      // browser's default Ctrl+v fires a native `paste` event, which xterm's own
      // handler reads from clipboardData (secure-context-free) and forwards as
      // (bracketed) onData.
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
    if (isOwnerRef.current) focusTypingSurface()

    // Copy-on-select (highlight to copy), gated by the `copy_on_select`
    // preference. Runs in the `mouseup` user gesture so the clipboard write is
    // permitted even over plain-HTTP (copyToClipboard falls synchronously to its
    // execCommand path there). Record the left-button-down position so mouseup can
    // tell a drag from a click. `copyOnSelectAction` decides: copy a real local
    // selection; when the user dragged but the app captured the mouse (so xterm
    // forwarded the drag to the host and nothing was selected locally), surface a
    // one-time hint to hold the force-selection modifier; otherwise do nothing.
    const onMouseDown = (e: MouseEvent) => {
      if (e.button === 0) mouseDownPosRef.current = { x: e.clientX, y: e.clientY }
    }
    const onMouseUp = (e: MouseEvent) => {
      const down = mouseDownPosRef.current
      mouseDownPosRef.current = null
      const dragged =
        down !== null &&
        Math.hypot(e.clientX - down.x, e.clientY - down.y) >= DRAG_THRESHOLD_PX
      const action = copyOnSelectAction({
        copyOnSelect: copyOnSelectRef.current,
        selection: term.getSelection(),
        dragged,
        mouseTrackingMode: term.modes.mouseTrackingMode,
        hintShown: mouseCaptureHintShown,
      })
      if (action === "copy") {
        copyTermSelection(term, focusTypingSurface)
      } else if (action === "hint") {
        mouseCaptureHintShown = true
        toast(
          `This app is using the mouse. Hold ${
            isMac ? "⌥ Option" : "Shift"
          } and drag to select and copy to your device.`,
          { id: "term-mouse-capture-hint", duration: 8000 },
        )
      }
    }
    container.addEventListener("mousedown", onMouseDown)
    container.addEventListener("mouseup", onMouseUp)

    // Kill xterm's right-click paste. On a mouse right-click xterm's own handler
    // stuffs the current selection into its hidden input textarea (its
    // native-Copy preparation); left there it leaks back into the PTY as a paste.
    // We drive our own clipboard menu, so wipe the textarea on `contextmenu`
    // (which fires right after that handler, before any input event could send
    // it). It only touches xterm's hidden input — the selection MODEL that our
    // menu's Copy reads is untouched, so the highlight and Copy stay intact.
    // Skip touch (a long-press uses the native selection gesture, not xterm's
    // mouse right-click handler, so nothing was stuffed).
    const onContextMenuPasteGuard = () => {
      if (pointerTypeRef.current === "touch") return
      if (term.textarea) term.textarea.value = ""
    }
    container.addEventListener("contextmenu", onContextMenuPasteGuard)

    // Touch gestures over the terminal, mapped to the natural mobile model:
    //   - a one-finger DRAG scrolls the scrollback,
    //   - a stationary LONG-PRESS hands off to the browser's native text
    //     selection (and its handle-drag to extend it),
    //   - a quick TAP falls through to xterm so it focuses and the keyboard opens.
    // xterm's text layer sits over its scrollable viewport, so a finger drag on
    // the output never reaches the native scroll (only the slim scrollbar does);
    // we bridge that by translating a vertical drag into xterm's own
    // scrollLines() — the same scrollback the accessory-bar PgUp/PgDn keys move
    // through (they call scrollPages). Touch-only
    // listeners, so this also lights up a touchscreen laptop, not just the mobile
    // layout.
    //
    // The normal buffer has xterm scrollback, so a drag scrolls it locally. The
    // ALT-SCREEN (a full-screen TUI like Claude's renderer) has NO xterm
    // scrollback — the app keeps its own history that never reaches xterm. When
    // such an app has mouse tracking on and we own the PTY, we forward the drag
    // to it as SGR wheel events (sgrWheelSeq), so it scrolls its own history just
    // as a desktop mouse wheel would. If the alt-screen app has no mouse tracking
    // (or we are a read-only viewer), there is nothing to forward to, so we leave
    // the touch to native handling and let the arrow row drive it.
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
    // Set when the debounced PTY resize (sendSize, below) came due while a
    // touch-scroll gesture was still in flight. A resize is a SIGWINCH — a full
    // child repaint — and landing one in the middle of the forwarded wheel-report
    // stream corrupts a mouse-tracking alt-screen pager's repaint (duplicated
    // rows that PERSIST, since an alt-screen has no client scrollback and nothing
    // reconnects to re-sync it). This is not exotic: the scroll-start blur below
    // collapses the soft keyboard, `interactive-widget=resizes-content` then
    // grows the viewport, and the debounced resize would fire under the finger,
    // which is exactly how in-app nav (whose mount-time compose focus rides the
    // row tap's user gesture, so the keyboard IS up) corrupted the pager while a
    // hard refresh (whose programmatic focus cannot open a keyboard) did not.
    // So sendSize holds the send while `touchScrolling`, and endTouch flushes it
    // through the same debounce once the finger lifts — after the last wheel
    // report, coalescing however many container resizes the collapse produced.
    let resizeHeldByGesture = false
    let longPressTimer: ReturnType<typeof setTimeout> | undefined
    const onTouchStart = (e: TouchEvent) => {
      // Any new touch (including a second finger landing mid-gesture) supersedes
      // a pending long-press, so always cancel it first.
      clearTimeout(longPressTimer)
      // Track single-finger touches on BOTH buffers: the normal buffer scrolls
      // xterm's scrollback, the alt-screen may forward to the app (decided per
      // move in onTouchMove, since mouse-tracking state can change mid-gesture).
      if (e.touches.length !== 1) {
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
      if (!touchActive || touchSelecting || e.touches.length !== 1) return
      // Decide the target fresh each move: an agent can flip in or out of an
      // alt-screen TUI mid-drag. On the alt-screen we can only act if the app
      // takes mouse input AND we own the PTY; otherwise there is nothing to
      // forward to, so leave the touch to native handling (selection/long-press).
      const altScreen = term.buffer.active.type !== "normal"
      const forwardWheel =
        altScreen &&
        isOwnerRef.current &&
        term.modes.mouseTrackingMode !== "none"
      if (altScreen && !forwardWheel) return
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
        // Whichever surface holds it (xterm's hidden textarea or the compose
        // bar's) must let go, or the keyboard stays up over the scrollback.
        term.textarea?.blur()
        composeInputRef.current?.blur()
      }
      e.preventDefault()
      const rowHeight = container.clientHeight / term.rows
      const { scrollLines, remainderPx } = dragScrollLines(touchAccum, rowHeight)
      if (scrollLines !== 0) {
        if (forwardWheel) {
          // Forward to the full-screen app as wheel events at the finger's cell
          // (most apps ignore the position, but we send a real in-bounds one).
          // Cap to at most ONE wheel notch per touch-move (`dragWheelReport`):
          // `dragScrollLines` can return a many-row magnitude on a fast flick, and
          // `sgrWheelSeq` would then emit that many SGR reports as a dense burst in
          // a single frame. A mouse-tracking alt-screen app (Claude Code, ...)
          // survives the desktop wheel's one-report-per-discrete-event cadence but
          // not that burst — it corrupts the app's scrollback-pager repaint, and
          // because an alt-screen has no client scrollback and nothing reconnects,
          // the duplicated lines persist. One notch per move reproduces the
          // desktop 1:1 cadence while still tracking the finger across moves.
          const { notch } = dragWheelReport(touchAccum, rowHeight)
          const colWidth = container.clientWidth / term.cols
          const rect = container.getBoundingClientRect()
          const col =
            Math.floor(
              (e.touches[0].clientX - rect.left) / (colWidth > 0 ? colWidth : 1),
            ) + 1
          const cellRow =
            Math.floor((y - rect.top) / (rowHeight > 0 ? rowHeight : 1)) + 1
          pty.sendInput(encoder.encode(sgrWheelSeq(notch, col, cellRow)))
        } else {
          term.scrollLines(scrollLines)
        }
        touchAccum = remainderPx
      }
    }
    const endTouch = () => {
      clearTimeout(longPressTimer)
      touchActive = false
      touchScrolling = false
      touchSelecting = false
      // Flush a resize the gesture held back (see resizeHeldByGesture above):
      // the wheel-report stream ends with the finger, so re-arming the normal
      // debounce here sends one resize, at the final size, after the stream.
      if (resizeHeldByGesture) {
        resizeHeldByGesture = false
        clearTimeout(sendTimer)
        sendTimer = setTimeout(sendSize, RESIZE_SEND_DEBOUNCE_MS)
      }
    }
    // Tap-to-focus redirect for the compose bar. xterm grabs focus from a
    // `mousedown` listener on its element (`ev.preventDefault(); this.focus()`,
    // see CoreBrowserTerminal), and on touch that mousedown is the SYNTHETIC
    // one the browser dispatches after `touchend`. So when the compose bar is
    // up and this client owns the input, a plain TAP (the gesture the
    // disambiguator above left as neither a scroll nor a long-press)
    // `preventDefault`s the touchend (suppressing the synthetic mouse events,
    // so xterm never focuses its hidden textarea) and focuses the compose
    // textarea instead: the soft keyboard always opens into the buffer.
    //
    // Swallowing those synthetic mouse events also swallows the CLICK a
    // mouse-tracking app would have received through xterm's mouse pipeline,
    // and full-screen TUIs (menus, buttons) are driven by exactly that click.
    // So when the app has mouse tracking on, we forward the tap ourselves as a
    // synthetic SGR left click (press + release) at the tapped cell, computed
    // from `changedTouches[0]` (`touches` is already empty on touchend) with
    // the same cell math as the drag path above. This restores the BEHAVIOR
    // the app saw before the redirect existed, not a byte-exact replay of the
    // browser's event pipeline; apps consume the click, not the DOM events.
    //
    // Scroll and long-press selection take the other branches and are
    // untouched, a non-owner is covered by the take-over overlay anyway, and
    // with the preference off (or on desktop) the redirect never fires, so a
    // tap reaches xterm exactly as today. The listener is registered
    // non-passive UNCONDITIONALLY (even when the redirect never fires): a
    // deliberate, harmless choice, since touchend passivity does not gate the
    // browser's scroll optimizations the way touchmove's does.
    const onTouchEnd = (e: TouchEvent) => {
      const wasTap = touchActive && !touchScrolling && !touchSelecting
      endTouch()
      if (!wasTap) return
      if (!composeActiveRef.current || !isOwnerRef.current) return
      const compose = composeInputRef.current
      if (!compose) return
      e.preventDefault()
      const touch = e.changedTouches[0]
      if (touch && term.modes.mouseTrackingMode !== "none") {
        // Mirror onTouchMove's wheel-coordinate math: 1-based cell from the
        // touch point, with the divide-by-zero guards for a not-yet-measured
        // container.
        const rect = container.getBoundingClientRect()
        const colWidth = container.clientWidth / term.cols
        const rowHeight = container.clientHeight / term.rows
        const col =
          Math.floor((touch.clientX - rect.left) / (colWidth > 0 ? colWidth : 1)) + 1
        const cellRow =
          Math.floor((touch.clientY - rect.top) / (rowHeight > 0 ? rowHeight : 1)) + 1
        pty.sendInput(encoder.encode(sgrClickSeq(col, cellRow)))
      }
      compose.focus()
    }
    container.addEventListener("touchstart", onTouchStart, { passive: true })
    container.addEventListener("touchmove", onTouchMove, { passive: false })
    container.addEventListener("touchend", onTouchEnd, { passive: false })
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
      // Never land a SIGWINCH inside an active touch-scroll's wheel-report
      // stream: hold the send and let endTouch flush it after the finger lifts
      // (see resizeHeldByGesture in the touch block above).
      if (touchScrolling) {
        resizeHeldByGesture = true
        return
      }
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
    // Whether the NEXT first-frame resize should jiggle (very first open) or send
    // a single plain resize (a reconnect). `onOpen` sets this before the first
    // frame lands; it defaults to `true` so the very first open still jiggles even
    // in the pathological case where the fallback timer beats `onOpen`.
    let firstFrameIsFirstOpen = true
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
      if (firstFrameResizePlan(firstFrameIsFirstOpen) === "jiggle") {
        // FIRST open only: force the agent to FULLY redraw at our size now that the
        // first paint has landed. A same-size resize is a kernel no-op (no
        // SIGWINCH), so when the PTY already matches this viewport the agent never
        // repaints and the initial snapshot (imperfect for a tall buffer with a
        // bottom-anchored prompt) stays on screen with the cursor and input box
        // misplaced. Nudge the width down one column and back: each step is a real
        // winsize change, so the kernel raises SIGWINCH and the agent redraws its
        // true UI, ending at the correct size. This automates the manual
        // divider-nudge that reliably fixed it.
        sendOwnedResize(term.rows, Math.max(1, term.cols - 1))
        jiggleTimer = setTimeout(() => {
          sendOwnedResize(term.rows, term.cols)
        }, 60)
      } else {
        // RECONNECT: the server kept the PTY alive at its prior size and replays a
        // fresh repaint as this first frame. Jiggling here would force TWO
        // full-screen agent repaints (at two widths) on EVERY reconnect, and mobile
        // reconnects constantly. Send a SINGLE resize to our true size instead: it
        // still re-asserts ownership, it is a kernel no-op (no repaint) when the
        // size is unchanged, and it raises exactly one natural SIGWINCH (one
        // repaint) only when the viewport genuinely changed while disconnected.
        sendOwnedResize(term.rows, term.cols)
      }
    }
    // On a RECONNECT the server replays the FULL scrollback as the first binary
    // frame. xterm still holds the buffer from before the drop, so writing the
    // replay on top would stack a second copy of history (duplicated/garbled
    // output). Reset xterm before that first reconnect frame so the replay
    // rebuilds the buffer cleanly. The very FIRST open starts from an empty buffer
    // (a fresh terminal), so it needs no reset — only opens after the first do.
    //
    // `reset()` clears every private MODE too (mouse tracking, bracketed paste,
    // cursor visibility, autowrap, application cursor keys), and the child emitted
    // those once at its own startup and never repeats them, so nothing on the live
    // stream puts them back. The repaint therefore carries an explicit mode-restore
    // tail from the server (`dux_core::pty::mode_restore_sequence`). Do not try to
    // infer modes here from what the replay draws. Without it a reconnect landed on
    // a full-screen agent with `mouseTrackingMode === "none"`, and the touch-scroll
    // forward path below (gated on exactly that) returned before it read the finger
    // delta, so a finger drag did nothing at all until a hard refresh.
    //
    // On mobile the socket reconnects constantly, so two defensive guards keep a
    // replay from ever stacking (Mechanism A):
    //  1. Idempotency by generation. The `connected` frame tags each replay with a
    //     monotonic generation (see ptySocket.ts / replayGeneration.ts). We record
    //     the last generation applied and DROP any replay whose generation we have
    //     already applied — a duplicate replay, or a late blob from a torn-down
    //     forwarder, becomes a no-op instead of a second copy of history.
    //  2. Drain-gating. Before resetting and replaying we let the PREVIOUS
    //     connection's xterm write queue fully drain (the empty-write callback fires
    //     only once queued writes have parsed — the same gate the focus/visibility
    //     re-assert uses below), so a stale queued byte cannot land after reset()
    //     and among the replay. Because the empty-write callback is async, any bytes
    //     that arrive during the drain window are HELD and replayed in order after
    //     the reset, so nothing is reordered or written ahead of the fresh replay.
    let firstOpen = true
    let awaitingRepaint = false
    let repaintNeedsReset = false
    let lastAppliedGen: number | null = null
    // Set only while draining the previous connection's write queue; incoming bytes
    // are buffered here (repaint first, then any live bytes) and flushed in order
    // once the drain completes so nothing is written ahead of the reset+replay.
    let draining = false
    let heldChunks: Uint8Array[] = []
    const writeChunk = (bytes: Uint8Array) => {
      if (!initialResizeDone) {
        // Resize only once xterm has parsed this first frame (the repaint).
        term.write(bytes, sendInitialResize)
      } else {
        term.write(bytes)
      }
    }
    pty.onBytes((bytes) => {
      // Mid-drain: hold everything (the repaint plus any live bytes that raced in)
      // so it lands in order after reset(), never ahead of the fresh replay.
      if (draining) {
        heldChunks.push(bytes)
        return
      }
      if (awaitingRepaint) {
        awaitingRepaint = false
        const gen = pty.replayGeneration
        if (!shouldApplyReplay(gen, lastAppliedGen)) {
          // A replay we have already applied (duplicate or stale/late blob): drop it
          // entirely (no reset, no write) so it can never stack a second copy.
          return
        }
        lastAppliedGen = nextAppliedGeneration(gen, lastAppliedGen)
        if (repaintNeedsReset) {
          // Reconnect replay: drain the previous connection's queue, then reset and
          // replay (plus any raced-in live bytes) in order.
          draining = true
          heldChunks = [bytes]
          term.write("", () => {
            term.reset()
            const chunks = heldChunks
            heldChunks = []
            draining = false
            for (const c of chunks) writeChunk(c)
          })
        } else {
          // Very first open: the buffer is already empty, so no reset or drain is
          // needed — write the repaint straight through.
          writeChunk(bytes)
        }
        return
      }
      writeChunk(bytes)
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
      // Retire the stale id from the store's own-connection set too: the server
      // has already released anything it owned, so keeping it would make a spine
      // field naming it read as "mine" when it no longer is.
      if (myConnIdRef.current !== null) {
        noteOwnPtyConnection(myConnIdRef.current, false)
      }
      myConnIdRef.current = null
      initialResizeDone = false
      setReconnecting(false)
      // The next binary frame is this open's scrollback replay: arm the repaint
      // handling (generation-drop + resize). Only opens AFTER the first also reset
      // the buffer first, since the first open starts from an empty terminal.
      awaitingRepaint = true
      if (firstOpen) {
        firstOpen = false
        repaintNeedsReset = false
        firstFrameIsFirstOpen = true
      } else {
        repaintNeedsReset = true
        // A reconnect must NOT jiggle: an unchanged size would double-repaint the
        // agent on every mobile reconnect. The first-frame resize sends a single
        // plain resize instead (see `sendInitialResize`).
        firstFrameIsFirstOpen = false
      }
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
      if (myConnIdRef.current !== null) {
        noteOwnPtyConnection(myConnIdRef.current, false)
        myConnIdRef.current = null
      }
    }
    // Connection-state transitions. The one we act on is `failed`: the PTY socket
    // now shares the events socket's 3-attempt cap, so when its budget is spent it
    // STOPS (no more silent behind-the-overlay reattach). Swap the endless
    // "Reconnecting…" cue for an explicit "connection lost — Reconnect" affordance.
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
      pty.shouldRetry = () => !isTabGone(sessionTabsRef.current ?? [], id)
      pty.onGone = () => {
        handleTabGone(id)
      }
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
      sendTimer = setTimeout(sendSize, RESIZE_SEND_DEBOUNCE_MS)
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
          isOwner: isOwnerRef.current,
          visible: document.visibilityState === "visible",
          now: Date.now(),
          visibleSince: visibleSinceRef.current,
          graceMs: attentionGraceMsRef.current,
        })
      ) {
        pty.sendViewed()
      }
    }
    const viewedTimer = setInterval(pingViewed, VIEWED_PING_INTERVAL_MS)

    let resyncTimer: ReturnType<typeof setTimeout> | undefined
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
      const graceMs = attentionGraceMsRef.current
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
      clearTimeout(graceTimer)
      clearTimeout(longPressTimer)
      clearInterval(viewedTimer)
      container.removeEventListener("mousedown", onMouseDown)
      container.removeEventListener("mouseup", onMouseUp)
      container.removeEventListener("contextmenu", onContextMenuPasteGuard)
      container.removeEventListener("touchstart", onTouchStart)
      container.removeEventListener("touchmove", onTouchMove)
      container.removeEventListener("touchend", onTouchEnd)
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
      // The socket dies with the pane, and so does its connection id: retire
      // it from the store's own-connection set (the server releases anything
      // it owned the moment the socket closes).
      if (myConnIdRef.current !== null) {
        noteOwnPtyConnection(myConnIdRef.current, false)
        myConnIdRef.current = null
      }
      pty.close()
      if (ptyRef.current === pty) ptyRef.current = null
      if (getActivePtySocket() === pty) setActivePtySocket(null)
      termRef.current = null
      disposeAgentNotifications()
      disposeOsc8Gate.dispose()
      term.dispose()
    }
  }, [kind, id, sessionId, ptyUrl])

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
    return onPtyOwner((ptyId, ownerId, device) => {
      if (ptyId !== id) return
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      // Flip the ref synchronously so an in-flight keystroke is gated by the new
      // state at once, then re-render into the owner view or take-over placeholder.
      isOwnerRef.current = mine
      setIsOwner(mine)
      // Remember which device took over (for the placeholder's copy) while we are
      // demoted; clear it the moment we are the owner again.
      setTakeoverDevice(mine ? null : (device ?? null))
    })
  }, [id])

  // Publish this pane's ownership verdict into the store ledger so surfaces
  // OUTSIDE the pane (the agent ⋯ menu) can disable mutating actions while
  // another device drives the agent. Agent PTYs only: a companion terminal
  // taken over elsewhere says nothing about the agent itself. The verdict is
  // the ledger's fast path in BOTH directions: "elsewhere" gates the menu the
  // moment the handover frame lands, and "mine" un-gates it right after a
  // take-over, while the spine's `input_owner` still names the previous owner
  // until the refetch. "mine" starts as the same optimistic foreground guess
  // `isOwner` itself starts from (the foregrounded pane claims via its first
  // resize moments later); it is corrected by the `pty.owner` handovers. A
  // pane whose socket has FAILED for good publishes no verdict at all: its
  // belief is about a connection that no longer exists, and a stale "mine"
  // would override the server field forever on a surface that cannot type.
  // The cleanup retires the verdict; it also runs between re-publishes (any
  // dep flip), which is harmless because the new verdict lands in the same
  // synchronous pass, and on unmount it is what hands the answer back to the
  // server-published spine field alone.
  useEffect(() => {
    if (kind !== "agent") return
    if (connectionLost) {
      noteAgentPtyOwnership(id, "unknown")
      return
    }
    noteAgentPtyOwnership(id, isOwner ? "mine" : "elsewhere")
    return () => noteAgentPtyOwnership(id, "unknown")
  }, [kind, id, isOwner, connectionLost])

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
        graceMs: attentionGraceMsRef.current,
      })
    ) {
      ptyRef.current?.sendViewed()
    }
  }, [isOwner])

  // Reclaim ownership from another device. Sending our current size IS the claim
  // server-side (most-recent claim wins), so the PTY snaps back to this viewport
  // and our input is forwarded again. Flip the ref synchronously (so the resize
  // passes the owner gate before the state re-render lands), then refocus. The
  // server's resulting `pty.owner` carries our connection id, so the handover
  // handler recognises it as ours by id and keeps us the owner.
  function takeOver() {
    isOwnerRef.current = true
    setIsOwner(true)
    // Clear the other device's name as we optimistically claim ownership, honoring
    // the invariant that `takeoverDevice` only names a device we do NOT own.
    setTakeoverDevice(null)
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
    // Refocus the active typing surface (the compose textarea when the mobile
    // compose bar is up, xterm's hidden textarea otherwise) so typing resumes
    // where it belongs the moment ownership returns.
    focusTypingSurface()
  }

  // Right-click pastes the browser clipboard (classic terminal: selecting copies
  // via copy-on-select, right-click pastes). Gated on ownership (a read-only
  // viewer can't drive input). Needs a secure context for `readText`;
  // pasteIntoTerm toasts a "use Ctrl+v" hint when the clipboard can't be read
  // (plain-HTTP).
  function onRightClickPaste() {
    const term = termRef.current
    if (term && isOwnerRef.current) pasteIntoTerm(term, focusTypingSurface)
  }

  // Save each dropped file, then paste its path.
  //
  // Sequential on purpose. The list of outcomes is in DROPPED order, and that is
  // also the order the paths are sent, which must not become whichever order the
  // uploads happen to finish in. One toast reports the whole drop at the end, so
  // a handful of files does not bury the screen.
  //
  // The FORM each path takes is per-CLI, because the agent CLIs do not agree on
  // how they read a pasted path (see `pastePayload`), and so is the length limit
  // beside it. Both come out of ONE resolved profile: what the focused tab's live
  // process launched with, off the spine (so a launch or a termination refreshes
  // it), falling back to what config says for its provider, off the bootstrap
  // document (so a `config.changed` refetch refreshes that).
  //
  // A TERMINAL is not a provider pane and never reads that setting: it runs a
  // SHELL, which is exactly why its path is always quoted rather than left bare
  // (see `TERMINAL_PASTE_FORM`). The owning session's provider is not consulted
  // either, for the separate reason that a companion terminal is not that agent.
  //
  // The form is resolved IMMEDIATELY BEFORE EACH PASTE, out of refs, for the same
  // reason the ownership and socket checks are: a drop's uploads are sequential,
  // so a config reload or a provider retarget can land between two files, and a
  // form snapshotted once at the top of the drop would silently outlive it.
  //
  // `toastId` is THIS drop's own sonner id, minted by `runDrop`. See
  // `nextFileDropToastId`: two quick drops sharing one id lose the first one's
  // report under the second one's spinner.
  async function handleDroppedFiles(files: File[], toastId: string) {
    if (files.length === 0) return
    const outcomes: DropOutcome[] = []

    for (const [i, file] of files.entries()) {
      // A spinner for THIS file, before the request goes out. The drop overlay
      // is already gone by now (`onDrop` clears it the moment the browser hands
      // the files over), and an upload can wait a bounded but real amount of
      // time for a server-side slot, so without this the interface returns to
      // normal and nothing visibly happens. Uploads are sequential, so a
      // multi-file drop counts through them rather than sitting on one message.
      //
      // Same sonner id as the report at the end of THIS drop, so the final
      // REPLACES the spinner in place rather than stacking a second toast, and
      // a concurrent drop cannot paint over either of them.
      showBusyToast(
        files.length === 1
          ? `Uploading ${file.name}...`
          : `Uploading ${file.name} (${i + 1} of ${files.length})...`,
        { id: toastId },
      )
      let saved
      try {
        saved = await uploadDroppedFile(file, {
          pty: id,
          // The TERMINAL SOCKET's id, not the events-socket one the other API
          // modules stamp in a header (the server refuses a PTY id there).
          conn: myConnIdRef.current,
        })
      } catch (e) {
        outcomes.push({
          kind: "refused",
          requestedName: file.name,
          // The STATUS decides the wording, not just the message: a 503 means
          // no upload slot came free and is worth retrying in a moment, which
          // is advice no other failure here deserves.
          reason:
            e instanceof FileDropApiError
              ? dropRefusalReason(e.status, e.message)
              : "the upload failed",
        })
        continue
      }
      // The folder travels with THIS file, not with the drop. A terminal's
      // directory changes the moment someone types `cd`, and these uploads are
      // sequential, so two files dropped together really can land in two
      // folders; keeping one label for the whole drop reported the last one for
      // all of them.
      const where = {
        requestedName: saved.requested_name,
        savedName: saved.saved_name,
        path: saved.path,
        folderLabel: saved.folder_label,
      }

      // Both checks happen IMMEDIATELY BEFORE this paste, not once at the start
      // of the drop: ownership can move and the socket can close between two
      // files. A write to a closed socket is dropped silently, so without the
      // second check a file would be reported as sent with nothing written.
      const term = termRef.current
      if (!isOwnerRef.current) {
        outcomes.push({
          kind: "saved-not-sent",
          ...where,
          reason: "another device took over input",
        })
        continue
      }
      if (!term || !(ptyRef.current?.isOpen ?? false)) {
        outcomes.push({
          kind: "saved-not-sent",
          ...where,
          reason: "the connection dropped",
        })
        continue
      }

      // Resolved here, per file, rather than once per drop: see the note above.
      // The FORM and the CLI's character LIMIT come out together, keyed by the
      // same target, so neither can be derived from the other: a terminal is a
      // shell and has no limit whatever form it uses, and codex has its limit on
      // every form it can be configured with.
      const { form, charLimit } = dragDropPasteFor(
        configuredDropPasteRef.current,
        kind === "agent"
          ? {
              kind: "agent",
              launched: launchedDropPasteRef.current,
              provider: providerNameRef.current,
            }
          : { kind: "terminal" },
      )
      const payload = pastePayload(where.path, form)
      // Too long for the receiving CLI to look at as a path. Codex files any
      // paste over its threshold away as generic large content before it tries
      // to recognize a path at all, so pasting this would put a placeholder in
      // the prompt and attach nothing, while the toast claimed success. Report
      // it as the stranded file it is: saved, here is the full path, go and
      // reference it yourself.
      if (charLimit !== null && pasteExceedsAttachmentLimit(payload, charLimit)) {
        outcomes.push({
          kind: "saved-not-sent",
          ...where,
          reason: tooLongToAttachReason(charLimit),
        })
        continue
      }

      // xterm's own paste, which applies bracketed paste (DECSET 2004) when the
      // running program asked for it and sends plain text when it did not.
      // Building the bracket markers by hand here would be a second
      // implementation of something that already works.
      //
      // This deliberately differs from the compose bar, which refuses bracketed
      // paste. That rule exists because compose text has to keep a soft line
      // break and a submitting Enter distinct on the wire. A dropped path
      // contains neither, so the reason does not apply here.
      term.paste(payload)
      // SENT, not "arrived". This is a socket write like any keystroke and
      // nothing acknowledges it: a take-over landing between the upload's
      // courtesy check and this frame reaching the server makes the server drop
      // it silently, so the toast claims only what dux knows.
      outcomes.push({ kind: "sent", ...where })
    }

    const ctx: DropContext = {
      kind: props.kind === "agent" ? "agent" : "terminal",
    }
    const report = dropToastFor(outcomes, ctx)
    // Through the SHARED final-toast raiser, so the user's configured dismiss
    // window applies. A bare sonner call would silently use the library default.
    // It also retires the spinner's leak guard, since it lands on the same id.
    showFinalToast(report.tone, report.message, {
      id: toastId,
      statusClearSeconds: bootstrap?.status_clear_seconds,
    })
  }

  /// Raise the drop's spinner and make sure something final always replaces it.
  ///
  /// The loop's per-file failures are already outcomes, so the only way out
  /// without a report is an unexpected throw. `handleDroppedFiles` is called
  /// with `void`, so that throw would become an unhandled rejection and leave
  /// the spinner on screen until its leak guard expires a minute later, still
  /// claiming the upload is running.
  ///
  /// The id is minted HERE, once per drop, and handed to both halves, so a
  /// second drop starting while this one is still uploading cannot land its
  /// spinner on this drop's report.
  async function runDrop(files: File[]) {
    const toastId = nextFileDropToastId()
    try {
      await handleDroppedFiles(files, toastId)
    } catch (e) {
      showFinalToast(
        "error",
        `The drop failed unexpectedly: ${e instanceof Error ? e.message : String(e)}`,
        {
          id: toastId,
          statusClearSeconds: bootstrap?.status_clear_seconds,
        },
      )
    }
  }

  // A drag from a non-owner, on a phone (where there is no drag), or while file
  // drop is switched off is left entirely alone: no overlay and no
  // preventDefault, so the browser does whatever it would normally do.
  //
  // `[server] file_drop_max_bytes = 0` is documented as switching file drop off,
  // and the server refuses every upload when it is. The server stays the real
  // enforcement; this gate is what stops a disabled feature ADVERTISING a drop
  // target, accepting the drop and only then reporting a refusal per file. It
  // is closed while the setting is merely UNKNOWN too, so nothing is offered
  // before dux can say the feature is there (see `fileDropEnabled`).
  function dragCarriesFiles(e: React.DragEvent): boolean {
    return (
      fileDropEnabled &&
      isOwner &&
      !isMobile &&
      Array.from(e.dataTransfer.types).includes("Files")
    )
  }

  // Where typing focus belongs right now: the compose textarea while the
  // mobile compose bar is up (so the soft keyboard keeps typing into the
  // buffer), xterm's hidden textarea otherwise. Every handler that used to
  // refocus the terminal after acting routes through this, keeping the
  // accessory-bar contract (a bar key never steals focus from the active
  // typing surface) intact for both surfaces.
  function focusTypingSurface() {
    if (composeActiveRef.current && composeInputRef.current) {
      composeInputRef.current.focus()
    } else {
      termRef.current?.focus()
    }
  }

  // Accessory-bar key sends. Esc/Tab/arrows are full sequences, not single
  // chars, so they bypass `applyModifiers` (which only transforms single-char
  // input). We still honor a latched Alt by prefixing ESC, and we clear any
  // latch one-shot afterward — Ctrl on a non-char key has no meaning here, so
  // it's simply consumed. Sends go through the same socket path as typed input.
  const encoder = new TextEncoder()

  // The compose bar's Send: deliver the buffered message, then submit it.
  // The write plan lives in the pure `composeSendWrites`: the MACRO keystroke
  // convention (newlines are Alt+Enter, ESC CR, exactly like
  // `macroPayloadBytes`) as the body write, and the submitting bare CR as a
  // SEPARATE write the timeout below delivers COMPOSE_SUBMIT_DELAY_MS later.
  // Deliberately NOT bracketed paste, and no read of `bracketedPasteMode`;
  // and the Enter travels alone because Claude Code merges stdin chunks into
  // one paste through a measured 50ms debounce that would swallow a
  // same-window CR into the paste as a newline (see COMPOSE_SUBMIT_DELAY_MS).
  // An empty buffer is a single immediate bare CR, a lone Enter keystroke.
  // The shared landing-effects writer replays the scroll-to-live-edge and
  // selection-drop a typed key would get, ONCE, with the first write. Focus
  // stays in the compose textarea (the Send button preventDefaults its
  // pointerdown, so it never left).
  //
  // Returns whether the send happened; the bar clears its buffer only on
  // true. A composed message can be minutes of typing, so unlike a keystroke
  // (cheap to re-type, silently droppable) every refused send KEEPS the buffer
  // and toasts the reason: not the input owner (like every write path; take
  // over to reclaim), socket not open (the sendInput readyState guard would
  // silently drop the bytes), or payload over the client-side cap (an
  // oversized frame would make the server abort the whole socket, see
  // MAX_COMPOSE_SEND_BYTES).
  //
  // Deliberately does NOT consume the one-shot Ctrl/Alt accessory latches: a
  // latch arms the next direct KEY, and a composed message is not a key; a
  // user who tapped Ctrl intending Ctrl-c should not lose the latch to an
  // unrelated Send.
  function sendCompose(text: string): boolean {
    if (!isOwnerRef.current) {
      toast.error("Another device is driving this terminal. Take over to send.", {
        id: "compose-send",
      })
      return false
    }
    if (!(ptyRef.current?.isOpen ?? false)) {
      toast.error("Not connected right now. Your message was kept.", {
        id: "compose-send",
      })
      return false
    }
    const writes = composeSendWrites(text)
    const totalBytes = writes.reduce((n, w) => n + w.byteLength, 0)
    if (composeSendTooLarge(totalBytes)) {
      toast.error("Message too large to send. Trim it down and try again.", {
        id: "compose-send",
      })
      return false
    }
    writeInputWithLandingEffects(termRef.current, ptyRef.current, writes[0])
    // A two-write plan: the submitting CR follows after the measured-safe gap
    // (see composeSendWrites). The send is committed at this point, hence
    // `true` below; the delayed CR is a bare PTY write with no further side
    // effects. Guards: the pane may unmount (its cleanup nulls `ptyRef`, so
    // the identity check fails) or the socket may drop (`isOpen`) before the
    // timer fires; in either case the orphaned CR is skipped rather than
    // delivered to a socket this pane no longer drives.
    if (writes.length > 1) {
      const pty = ptyRef.current
      const rest = writes.slice(1)
      setTimeout(() => {
        if (pty === null || ptyRef.current !== pty || !pty.isOpen) return
        for (const w of rest) pty.sendInput(w)
      }, COMPOSE_SUBMIT_DELAY_MS)
    }
    return true
  }

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
    focusTypingSurface()
  }

  function onArrow(dir: "up" | "down" | "left" | "right") {
    const app = termRef.current?.modes.applicationCursorKeysMode ?? false
    sendSeq(arrowSeq(dir, app))
  }

  // The accessory bar's ⇧↵ key — the touch equivalent of Shift-Enter, since a
  // soft keyboard can't produce that chord. Owner-gated like every accessory
  // send; consumes any armed Ctrl/Alt latch (a raw newline doesn't combine with
  // them, so unlike `sendSeq` it never routes through `applyModifiers`) and keeps
  // focus so the user keeps typing. Shares `writeSoftNewline` with the physical
  // Shift-Enter handler so both land input identically.
  function sendNewline() {
    if (!isOwnerRef.current) return
    if (modsRef.current.ctrl || modsRef.current.alt) {
      setMods({ ctrl: false, alt: false })
    }
    writeSoftNewline(termRef.current, ptyRef.current)
    focusTypingSurface()
  }

  function toggleCtrl() {
    setMods({ ctrl: !modsRef.current.ctrl, alt: modsRef.current.alt })
    focusTypingSurface()
  }

  function toggleAlt() {
    setMods({ ctrl: modsRef.current.ctrl, alt: !modsRef.current.alt })
    focusTypingSurface()
  }

  // Scroll the xterm viewport from the accessory bar's second row. On the normal
  // buffer these drive xterm's own scrollback (the history that accumulates as
  // the agent streams output), giving a reliable touch target the slim scrollbar
  // can't.
  //
  // On the ALT-SCREEN (a full-screen TUI) xterm has no scrollback, so PgUp/PgDn
  // forward a page to the app itself, mirroring the TUI's forward-scroll: a
  // mouse-tracking app (Claude, Codex, ...) gets a screenful of wheel events; a
  // keyboard-only app gets the PgUp/PgDn keys. Jump-to-top/bottom has no clean
  // wheel equivalent, so those two stay scrollback-only and are a no-op on the
  // alt-screen — the cursor-arrow row drives fine-grained movement there.
  //
  // Scrolling is a READ gesture, so it drops the hidden textarea's focus: that
  // slides the soft keyboard away to free the whole screen for reading back and,
  // crucially, stops a scroll-button tap from re-summoning it. On iOS the
  // textarea stays the focused element after the user swipes the keyboard down,
  // so any later tap on a focus-retaining (preventDefault) button pops it right
  // back up; blurring here is what keeps it down. Tapping the terminal refocuses
  // to resume typing. (The input keys — Esc/Tab/Ctrl/Alt/newline and the cursor
  // arrows — instead KEEP focus; only PgUp/PgDn blur. It's an input vs
  // page-scroll split, not a row split.)
  function onScroll(dir: ScrollDir) {
    const term = termRef.current
    if (!term) return
    const altScreen = term.buffer.active.type !== "normal"
    // On the alt-screen, a Page button forwards to the full-screen app (input,
    // so only when we own the PTY); top/bottom have no wheel equivalent and fall
    // through to the local scroll, which is a no-op there.
    if (
      altScreen &&
      isOwnerRef.current &&
      (dir === "pageUp" || dir === "pageDown")
    ) {
      const up = dir === "pageUp"
      if (term.modes.mouseTrackingMode !== "none") {
        // A screenful of wheel notches toward older (up) or newer (down) output.
        // The exact distance depends on the app's per-notch step; one row-height
        // shy of a full screen is a reasonable page.
        const lines = Math.max(1, term.rows - 1)
        const col = Math.max(1, Math.floor(term.cols / 2))
        const cellRow = Math.max(1, Math.floor(term.rows / 2))
        const seq = sgrWheelSeq(up ? -lines : lines, col, cellRow)
        ptyRef.current?.sendInput(encoder.encode(seq))
      } else {
        // Keyboard-only full-screen app: send the actual PgUp/PgDn key.
        ptyRef.current?.sendInput(encoder.encode(pageKeySeq(up ? "up" : "down")))
      }
      if (navigator.maxTouchPoints > 0) {
        term.textarea?.blur()
        // The compose textarea holds the keyboard when the compose bar is up;
        // a page-scroll is a reading gesture on either surface, so let it go.
        composeInputRef.current?.blur()
      }
      return
    }
    switch (dir) {
      case "pageUp":
        term.scrollPages(-1)
        break
      case "pageDown":
        term.scrollPages(1)
        break
    }
    // Only a touch device has a soft keyboard to dismiss. Gating on touch
    // capability stops a narrow-window mouse user (who also gets this mobile bar)
    // from silently losing terminal focus when paging through output. The
    // compose textarea can be the keyboard's holder too, so both surfaces let go.
    if (navigator.maxTouchPoints > 0) {
      term.textarea?.blur()
      composeInputRef.current?.blur()
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
  // the pane covers every host: the desktop ResizablePanel and the mobile
  // viewport-pinned root. The overlays (macro popover, readiness card) are
  // absolutely positioned inside these bounds, so clipping never affects them.
  // Human label for the device that took over ("Chrome on macOS"), or null when the
  // other device's `User-Agent` was absent/unrecognized (the modal then shows a
  // generic fallback). Parsing lives in the pure, tested `deviceLabel` helper.
  const takeoverLabel = deviceLabel(takeoverDevice)
  const pane = (
    <div
      className={
        isMobile
          ? "group relative min-h-0 w-full flex-1 overflow-hidden bg-background"
          : "group relative h-full w-full overflow-hidden bg-background"
      }
      onDragEnter={(e) => {
        if (!dragCarriesFiles(e)) return
        e.preventDefault()
        dragDepthRef.current += 1
        setDragActive(true)
      }}
      onDragOver={(e) => {
        if (!dragCarriesFiles(e)) return
        // Without preventDefault on dragover the browser refuses the drop and
        // navigates to the file instead, which loses the whole page.
        e.preventDefault()
        e.dataTransfer.dropEffect = "copy"
      }}
      onDragLeave={(e) => {
        if (!dragCarriesFiles(e)) return
        dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
        if (dragDepthRef.current === 0) setDragActive(false)
      }}
      onDrop={(e) => {
        if (!dragCarriesFiles(e)) return
        e.preventDefault()
        dragDepthRef.current = 0
        setDragActive(false)
        void runDrop(Array.from(e.dataTransfer.files))
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
              ? "It lands in this agent's worktree root, where git can see it."
              : "It lands in the folder this terminal is currently in."}
          </p>
        </div>
      ) : null}
      {/* Padding lives on the host, NOT the measured element below — see the
          hostRef comment: border-box computed heights include padding, and
          FitAddon would mint a phantom row/column from it. A mouse/pen right-click
          opens our controlled clipboard menu at the cursor; a TOUCH long-press
          (which fires `contextmenu` on Android) is left to the native
          selection gesture — see `pointerTypeRef`. */}
      <div
        ref={hostRef}
        className="h-full w-full p-2"
        onPointerDown={(e) => {
          pointerTypeRef.current = e.pointerType
        }}
        onContextMenu={(e) => {
          // Touch long-press: leave the OS's native text-selection gesture alone.
          if (pointerTypeRef.current === "touch") return
          // Mouse/pen right-click pastes the clipboard (classic terminal model,
          // no menu). preventDefault suppresses the native browser menu; the
          // contextmenu textarea-wipe (mount effect) kills xterm's own right-click
          // selection-stuffing so only the clipboard is pasted.
          e.preventDefault()
          onRightClickPaste()
        }}
      >
        {/* data-testid: the touch-gesture and tap-redirect tests dispatch real
            TouchEvents on this exact node (the element the gesture listeners
            are registered on); there is no accessible role to query it by. */}
        <div
          ref={containerRef}
          data-testid="terminal-container"
          className="h-full w-full"
        />
      </div>
      {/* Pane chrome. An absolutely-positioned overlay (a sibling of the xterm
          host, NOT inside the unpadded containerRef xterm opens into) so it never
          changes the terminal's box measurement — see the hostRef comment. The
          right offset reserves the xterm scrollbar gutter so the button never
          overlaps the scrollbar: 0.5rem MUST match the host's `p-2` padding below,
          then the shared --xterm-scrollbar-width (fallback keeps the offset valid
          if the var is ever missing), then a small gap. */}
      {/* DESKTOP ONLY: on a phone this floating trigger sat on top of the PTY
          text and made the text under it unreadable, so the mobile entry
          point is the terminal screen's header icon button (MobileShell),
          never this overlay. */}
      {!isMobile ? (
        <div className="absolute top-3 right-[calc(0.5rem+var(--xterm-scrollbar-width,8px)+0.25rem)] z-10 flex gap-2">
          {/* The popover trigger renders a secondary labeled Button (see
              MacroPopover). On close we hand Base UI the terminal's textarea as
              the focus target instead of calling termRef.focus() imperatively
              like the accessory-bar handlers do, because Base UI owns focus
              during a popover close — see the MacroPopover finalFocus comment. */}
          <MacroPopover
            target={macroTarget}
            finalFocus={() => termRef.current?.textarea ?? null}
          />
        </div>
      ) : null}
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
              <CardTitle>
                {takeoverLabel
                  ? `Open on ${takeoverLabel}`
                  : "Active on another device"}
              </CardTitle>
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

  // Mobile: a column root so the terminal host (flex-1 min-h-0) and the
  // accessory bar (shrink-0) stack. The MobileApp root pins the whole shell to
  // the visual viewport (and interactive-widget=resizes-content shrinks the
  // layout viewport for the soft keyboard), so this column just fills its parent
  // and the accessory bar sits on the keyboard — no per-pane keyboard sizing.
  // The ResizeObserver on the host refits + debounce-resizes the PTY when this
  // column reflows, so no extra resize wiring is needed. (The web UI has no
  // fullscreen mode — see the CLAUDE.md web tenet.)
  return (
    <div className="flex h-full w-full flex-col bg-background">
      {pane}
      {/* Typing surfaces render only for the input OWNER. When another device
          drives this PTY, the take-over card (inside `pane`) is this client's
          only interaction: hiding the accessory keys and the compose bar
          removes any surface that could even stage input at a session this
          device does not drive. The per-write owner gates (`sendSeq`,
          `sendCompose`) stay behind this as defense in depth, and the bars
          reappear the moment ownership returns. */}
      {isOwner ? (
        <>
          {/* The accessory bar is additionally gated on the
              `ui.mobile_accessory_bar` preference (default on): hiding it
              returns its two key rows to the terminal. The compose bar's
              restore button (below) and Preferences bring it back. */}
          {accessoryBarVisible ? (
            <AccessoryBar
              onEsc={() => sendSeq(ESC)}
              onTab={() => sendSeq(TAB)}
              onNewline={sendNewline}
              onArrow={onArrow}
              onScroll={onScroll}
              ctrl={ctrl}
              alt={alt}
              onToggleCtrl={toggleCtrl}
              onToggleAlt={toggleAlt}
            />
          ) : null}
          {/* The compose bar (the `ui.compose_bar` preference, default on):
              the third row, below the accessory bar's two key rows, so the
              typing surface sits directly on the soft keyboard. When the
              preference is off nothing renders and the tap-to-focus redirect
              stays dormant, so the terminal behaves exactly as it did before
              the bar existed. The draft value lives in this pane's state, so
              losing and regaining ownership keeps an in-progress draft. */}
          {composeBarEnabled ? (
            <ComposeBar
              value={composeText}
              onChange={setComposeText}
              onSend={sendCompose}
              inputRef={composeInputRef}
              showRestoreBars={anyMobileBarHidden}
              onRestoreBars={() => void restoreMobileBars()}
            />
          ) : anyMobileBarHidden ? (
            // The compose bar is off AND a bar is hidden: without this the
            // terminal screen would be completely chrome-free, and the app
            // ships as a standalone PWA where no browser Back button exists.
            // A minimal bottom row carries ONLY the same restore button the
            // compose bar would (the shared RestoreBarsButton), so the way
            // back is always one visible tap.
            <div className="flex shrink-0 items-end gap-1.5 border-t bg-background px-1 py-1">
              <RestoreBarsButton
                onRestoreBars={() => void restoreMobileBars()}
              />
            </div>
          ) : null}
        </>
      ) : null}
    </div>
  )
}
