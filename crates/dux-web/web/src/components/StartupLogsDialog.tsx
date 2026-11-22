import { GlyphSpinner } from "@/components/GlyphSpinner"
import { Modal } from "@/components/modal"
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
import { renderProse } from "@/lib/prose"
import { DUX_TERMINAL_FONT_STACK } from "@/lib/terminalFont"
import { sessionLabel } from "@/lib/agentWorkspace"

import { startupLogsCopy } from "./startupLogsCopy"

// View startup-command logs. Each run of the project startup command writes a
// timestamped log file; this lists them newest first and shows the selected
// file's contents, fetched into the store when the viewer opens.
function StartupLogsBody({ emptyMessage }: { emptyMessage: string }) {
  const { startupLogsEntries, startupLogsSelected, startupLogsLoading, startupLogsError } =
    useDux()
  const hasLogs = startupLogsEntries.length > 0

  return startupLogsError ? (
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
        <SelectTrigger className="w-full font-mono">
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
  )
}

export function StartupLogsDialog() {
  const { startupLogsScope, startupLogsTarget, spine } = useDux()
  // Looked up by scope: a project id resolved against the sessions finds
  // nothing and would close a valid project view on its first frame.
  const project =
    startupLogsScope === "project"
      ? spine?.projects.find((p) => p.id === startupLogsTarget)
      : undefined
  const session =
    startupLogsScope === "project"
      ? undefined
      : spine?.sessions.find((s) => s.id === startupLogsTarget)
  const targetExists = project !== undefined || session !== undefined
  const { title, description, emptyMessage } = startupLogsCopy(
    startupLogsScope,
    project?.name,
    session ? sessionLabel(session) : undefined,
  )

  return (
    <Modal
      kind="report"
      open={startupLogsTarget !== null}
      vanished={startupLogsTarget !== null && !targetExists}
      onClose={closeStartupLogs}
      size="3xl"
      title={renderProse(title)}
      description={description}
    >
      {startupLogsTarget !== null && targetExists ? (
        <StartupLogsBody emptyMessage={emptyMessage} />
      ) : null}
    </Modal>
  )
}
