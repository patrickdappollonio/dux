import { AppSidebar } from "@/components/Sidebar"
import { ChangedFiles } from "@/components/ChangedFiles"
import { CommandPalette } from "@/components/CommandPalette"
import { CommitDialog } from "@/components/CommitDialog"
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
}

function InsetHeader() {
  const { viewModel, selectedSessionId, conn } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? viewModel?.projects.find((p) => p.id === session.project_id)
    : undefined
  const badge = CONN_BADGE[conn]

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      <SidebarTrigger className="-ml-1" />
      <Separator orientation="vertical" className="h-4" />
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
  const { selectedSessionId } = useDux()

  if (!selectedSessionId) {
    return (
      <Empty className="h-full border-0">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <SquareTerminal />
          </EmptyMedia>
          <EmptyTitle>No session selected</EmptyTitle>
          <EmptyDescription>
            Pick an agent session from the sidebar to attach its terminal.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    )
  }

  return (
    <div className="h-full min-h-0">
      <TerminalPane key={selectedSessionId} sessionId={selectedSessionId} />
    </div>
  )
}

function App() {
  return (
    <SidebarProvider>
      <AppSidebar />
      <SidebarInset className="h-svh min-h-0 overflow-hidden">
        <InsetHeader />
        <div className="flex min-h-0 flex-1 flex-col">
          <ResizablePanelGroup orientation="horizontal" className="flex-1">
            <ResizablePanel defaultSize={74} minSize={30}>
              <TerminalArea />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel defaultSize={26} minSize={14} collapsible>
              <ChangedFiles />
            </ResizablePanel>
          </ResizablePanelGroup>
          <StatusBar />
        </div>
      </SidebarInset>

      <CommandPalette />
      <CommitDialog />
      <Toaster />
    </SidebarProvider>
  )
}

export default App
