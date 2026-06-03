import type * as React from "react"

import { AppSidebar } from "@/components/Sidebar"
import { ChangedFiles } from "@/components/ChangedFiles"
import { CommandPalette } from "@/components/CommandPalette"
import { CommitDialog } from "@/components/CommitDialog"
import { DeleteSessionDialog } from "@/components/DeleteSessionDialog"
import { GlobalEnvDialog } from "@/components/GlobalEnvDialog"
import { StatusBar } from "@/components/StatusBar"
import { TerminalPane } from "@/components/TerminalPane"
import { Badge } from "@/components/ui/badge"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb"
import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
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
import { setPaletteOpen, useDux } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { SquareTerminal } from "lucide-react"

const CONN_BADGE: Record<
  ConnState,
  { variant: "default" | "secondary" | "outline"; label: string }
> = {
  open: { variant: "default", label: "Connected" },
  connecting: { variant: "secondary", label: "Connecting" },
  closed: { variant: "outline", label: "Offline" },
  failed: { variant: "outline", label: "Disconnected" },
}

function InsetHeader() {
  const { viewModel, selectedSessionId, selectedTarget, conn } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? viewModel?.projects.find((p) => p.id === session.project_id)
    : undefined
  // When a companion terminal is focused, surface its label as a third crumb.
  const terminalLabel =
    selectedTarget?.kind === "terminal"
      ? session?.terminals.find((t) => t.id === selectedTarget.terminalId)
          ?.label
      : undefined
  const badge = CONN_BADGE[conn]

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
        <Badge variant={badge.variant}>{badge.label}</Badge>
      </div>
    </header>
  )
}

function TerminalArea() {
  const { selectedTarget } = useDux()

  if (!selectedTarget) {
    return (
      <Empty className="h-full border-0">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <SquareTerminal />
          </EmptyMedia>
          <EmptyTitle>Nothing selected</EmptyTitle>
          <EmptyDescription>
            Pick an agent session or a companion terminal from the sidebar to
            attach it.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    )
  }

  // For an agent the streamed id is the session id; for a terminal it is the
  // terminal id. Key by that id so switching remounts the terminal cleanly.
  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.sessionId

  // TerminalPane owns its own background and padding (via inline style) so the
  // padding area is seamlessly part of the terminal surface.
  return (
    <div className="h-full min-h-0">
      <TerminalPane
        key={targetId}
        kind={selectedTarget.kind}
        id={targetId}
      />
    </div>
  )
}

function App() {
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

      <CommandPalette />
      <CommitDialog />
      <DeleteSessionDialog />
      <GlobalEnvDialog />
      <Toaster />
    </SidebarProvider>
  )
}

export default App
