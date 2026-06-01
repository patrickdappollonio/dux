import { CommandPalette } from "@/components/CommandPalette"
import { CommitDialog } from "@/components/CommitDialog"
import { Sidebar } from "@/components/Sidebar"
import { StatusBar } from "@/components/StatusBar"
import { TerminalPane } from "@/components/TerminalPane"
import { TopBar } from "@/components/TopBar"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { useDux } from "@/lib/store"

function TerminalArea() {
  const { selectedSessionId } = useDux()

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {selectedSessionId ? (
        <TerminalPane key={selectedSessionId} sessionId={selectedSessionId} />
      ) : (
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          <span>Select a session to attach its terminal</span>
        </div>
      )}
    </div>
  )
}

function ChangedFilesArea() {
  return (
    <div className="flex h-full flex-col bg-background p-2 text-xs text-muted-foreground">
      <h2 className="px-1 py-1 text-[0.7rem] font-semibold tracking-wide uppercase">
        Changes
      </h2>
    </div>
  )
}

function App() {
  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <CommandPalette />
      <CommitDialog />
      <TopBar />
      <ResizablePanelGroup orientation="horizontal" className="flex-1">
        <ResizablePanel defaultSize={22} minSize={12} collapsible>
          <Sidebar />
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={54} minSize={30}>
          <TerminalArea />
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={24} minSize={14} collapsible>
          <ChangedFilesArea />
        </ResizablePanel>
      </ResizablePanelGroup>
      <StatusBar />
    </div>
  )
}

export default App
