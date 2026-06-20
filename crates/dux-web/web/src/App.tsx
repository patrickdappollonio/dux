import type * as React from "react"
import { Fragment, Suspense } from "react"
import { LogOut } from "lucide-react"

import { AddProjectDialog } from "@/components/AddProjectDialog"
import { AttachWorktreeDialog } from "@/components/AttachWorktreeDialog"
import { AppSidebar } from "@/components/Sidebar"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { CommandPalette } from "@/components/CommandPalette"
import { ChangeProviderDialog } from "@/components/ChangeProviderDialog"
import { CommitDialog } from "@/components/CommitDialog"
import { EditorOverlay } from "@/components/EditorOverlay"
import { ConfirmDeleteTerminalDialog } from "@/components/ConfirmDeleteTerminalDialog"
import { ConfirmDiscardFileDialog } from "@/components/ConfirmDiscardFileDialog"
import { CreateAgentDialog } from "@/components/CreateAgentDialog"
import { RenameSessionDialog } from "@/components/RenameSessionDialog"
import { CheckoutDefaultBranchDialog } from "@/components/CheckoutDefaultBranchDialog"
import { DeleteSessionDialog } from "@/components/DeleteSessionDialog"
import { GlobalEnvDialog } from "@/components/GlobalEnvDialog"
import { MacrosDialog } from "@/components/MacrosDialog"
import { MobileShell } from "@/components/MobileShell"
import { PrBanner } from "@/components/PrBanner"
import { ProjectInfoDialog } from "@/components/ProjectInfoDialog"
import { ProjectSettingsDialog } from "@/components/ProjectSettingsDialog"
import { RemoveProjectDialog } from "@/components/RemoveProjectDialog"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { StatusBar } from "@/components/StatusBar"
import { LoginScreen } from "@/components/LoginScreen"
import { Welcome } from "@/components/Welcome"
import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Button } from "@/components/ui/button"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import {
  SidebarInset,
  SidebarProvider,
} from "@/components/ui/sidebar"
import { Toaster } from "@/components/ui/sonner"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { paletteShortcutKeys } from "@/lib/platform"
import { UNREACHABLE_MESSAGE } from "@/lib/auth"
import {
  changesPaneVisible,
  logout,
  retryBoot,
  setPaletteOpen,
  useDux,
} from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"
import { keyboardLikelyOpen } from "@/lib/viewport"

function InsetHeader() {
  const { viewModel, selectedSessionId, selectedTarget, auth } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? viewModel?.projects.find((p) => p.id === session.project_id)
    : undefined
  // When a companion terminal is focused, surface it as a third crumb. The crumb
  // text follows the TUI precedence (foreground command if running, else label).
  const terminal =
    selectedTarget?.kind === "terminal"
      ? session?.terminals.find((t) => t.id === selectedTarget.terminalId)
      : undefined
  const terminalLabel = terminal ? terminalTitle(terminal) : undefined

  // The header details, mirroring the TUI: a flat `key: value` list joined by a
  // single separator. `terminal` only appears when a companion terminal is the
  // focused target; `terminals` (the count) only when there is at least one.
  const details: { key: string; value: string }[] = []
  if (session) {
    details.push({ key: "agent", value: session.title || session.branch_name })
    details.push({ key: "provider", value: session.provider })
    if (project?.name) details.push({ key: "project", value: project.name })
    details.push({ key: "branch", value: session.branch_name })
    if (terminalLabel) details.push({ key: "terminal", value: terminalLabel })
    if (session.terminals.length > 0) {
      details.push({ key: "terminals", value: String(session.terminals.length) })
    }
  }

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      {/* Left region shares one shrink budget so the details row clips instead of
          pushing the right-hand controls off the edge. One font (sans) throughout
          — mixing mono values with sans labels made `items-center` misalign them
          (mono/sans glyphs center differently). Distinguish key vs value by
          color/weight, not font. See the mono/sans alignment memory. */}
      <div className="flex min-w-0 flex-1 items-center gap-x-2 overflow-hidden text-sm">
        {details.map((d, i) => (
          <Fragment key={d.key}>
            {i > 0 && (
              // A thin, vertically centered divider (items-center keeps it on the
              // text's midline — a literal "|" glyph rode high).
              <span
                aria-hidden
                className="h-3 w-px shrink-0 bg-border"
              />
            )}
            <span className="shrink-0 whitespace-nowrap">
              <span className="text-muted-foreground">{d.key}: </span>
              <span className="font-medium text-foreground">{d.value}</span>
            </span>
          </Fragment>
        ))}
      </div>

      <div className="flex shrink-0 items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => setPaletteOpen(true)}
        >
          {/* Render each key as its own flex child so the keys share the button's
              sans font for consistent vertical alignment. The keycaps are
              decorative (the button already reads "Commands…"), so they're hidden
              from assistive tech. The label gets extra leading margin so the
              keycaps sit as a distinct group, set apart from the word. */}
          {paletteShortcutKeys().map((key) => (
            <span key={key} aria-hidden className="text-muted-foreground">
              {key}
            </span>
          ))}
          <span className="ms-2">Commands…</span>
        </Button>
        {auth.phase === "authed" ? (
          <div className="flex items-center gap-1.5">
            <span className="max-w-32 truncate text-sm text-muted-foreground">
              {auth.username}
            </span>
            <SimpleTooltip content="Log out">
              <Button
                variant="ghost"
                size="icon"
                aria-label="Log out"
                onClick={() => void logout()}
              >
                <LogOut />
              </Button>
            </SimpleTooltip>
          </div>
        ) : null}
      </div>
    </header>
  )
}

function TerminalArea() {
  const { viewModel, selectedSessionId, selectedTarget, terminalEpoch } =
    useDux()

  // Idle center pane: the duck + logo + a tip, exactly like the TUI's welcome
  // screen. It vanishes the moment a target is selected (the loading state is
  // the terminal pane's readiness spinner, not this).
  if (!selectedTarget) {
    return <Welcome />
  }

  // The PR belongs to the owning session, so it shows whether the agent or one
  // of its companion terminals is focused (mirroring the TUI, which shares the
  // session's PR across surfaces). Placement honours the same config the TUI
  // does: "bottom" puts the lane below the terminal, anything else above.
  const pr =
    viewModel?.sessions.find((s) => s.id === selectedSessionId)?.pr ?? null
  const bannerAtBottom = viewModel?.pr_banner_position === "bottom"

  // For an agent the streamed id is the session id; for a terminal it is the
  // terminal id. Key by that id so switching remounts the terminal cleanly.
  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.sessionId
  // A reconnect bumps `terminalEpoch` so an already-focused agent pane remounts
  // and re-subscribes to the freshly launched provider. Terminals don't
  // reconnect, so the epoch only affects the agent key.
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId

  // TerminalPane owns its own background and padding (via inline style) so the
  // padding area is seamlessly part of the terminal surface.
  // Suspense fallback is null: the lazy chunk loads fast and TerminalPane shows
  // its own readiness spinner the moment it mounts, so a fallback spinner here
  // would just double up.
  // ChunkBoundary wraps Suspense (not inside it) so a failed lazy import after a
  // server redeploy is caught and recovered instead of unmounting the tree.
  //
  // overflow-hidden is load-bearing: during a divider/window resize the
  // terminal keeps its previous size until the next-rAF refit, so for one
  // frame it overflows this box. The ResizablePanel's inner wrapper is
  // `overflow: auto` — left unclipped, that one-frame overflow sprouts real
  // div scrollbars whose width shrinks the content box, which retriggers the
  // ResizeObserver, which refits, which toggles the scrollbar again: a
  // visible jitter loop. Clipping here means the transient overflow is
  // simply invisible and the loop can never start.
  return (
    <div className="flex h-full min-h-0 flex-col">
      {pr && !bannerAtBottom ? <PrBanner pr={pr} position="top" /> : null}
      <div className="min-h-0 flex-1 overflow-hidden">
        <ChunkBoundary>
          <Suspense fallback={null}>
            <LazyTerminalPane
              key={paneKey}
              kind={selectedTarget.kind}
              id={targetId}
            />
          </Suspense>
        </ChunkBoundary>
      </div>
      {pr && bannerAtBottom ? <PrBanner pr={pr} position="bottom" /> : null}
    </div>
  )
}

// The palette, all dialogs, and the toaster live above the shell so they stay
// mounted in both the desktop and mobile layouts. Shared JSX — never duplicated.
function GlobalOverlays() {
  return (
    <>
      <CommandPalette />
      <CommitDialog />
      <EditorOverlay />
      <CreateAgentDialog />
      <RenameSessionDialog />
      <ChangeProviderDialog />
      <DeleteSessionDialog />
      <ConfirmDeleteTerminalDialog />
      <ConfirmDiscardFileDialog />
      <GlobalEnvDialog />
      <MacrosDialog />
      <ProjectInfoDialog />
      <ProjectSettingsDialog />
      <AddProjectDialog />
      <AttachWorktreeDialog />
      <RemoveProjectDialog />
      <CheckoutDefaultBranchDialog />
      <Toaster />
    </>
  )
}

function DesktopShell() {
  const dux = useDux()
  const { sidebarWidth } = dux
  // The Changes pane honours config.ui.show_changes_pane (via the ViewModel) and
  // the per-session palette/menu toggle. When hidden, the terminal panel takes
  // the full width and the handle + panel are unmounted (no leftover sliver).
  const showChanges = changesPaneVisible(dux)

  return (
    <SidebarProvider
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
    >
      <AppSidebar />
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        <InsetHeader />
        <div className="min-h-0 flex-1">
          <ResizablePanelGroup orientation="horizontal" className="size-full">
            {/* The terminal panel's defaultSize drops to 100 when the Changes
                panel is absent so it fills the width (no leftover sliver). The
                ids keep the two panels stable across the conditional mount.
                Note: a user-dragged split is NOT yet persisted across hide/show
                (defaultSize only applies on mount). */}
            <ResizablePanel
              id="terminal-pane"
              defaultSize={showChanges ? 74 : 100}
              minSize={30}
            >
              <TerminalArea />
            </ResizablePanel>
            {showChanges ? (
              <>
                <ResizableHandle />
                <ResizablePanel
                  id="changes-pane"
                  defaultSize={26}
                  minSize={14}
                  collapsible
                >
                  <ChangedFiles />
                </ResizablePanel>
              </>
            ) : null}
          </ResizablePanelGroup>
        </div>
        <StatusBar />
      </SidebarInset>

      <GlobalOverlays />
    </SidebarProvider>
  )
}

// Mobile gets the hub-&-spoke shell (no SidebarProvider — that's desktop-only
// chrome). The shell fills the column above the fixed-height status bar; the
// shared dialogs/palette/toaster mount in both layouts. Split out from `App` so
// the store/viewport subscriptions live here and never run on the desktop path.
function MobileApp() {
  const { mobileScreen } = useDux()
  // On the terminal screen the soft keyboard opens, and h-svh does NOT shrink
  // for it — so the bottom of the shell (accessory bar + status bar) would hide
  // behind the keyboard. The visual viewport DOES track the keyboard, so pin the
  // mobile root to it there. Other screens (home/changes) have no focused text
  // input, so the viewport equals h-svh and we keep the default class height.
  const viewportHeight = useVisualViewportHeight()
  const constrainToKeyboard =
    mobileScreen === "terminal" && viewportHeight !== null
  // Drop the bottom safe-area inset only when we're actually pinning the shell
  // above an open keyboard — i.e. the terminal screen (constrainToKeyboard) with
  // the keyboard up. iOS does NOT zero env(safe-area-inset-bottom) when the
  // keyboard is open, so keeping it there would leave a dead strip between the
  // status bar and the keyboard. Everywhere else (keyboard down, or the
  // home/changes screens where opening the palette keyboard doesn't pin the
  // shell) the inset must stay to clear the home indicator. The `&&`
  // short-circuit avoids reading window.innerHeight when there's no viewport.
  const dropBottomInset =
    constrainToKeyboard &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  return (
    // Safe-area padding lives on this single mobile root so EVERY screen
    // (terminal/home/changes) clears the notch, home indicator, and rounded
    // corners — except the fullscreen terminal column, which escapes this root
    // into the fullscreen layer and pads itself (see TerminalPane). Top/side
    // insets always apply; the bottom inset drops only above an open keyboard.
    <div
      className="flex min-h-0 flex-col overflow-hidden"
      style={{
        height:
          constrainToKeyboard && viewportHeight !== null
            ? viewportHeight
            : "100svh",
        paddingTop: "env(safe-area-inset-top)",
        paddingBottom: dropBottomInset ? 0 : "env(safe-area-inset-bottom)",
        paddingLeft: "env(safe-area-inset-left)",
        paddingRight: "env(safe-area-inset-right)",
      }}
    >
      <div className="min-h-0 flex-1">
        <MobileShell />
      </div>
      <StatusBar />
      <GlobalOverlays />
    </div>
  )
}

// While the boot `/api/me` round-trip is in flight, the app shell must NOT mount
// — both because we don't yet know whether to show the login screen, and because
// the WS connect (issued the moment we learn auth is off / we're authed) must
// precede the terminal's first subscribe. A minimal centered spinner covers the
// brief gap.
function BootSpinner() {
  return (
    <div className="flex min-h-svh items-center justify-center bg-background">
      <BrailleSpinner className="text-primary" />
    </div>
  )
}

// Shown when the boot `/api/me` probe network-fails (server down/restarting).
// The store is already auto-retrying with capped backoff; this is the honest
// "we can't reach the server" state — NOT a login screen, which would imply auth
// is the problem (it may be an auth-OFF deployment mid-restart). The duck mark
// keeps it on-brand; the spinner signals the retry is live; "Retry now" lets an
// impatient user skip the current backoff window. A successful retry leaves this
// state on its own (the phase flips to disabled/authed/anonymous).
function UnreachableScreen() {
  return (
    <div className="flex min-h-svh flex-col items-center justify-center gap-4 bg-background p-4 text-center">
      <img src="/dux-logo.png" alt="dux" className="size-12 rounded-lg" />
      <div className="flex items-center gap-2 text-muted-foreground">
        <BrailleSpinner className="text-primary" />
        <span className="text-sm">{UNREACHABLE_MESSAGE}</span>
      </div>
      <Button variant="outline" size="sm" onClick={() => retryBoot()}>
        Retry now
      </Button>
    </div>
  )
}

function App() {
  const { auth } = useDux()
  const isMobile = useIsMobile()

  // Top-level auth branch, BEFORE the shell. "checking" → spinner; "unreachable"
  // → the retrying reconnect screen; "anonymous" → the login screen ONLY (no
  // sidebar, no WS-dependent UI); "disabled"/"authed" → today's app exactly.
  if (auth.phase === "checking") {
    return <BootSpinner />
  }
  if (auth.phase === "unreachable") {
    return <UnreachableScreen />
  }
  if (auth.phase === "anonymous") {
    return <LoginScreen />
  }

  if (isMobile) {
    return <MobileApp />
  }

  return <DesktopShell />
}

export default App
