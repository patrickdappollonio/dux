import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  closeStartupLogs,
  selectStartupLog,
  useDux,
} from "@/lib/store"

// View an agent's startup-command logs (the web counterpart to the TUI's
// `read-startup-command-logs`). Each run of the project startup command writes a
// timestamped log file; this lists them (newest first) and shows the selected
// file's contents. The list + contents are fetched into the store when the viewer
// opens (see `openStartupLogs`); a Select switches between runs.
function StartupLogsBody({ sessionId }: { sessionId: string }) {
  const {
    spine,
    startupLogsEntries,
    startupLogsSelected,
    startupLogsLoading,
    startupLogsError,
  } = useDux()

  const session = spine?.sessions.find((s) => s.id === sessionId)
  const agentName = session?.title || session?.branch_name || "agent"
  const hasLogs = startupLogsEntries.length > 0

  return (
    <DialogContent showCloseButton={false} className="sm:max-w-3xl">
      <DialogHeader>
        <DialogTitle>Startup command logs — {agentName}</DialogTitle>
        <DialogDescription>
          Output from each run of the project startup command in this agent's
          worktree, newest first.
        </DialogDescription>
      </DialogHeader>

      {startupLogsError ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive">
          {startupLogsError}
        </div>
      ) : !hasLogs && startupLogsLoading ? (
        <div className="flex h-64 items-center justify-center">
          <BrailleSpinner className="text-lg text-muted-foreground" />
        </div>
      ) : !hasLogs ? (
        <div className="flex h-64 items-center justify-center px-6 text-center text-sm text-muted-foreground">
          No startup command logs yet. Run the startup command for this agent to
          generate one.
        </div>
      ) : (
        <div className="grid gap-3">
          <Select
            value={startupLogsSelected?.name ?? startupLogsEntries[0]?.name ?? ""}
            onValueChange={(name) => name && selectStartupLog(name)}
          >
            <SelectTrigger className="w-full font-mono max-md:min-h-11">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {startupLogsEntries.map((entry) => (
                <SelectItem key={entry.name} value={entry.name} className="font-mono">
                  {entry.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <ScrollArea className="h-[50vh] rounded-md border md:h-96">
            {/* The relative positioning anchors the in-flight spinner over the
                content while switching to a different log file. */}
            <div className="relative">
              <pre className="whitespace-pre-wrap break-words p-3 font-mono text-xs leading-relaxed">
                {startupLogsSelected?.content ?? ""}
              </pre>
              {startupLogsLoading ? (
                <div className="absolute inset-0 flex items-center justify-center bg-background/60">
                  <BrailleSpinner className="text-lg text-muted-foreground" />
                </div>
              ) : null}
            </div>
          </ScrollArea>
        </div>
      )}

      <DialogFooter>
        <Button variant="outline" onClick={closeStartupLogs}>
          Close
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function StartupLogsDialog() {
  const { startupLogsTarget } = useDux()

  return (
    <Dialog
      open={startupLogsTarget !== null}
      onOpenChange={(o) => {
        if (!o) closeStartupLogs()
      }}
    >
      {startupLogsTarget !== null && (
        <StartupLogsBody sessionId={startupLogsTarget} />
      )}
    </Dialog>
  )
}
