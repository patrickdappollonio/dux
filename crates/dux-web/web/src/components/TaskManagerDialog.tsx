import { ChevronDown, ChevronRight } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useIsMobile } from "@/hooks/use-mobile"
import { formatBytes, formatCpu } from "@/lib/formatStats"
import { RESOURCE_POLL_INTERVAL_MS, nextPollDelay, shouldPoll } from "@/lib/resourcePoll"
import { nothingRunning, taskManagerRows, type TaskRow } from "@/lib/resourceRows"
import { resourcesApi, type ResourceStatsView } from "@/lib/resourcesApi"
import {
  closeStopAll,
  closeTaskManager,
  openCloseTab,
  openDeleteTerminal,
  openStopAll,
  stopAllRunning,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"

// The Task Manager (the app menu's "Task Manager…"): what is running, what it
// costs, and how to stop it. It merges two things that used to be separate: the
// web's kill-running modal and the resource monitor the web never had.
//
// Rows are PER TAB, not per agent: stats are sampled per provider process, and
// the engine keys those by tab id. A three-tab agent is three rows, grouped
// under its session-slot tab. (The modal this replaces listed one row per
// session and could not tell you which tab was burning the CPU.)
//
// EVERY stop confirms, including "Stop all…". The modal this replaces killed on
// a single click, justified by being "the deliberate, clearly-labelled
// destructive surface". That justification lapses here: a Task Manager is a
// surface you leave open and read numbers off, so a misclick must not end a
// process. Both confirmations are existing, established dialogs, so the hung-
// agent escape hatch stays one keystroke away.
//
// Stats arrive by REST poll (only while open, paused when the tab is hidden),
// not over the event bus: an event names what changed and never carries the
// changed value, and stats are a value that changes every sample.
export function TaskManagerDialog() {
  const { taskManagerOpen, stopAllOpen } = useDux()

  function handleOpenChange(open: boolean) {
    if (!open) closeTaskManager()
  }

  return (
    <>
      <Dialog open={taskManagerOpen} onOpenChange={handleOpenChange}>
        {/* No `destructive` on the dialog itself: the grayscale backdrop suits a
            kill modal but fights a monitor you read numbers off. The nested
            confirmations keep it. */}
        <DialogContent
          showCloseButton={false}
          className="sm:max-w-[min(44rem,calc(100%-2rem))]"
        >
          {/* The body mounts only while open, so its sample and expansion state
              reset by unmounting rather than by being cleared, and the poll loop
              is torn down by the effect's own cleanup. */}
          {taskManagerOpen ? <TaskManagerBody /> : null}
        </DialogContent>
      </Dialog>

      <ConfirmStopAllDialog open={stopAllOpen} />
    </>
  )
}

function TaskManagerBody() {
  const { spine } = useDux()
  // ONE layout renders at a time (not two hidden behind CSS): the 4-column table
  // cannot fit a phone, so mobile gets stacked cards instead. Rendering both and
  // hiding one would duplicate every row, and every Stop control, in the DOM.
  const isMobile = useIsMobile()
  const [stats, setStats] = useState<ResourceStatsView[]>([])
  const [expanded, setExpanded] = useState<Set<string>>(new Set())

  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  const rows = useMemo(() => taskManagerRows(sessions, stats), [sessions, stats])
  const empty = nothingRunning(rows)

  // Poll while visible. Wall-clock: each tick schedules the next from how long
  // the fetch actually took, so a slow round-trip does not stretch the cadence.
  // A closed dialog polls nothing (this body is unmounted), which is the whole
  // point of serving stats over REST rather than pushing them to every client.
  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const controller = new AbortController()

    async function tick() {
      if (cancelled) return
      if (!shouldPoll({ open: true, hidden: document.hidden })) {
        // Hidden: idle until `visibilitychange` restarts the loop.
        return
      }
      const startedAt = Date.now()
      try {
        const resp = await resourcesApi.get(controller.signal)
        if (!cancelled) setStats(resp.rows)
      } catch {
        // A failed or aborted sample is not worth a toast: the next tick (one
        // second later) either recovers or the user closes the dialog. The
        // rows keep rendering from the spine with dashes meanwhile.
      }
      if (cancelled) return
      timer = setTimeout(
        tick,
        nextPollDelay(RESOURCE_POLL_INTERVAL_MS, Date.now() - startedAt),
      )
    }

    function onVisibility() {
      if (document.hidden) {
        if (timer !== undefined) clearTimeout(timer)
        return
      }
      // Back in the foreground: sample immediately rather than waiting out the
      // interval, so the numbers are never stale on return.
      if (timer !== undefined) clearTimeout(timer)
      void tick()
    }

    document.addEventListener("visibilitychange", onVisibility)
    void tick()

    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
      controller.abort()
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [])

  // Auto-close ONLY when the list goes from populated to empty while open (the
  // user stopped the last runtime), never on an open that starts empty, which
  // would flash the dialog shut before the "Nothing is running." state is read.
  const wasPopulated = useRef(false)
  useEffect(() => {
    if (!empty) {
      wasPopulated.current = true
      return
    }
    if (wasPopulated.current) closeTaskManager()
  }, [empty])

  const toggleExpanded = useCallback((key: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }, [])

  function handleStop(row: TaskRow) {
    if (row.sessionId === null || row.targetId === null) return
    // Both paths open an EXISTING confirmation dialog rather than acting now.
    if (row.kind === "terminal") {
      openDeleteTerminal(row.targetId)
      return
    }
    openCloseTab(row.sessionId, row.targetId)
  }

  return (
    <>
          <DialogHeader>
            <DialogTitle>Task Manager</DialogTitle>
            <DialogDescription>
              What&apos;s running, what it costs, and how to stop it. Agents
              detach and can be reconnected; terminals are destroyed.
            </DialogDescription>
          </DialogHeader>

          {/* The table scrolls HORIZONTALLY before any name is ellipsized: child
              rows are command names ("node", "rg", "nvim") and a name truncated
              to nothing tells the user less than a scrollbar does. */}
          <div className="max-h-96 overflow-y-auto">
            <div className="overflow-x-auto">
              {/* Desktop: the 4-column table. */}
              {isMobile ? null : (
              <table className="w-full border-collapse text-sm">
                <thead>
                  <tr className="text-xs text-muted-foreground">
                    <th className="py-1 pr-2 text-left font-medium">Name</th>
                    <th className="py-1 pr-2 text-right font-medium">CPU</th>
                    <th className="py-1 pr-2 text-right font-medium">Memory</th>
                    <th className="py-1 pr-2 text-right font-medium">Procs</th>
                    <th className="py-1" />
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <DesktopRow
                      key={row.key}
                      row={row}
                      expanded={expanded.has(row.key)}
                      onToggle={() => toggleExpanded(row.key)}
                      onStop={() => handleStop(row)}
                    />
                  ))}
                </tbody>
              </table>
              )}

              {/* Mobile: stacked cards carrying the same rows. */}
              {isMobile ? (
                <div className="flex flex-col gap-1">
                  {rows.map((row) => (
                    <MobileRow
                      key={row.key}
                      row={row}
                      expanded={expanded.has(row.key)}
                      onToggle={() => toggleExpanded(row.key)}
                      onStop={() => handleStop(row)}
                    />
                  ))}
                </div>
              ) : null}
            </div>

            {empty ? (
              <p className="px-2 py-6 text-center text-sm text-muted-foreground">
                Nothing is running.
              </p>
            ) : null}
          </div>

          {/* Misclick-safe spacing between the list and the footer buttons. */}
          <div className="h-2" />
          <DialogFooter>
            {empty ? null : (
              <Button variant="outline" onClick={openStopAll}>
                Stop all…
              </Button>
            )}
            <Button variant="outline" autoFocus onClick={closeTaskManager}>
              Done
            </Button>
          </DialogFooter>
    </>
  )
}

// A dash, not a disabled button: dux and TOTAL have nothing to stop, and a
// disabled control would imply an action that does not exist.
function NoStop() {
  return (
    <span aria-hidden className="text-muted-foreground/50">
      —
    </span>
  )
}

// The expand caret. Every kind expands, terminals included: the collector runs
// the same tree walk over every target.
function ExpandToggle({
  row,
  expanded,
  onToggle,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
}) {
  const children = row.stats?.children ?? []
  if (children.length === 0) return <span className="inline-block w-4" />
  const Icon = expanded ? ChevronDown : ChevronRight
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={expanded}
      aria-label={
        expanded
          ? `Hide ${row.name} child processes`
          : `Show ${row.name} child processes`
      }
      className="inline-flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground"
    >
      <Icon className="size-3.5" />
    </button>
  )
}

// Stat cells read as dashes when this row had no sample: a dormant tab, or a
// process born since the last poll. The row still renders and stays stoppable.
function statCells(stats: ResourceStatsView | null) {
  return {
    cpu: stats ? formatCpu(stats.cpu_percent) : "—",
    mem: stats ? formatBytes(stats.rss_bytes) : "—",
    procs: stats ? String(stats.process_count) : "—",
  }
}

function DesktopRow({
  row,
  expanded,
  onToggle,
  onStop,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
  onStop: () => void
}) {
  const isTotal = row.kind === "total"
  const { cpu, mem, procs } = statCells(row.stats)
  const children = row.stats?.children ?? []

  return (
    <>
      <tr
        data-testid={`task-row-${row.key}`}
        className={cn(
          "hover:bg-muted/50",
          isTotal && "border-t border-border font-medium",
        )}
      >
        <td className="py-1.5 pr-2">
          <div
            className={cn("flex items-center gap-1.5", row.nested && "pl-5")}
          >
            {isTotal ? (
              <span className="inline-block w-4" />
            ) : (
              <ExpandToggle row={row} expanded={expanded} onToggle={onToggle} />
            )}
            {/* `whitespace-nowrap`, never truncate: the container scrolls. */}
            <span className="whitespace-nowrap">{row.name}</span>
            {row.detail ? (
              <span className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                {row.detail}
              </span>
            ) : null}
          </div>
        </td>
        {/* `tabular-nums` so digits do not jitter as the numbers change. */}
        <td className="py-1.5 pr-2 text-right tabular-nums">{cpu}</td>
        <td className="py-1.5 pr-2 text-right tabular-nums">{mem}</td>
        <td className="py-1.5 pr-2 text-right tabular-nums">{procs}</td>
        <td className="py-1.5 text-right">
          {row.stoppable ? (
            <Button
              size="sm"
              variant="outline"
              onClick={onStop}
              aria-label={`Stop ${row.name}`}
            >
              Stop
            </Button>
          ) : (
            <NoStop />
          )}
        </td>
      </tr>
      {expanded
        ? children.map((child) => (
            <tr
              key={`${row.key}-${child.pid}`}
              className="text-xs text-muted-foreground"
            >
              <td className="py-0.5 pr-2">
                <div className={cn("pl-10", row.nested && "pl-15")}>
                  <span className="whitespace-nowrap font-mono">
                    {child.name}
                  </span>
                </div>
              </td>
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {formatCpu(child.cpu_percent)}
              </td>
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {formatBytes(child.rss_bytes)}
              </td>
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {child.pid}
              </td>
              <td />
            </tr>
          ))
        : null}
    </>
  )
}

function MobileRow({
  row,
  expanded,
  onToggle,
  onStop,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
  onStop: () => void
}) {
  const isTotal = row.kind === "total"
  const { cpu, mem, procs } = statCells(row.stats)
  const children = row.stats?.children ?? []

  return (
    <div
      className={cn(
        "rounded-md px-2 py-1.5",
        isTotal && "border-t border-border font-medium",
        row.nested && "ml-4",
      )}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-1.5">
          {isTotal ? null : (
            <ExpandToggle row={row} expanded={expanded} onToggle={onToggle} />
          )}
          <div className="min-w-0">
            {/* Truncate here (a phone genuinely cannot fit a long branch name),
                but the tooltip keeps the full name reachable. */}
            <SimpleTooltip content={row.name}>
              <div className="truncate text-sm">{row.name}</div>
            </SimpleTooltip>
            <div className="truncate text-xs text-muted-foreground tabular-nums">
              {cpu} · {mem} · {procs} procs
            </div>
          </div>
        </div>
        {row.stoppable ? (
          <Button
            size="sm"
            variant="outline"
            onClick={onStop}
            aria-label={`Stop ${row.name}`}
            className="max-md:min-h-10 shrink-0"
          >
            Stop
          </Button>
        ) : null}
      </div>
      {expanded ? (
        <ul className="mt-1 flex flex-col gap-0.5 pl-6">
          {children.map((child) => (
            <li
              key={child.pid}
              className="flex items-center justify-between gap-2 text-xs text-muted-foreground"
            >
              <span className="truncate font-mono">{child.name}</span>
              <span className="shrink-0 tabular-nums">
                {formatCpu(child.cpu_percent)} · {formatBytes(child.rss_bytes)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

// The bulk stop's confirmation. Nested inside the Task Manager, and destructive-
// styled (unlike the Task Manager itself): this one really is only a dangerous
// action. Follows the established pattern: Cancel `autoFocus`, the confirm
// `variant="destructive"`, misclick-safe spacing.
function ConfirmStopAllDialog({ open }: { open: boolean }) {
  const { spine } = useDux()
  const sessions = spine?.sessions ?? []
  const agents = sessions.filter((s) => s.status === "active").length
  const terminals = sessions.reduce((n, s) => n + s.terminals.length, 0)

  function handleConfirm() {
    stopAllRunning()
    closeStopAll()
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeStopAll()
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Stop everything?</DialogTitle>
          <DialogDescription>
            {`This stops ${countLabel(agents, "agent")} and ${countLabel(terminals, "terminal")}. `}
            Agents detach and stay in Projects, reopenable; terminals are
            destroyed and cannot be recovered.
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeStopAll}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Stop all
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function countLabel(n: number, noun: string): string {
  return n === 1 ? `1 ${noun}` : `${n} ${noun}s`
}
