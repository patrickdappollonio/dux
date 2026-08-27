import { GlyphSpinner } from "@/components/GlyphSpinner"
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
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import type { StartupLogsScope } from "@/lib/store"
import {
  closeStartupLogs,
  selectStartupLog,
  useDux,
} from "@/lib/store"
import { DUX_TERMINAL_FONT_STACK } from "@/lib/terminalFont"
import { sessionLabel } from "@/lib/agentWorkspace"

// View startup-command logs (the web counterpart to the TUI's
// `read-startup-command-logs`). Each run of the project startup command writes a
// timestamped log file; this lists them (newest first) and shows the selected
// file's contents. The list + contents are fetched into the store when the viewer
// opens (see `openStartupLogs` / `openProjectStartupLogs`); a Select switches
// between runs.
//
// ONE dialog serves both scopes of `StartupCommandLogScope`. Only the naming
// differs, and it MUST differ: a user who opens an agent's runs and then a
// project's could not otherwise tell the two lists apart, so the title says
// which entity and the subtitle says how wide the list is.
function StartupLogsBody({
  scope,
  targetId,
}: {
  scope: StartupLogsScope
  targetId: string
}) {
  const {
    spine,
    startupLogsEntries,
    startupLogsSelected,
    startupLogsLoading,
    startupLogsError,
  } = useDux()

  const project = spine?.projects.find((p) => p.id === targetId)
  const session = spine?.sessions.find((s) => s.id === targetId)
  const isProject = scope === "project"
  const title = isProject
    ? `Startup command logs: ${project?.name || "project"} (all agents)`
    : `Startup command logs: ${session ? sessionLabel(session) : "agent"}`
  const description = isProject
    ? "Output from each run of the project startup command across every agent in this project, newest first."
    : "Output from each run of the project startup command in this agent's worktree, newest first."
  const emptyMessage = isProject
    ? "No startup command logs yet. Run the startup command for an agent in this project to generate one."
    : "No startup command logs yet. Run the startup command for this agent to generate one."
  const hasLogs = startupLogsEntries.length > 0

  return (
    <DialogContent showCloseButton={false} className="sm:max-w-3xl">
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>

      {startupLogsError ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive">
          {startupLogsError}
        </div>
      ) : !hasLogs && startupLogsLoading ? (
        <div className="flex h-64 items-center justify-center">
          <GlyphSpinner className="text-lg text-muted-foreground" />
        </div>
      ) : !hasLogs ? (
        <div className="flex h-64 items-center justify-center px-6 text-center text-sm text-muted-foreground">
          {emptyMessage}
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
              {/* The bundled terminal stack (not bare font-mono) so startup-command
                  output gets the same box-drawing/block/braille glyph coverage as
                  the live terminal. */}
              <pre
                className="whitespace-pre-wrap break-words p-3 font-mono text-xs leading-relaxed"
                style={{ fontFamily: DUX_TERMINAL_FONT_STACK }}
              >
                {startupLogsSelected?.content ?? ""}
              </pre>
              {startupLogsLoading ? (
                <div className="absolute inset-0 flex items-center justify-center bg-background/60">
                  <GlyphSpinner className="text-lg text-muted-foreground" />
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
  const { startupLogsScope, startupLogsTarget, spine } = useDux()
  // Closes the dialog when the entity it is scoped to vanishes from the
  // ViewModel: the logs belong to a deleted agent's worktree, or to a removed
  // project, and either way they are gone. The lookup MUST follow the scope,
  // because resolving a project id against the session list would find nothing
  // and slam a perfectly valid project view shut on its first frame. See the
  // hook.
  const targetExists =
    startupLogsScope === "project"
      ? spine?.projects.some((p) => p.id === startupLogsTarget) === true
      : spine?.sessions.some((s) => s.id === startupLogsTarget) === true
  const open = useVanishedTargetGuard(
    startupLogsTarget !== null,
    targetExists,
    closeStartupLogs,
  )

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeStartupLogs()
      }}
    >
      {open && startupLogsTarget !== null && (
        <StartupLogsBody scope={startupLogsScope} targetId={startupLogsTarget} />
      )}
    </Dialog>
  )
}
