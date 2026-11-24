import { Spinner } from "@/components/Spinner"
import { Notice } from "@/components/Notice"
import { Modal, useHeldWhileClosed } from "@/components/modal"
import { ScrollArea } from "@/components/ui/scroll-area"
import { EmptyState } from "@/components/ui/empty"
import { FormGroup } from "@/components/ui/form-group"
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
function StartupLogsBody({ open, emptyMessage }: { open: boolean; emptyMessage: string }) {
  const live = useDux()
  // What the viewer showed when the close began, kept while the close animates.
  const startupLogsEntries = useHeldWhileClosed(open, live.startupLogsEntries)
  const startupLogsSelected = useHeldWhileClosed(open, live.startupLogsSelected)
  const startupLogsLoading = useHeldWhileClosed(open, live.startupLogsLoading)
  const startupLogsError = useHeldWhileClosed(open, live.startupLogsError)
  const hasLogs = startupLogsEntries.length > 0

  return startupLogsError ? (
    // The spinner's box too, with the notice across it, so a failed read sits
    // where the spinner and the empty line sit.
    <div className="flex h-64 flex-col justify-center">
      <Notice shape="box" tone="danger">
        <span className="text-destructive">{startupLogsError}</span>
      </Notice>
    </div>
  ) : !hasLogs && startupLogsLoading ? (
    <div className="flex h-64 items-center justify-center">
      <Spinner kind="glyph" size="lg" tone="muted" />
    </div>
  ) : !hasLogs ? (
    // The spinner's box, so the dialog keeps its height when there are no logs.
    <div className="flex h-64 items-center justify-center">
      <EmptyState size="compact" title={emptyMessage} />
    </div>
  ) : (
    <div className="grid gap-3">
      <FormGroup layout="stacked" control="single" spokenName="Log file">
        {(field) => (
          <Select
            value={startupLogsSelected?.name ?? startupLogsEntries[0]?.name ?? ""}
            onValueChange={(name) => name && selectStartupLog(name)}
          >
            <SelectTrigger {...field} className="w-full font-mono">
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
        )}
      </FormGroup>

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
              <Spinner kind="glyph" size="lg" tone="muted" />
            </div>
          ) : null}
        </div>
      </ScrollArea>
    </div>
  )
}

export function StartupLogsDialog() {
  const live = useDux()
  const open = live.startupLogsTarget !== null
  // The scope and target the viewer showed, kept while the close animates.
  const startupLogsScope = useHeldWhileClosed(open, live.startupLogsScope)
  const startupLogsTarget = useHeldWhileClosed(open, live.startupLogsTarget)
  const { spine } = live
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
      open={open}
      vanished={open && !targetExists}
      onClose={closeStartupLogs}
      size="3xl"
      title={renderProse(title)}
      description={description}
    >
      {startupLogsTarget !== null && targetExists ? (
        <StartupLogsBody open={open} emptyMessage={emptyMessage} />
      ) : null}
    </Modal>
  )
}
