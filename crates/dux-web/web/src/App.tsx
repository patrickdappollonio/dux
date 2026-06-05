import type * as React from "react"
import { Suspense } from "react"

import { AddProjectDialog } from "@/components/AddProjectDialog"
import { AttachWorktreeDialog } from "@/components/AttachWorktreeDialog"
import { AppSidebar } from "@/components/Sidebar"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { CommandPalette } from "@/components/CommandPalette"
import { ChangeProviderDialog } from "@/components/ChangeProviderDialog"
import { CommitDialog } from "@/components/CommitDialog"
import { ConfirmDeleteTerminalDialog } from "@/components/ConfirmDeleteTerminalDialog"
import { ConfirmDiscardFileDialog } from "@/components/ConfirmDiscardFileDialog"
import { CreateAgentDialog } from "@/components/CreateAgentDialog"
import { RenameSessionDialog } from "@/components/RenameSessionDialog"
import { CheckoutDefaultBranchDialog } from "@/components/CheckoutDefaultBranchDialog"
import { DeleteSessionDialog } from "@/components/DeleteSessionDialog"
import { GlobalEnvDialog } from "@/components/GlobalEnvDialog"
import { MobileShell } from "@/components/MobileShell"
import { ProjectInfoDialog } from "@/components/ProjectInfoDialog"
import { ProjectSettingsDialog } from "@/components/ProjectSettingsDialog"
import { RemoveProjectDialog } from "@/components/RemoveProjectDialog"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { StatusBar } from "@/components/StatusBar"
import { Welcome } from "@/components/Welcome"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb"
import { Button } from "@/components/ui/button"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { Separator } from "@/components/ui/separator"
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { Toaster } from "@/components/ui/sonner"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { setPaletteOpen, useDux } from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"

function InsetHeader() {
  const { viewModel, selectedSessionId, selectedTarget } = useDux()
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

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      <SidebarTrigger className="-ml-1" />
      <Separator orientation="vertical" className="mx-1 h-4" />
      <Breadcrumb>
        <BreadcrumbList>
          {session ? (
            <>
              <BreadcrumbItem>
                <BreadcrumbPage>{project?.name ?? "dux"}</BreadcrumbPage>
              </BreadcrumbItem>
              <BreadcrumbSeparator />
              <BreadcrumbItem>
                <BreadcrumbPage className="font-mono">
                  {session.branch_name}
                </BreadcrumbPage>
              </BreadcrumbItem>
              {terminalLabel ? (
                <>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <BreadcrumbPage>{terminalLabel}</BreadcrumbPage>
                  </BreadcrumbItem>
                </>
              ) : null}
            </>
          ) : (
            <BreadcrumbItem>
              <BreadcrumbPage>dux</BreadcrumbPage>
            </BreadcrumbItem>
          )}
        </BreadcrumbList>
      </Breadcrumb>

      <div className="ml-auto flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => setPaletteOpen(true)}
        >
          <span className="font-mono text-xs">⌘K</span>
          Search…
        </Button>
      </div>
    </header>
  )
}

function TerminalArea() {
  const { selectedTarget, terminalEpoch } = useDux()

  // Idle center pane: the duck + logo + a tip, exactly like the TUI's welcome
  // screen. It vanishes the moment a target is selected (the loading state is
  // the terminal pane's readiness spinner, not this).
  if (!selectedTarget) {
    return <Welcome />
  }

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
    <div className="h-full min-h-0 overflow-hidden">
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
  )
}

// The palette, all dialogs, and the toaster live above the shell so they stay
// mounted in both the desktop and mobile layouts. Shared JSX — never duplicated.
function GlobalOverlays() {
  return (
    <>
      <CommandPalette />
      <CommitDialog />
      <CreateAgentDialog />
      <RenameSessionDialog />
      <ChangeProviderDialog />
      <DeleteSessionDialog />
      <ConfirmDeleteTerminalDialog />
      <ConfirmDiscardFileDialog />
      <GlobalEnvDialog />
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
  const { sidebarWidth } = useDux()

  return (
    <SidebarProvider
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
    >
      <AppSidebar />
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        <InsetHeader />
        <div className="min-h-0 flex-1">
          <ResizablePanelGroup orientation="horizontal" className="size-full">
            <ResizablePanel defaultSize={74} minSize={30}>
              <TerminalArea />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel defaultSize={26} minSize={14} collapsible>
              <ChangedFiles />
            </ResizablePanel>
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

  return (
    <div
      className="flex min-h-0 flex-col overflow-hidden"
      style={
        constrainToKeyboard ? { height: viewportHeight } : { height: "100svh" }
      }
    >
      <div className="min-h-0 flex-1">
        <MobileShell />
      </div>
      <StatusBar />
      <GlobalOverlays />
    </div>
  )
}

function App() {
  const isMobile = useIsMobile()

  if (isMobile) {
    return <MobileApp />
  }

  return <DesktopShell />
}

export default App
