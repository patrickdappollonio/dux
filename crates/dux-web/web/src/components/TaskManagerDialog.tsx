import {
  Activity,
  Bot,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleStop,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Badge } from "@/components/ui/badge"
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
import { formatRegularCount } from "@/lib/formatRegularCount"
import { formatBytes, formatCpu } from "@/lib/formatStats"
import {
  RESOURCE_POLL_INTERVAL_MS,
  nextPollDelay,
  pollIntervalLabel,
  shouldPoll,
  statsAreStale,
} from "@/lib/resourcePoll"
import {
  nothingRunning,
  taskManagerRows,
  taskManagerSummary,
  type TaskRow,
  type TaskRowKind,
} from "@/lib/resourceRows"
import { resourcesApi, type ResourceStatsView } from "@/lib/resourcesApi"
import {
  closeStopAll,
  closeTaskManager,
  openCloseTab,
  openDeleteTerminal,
  openForceStopAgent,
  openStopAll,
  stopAllRunning,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"

// The Task Manager: what is running, what it costs, and how to stop it. Rows are
// PER TAB, since stats are sampled per provider process; every stop confirms,
// because this is a surface you leave open and read numbers off. Stats arrive by
// REST poll, because an event names what changed and never carries the value.
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
  // ONE layout renders at a time, not two hidden behind CSS: rendering both would
  // duplicate every row, and every Stop control, in the DOM.
  const isMobile = useIsMobile()
  const [stats, setStats] = useState<ResourceStatsView[]>([])
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  // When the last poll succeeded, or `null` before the first sample. A failing
  // poll must not go on rendering the last good numbers as if they were live.
  const [lastSuccessAt, setLastSuccessAt] = useState<number | null>(null)
  // The time as of the last poll attempt, or `null` before any has run. State
  // rather than `Date.now()` at render time, so a failing poll still re-renders.
  const [now, setNow] = useState<number | null>(null)

  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  const projects = useMemo(() => spine?.projects ?? [], [spine])
  const terminals = useMemo(() => spine?.terminals ?? [], [spine])
  const rows = useMemo(
    () => taskManagerRows(sessions, stats, projects, terminals),
    [sessions, stats, projects, terminals],
  )
  const empty = nothingRunning(rows)
  const stale = now !== null && statsAreStale(now, lastSuccessAt)
  const summary = useMemo(() => taskManagerSummary(rows), [rows])

  // Poll while visible. Each tick schedules the next from how long the fetch took,
  // so a slow round-trip does not stretch the cadence; a closed dialog polls nothing.
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
        if (!cancelled) {
          setStats(resp.rows)
          setLastSuccessAt(Date.now())
        }
      } catch {
        // A failed sample raises no toast: the next tick recovers, and a run of
        // failures surfaces through the staleness indicator instead.
      }
      if (!cancelled) setNow(Date.now())
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

  // Auto-close only on the populated-to-empty transition, never on an open that
  // starts empty, which would flash the dialog shut before it can be read.
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
    if (row.targetId === null) return
    // Every path opens a confirmation rather than acting now. Guard per KIND: a
    // project terminal's `sessionId` is null, so a null check here kills its Stop.
    if (row.kind === "terminal") {
      openDeleteTerminal(row.targetId)
      return
    }
    if (row.sessionId === null) return
    // Two acts wear the same Force stop control: a first tab FORCE-STOPS the
    // agent, an extra tab is CLOSED. Both are immediate, which is what the one
    // label promises. Slot-ness comes from `nested`, resolved when the row was
    // built.
    if (!row.nested) {
      openForceStopAgent(row.sessionId)
      return
    }
    openCloseTab(row.sessionId, row.targetId)
  }

  return (
    <>
          <DialogHeader>
            <div className="flex items-center justify-between gap-2">
              <DialogTitle>Task Manager</DialogTitle>
              {/* Coexists with the stalled indicator below, never both at once:
                  this pill claims the numbers are live, the stalled message
                  says they are not, and showing both would contradict itself. */}
              {stale ? null : <LivePollPill intervalMs={RESOURCE_POLL_INTERVAL_MS} />}
            </div>
            <DialogDescription>
              What&apos;s running, what it costs, and how to stop it. Agents
              detach and can be reconnected; terminals are destroyed.
            </DialogDescription>
          </DialogHeader>

          {/* A permanently failing poll must not go on rendering the last good
              numbers as though they were live: this surfaces once a run of
              failures crosses the staleness threshold, and clears the moment
              a poll succeeds again. Subtle by design, no toast: this is a
              persistent state of the numbers, not a one-off event. */}
          {stale ? (
            <p className="flex items-center gap-1.5 px-2 text-xs text-muted-foreground">
              <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
              Stats stalled: showing the last successful sample, not live
              numbers.
            </p>
          ) : null}

          {/* The table scrolls HORIZONTALLY before any name is ellipsized: child
              rows are command names ("node", "rg", "nvim") and a name truncated
              to nothing tells the user less than a scrollbar does. */}
          <div className="max-h-96 overflow-y-auto">
            <div className="overflow-x-auto">
              {/* Desktop: the 4-column table. */}
              {isMobile ? null : (
              <table className="w-full border-collapse text-sm">
                <thead>
                  <tr className="text-[10px] tracking-wide text-muted-foreground uppercase">
                    <th className="py-1 pr-2 text-left font-medium">Name</th>
                    <th className="py-1 pr-2 text-right font-medium">PID</th>
                    <th className="py-1 pr-2 text-right font-medium">Procs</th>
                    <th className="py-1 pr-2 text-right font-medium">CPU</th>
                    <th className="py-1 pr-2 text-right font-medium">Memory</th>
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
            {summary ? (
              <p className="self-center text-xs text-muted-foreground tabular-nums sm:mr-auto">
                {summary}
              </p>
            ) : null}
            {empty ? null : (
              // Destructive here, unlike a `⋯` menu's neutral items: a dialog footer
              // button is the surface `variant="destructive"` is reserved for.
              <Button variant="destructive" onClick={openStopAll}>
                Force stop everything…
              </Button>
            )}
            <Button variant="outline" autoFocus onClick={closeTaskManager}>
              Done
            </Button>
          </DialogFooter>
    </>
  )
}

// The header's "still live" cue. The interval is read from the poll constant so
// the copy cannot drift, and the blink reuses `.agent-status-dot`'s one definition.
function LivePollPill({ intervalMs }: { intervalMs: number }) {
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
      <Circle
        aria-hidden
        className="size-2 shrink-0 fill-current agent-status-dot agent-status-dot--on"
      />
      Updating {pollIntervalLabel(intervalMs)}
    </span>
  )
}

// The leading row icon reuses the icons the rest of the app already uses per kind;
// `dux` takes the app menu's own Task Manager icon, and TOTAL is not a process.
const ROW_ICONS: Partial<Record<TaskRowKind, typeof Bot>> = {
  dux: Activity,
  agent: Bot,
  terminal: SquareTerminal,
}

function RowIcon({ kind }: { kind: TaskRowKind }) {
  const Icon = ROW_ICONS[kind]
  if (!Icon) return null
  return <Icon aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
}

// Marks the dux row as "this process". Cyan but deliberately STATIC: the blink
// paired with that color everywhere else means "needs attention", which this never is.
function DuxBadge() {
  return (
    <Badge
      variant="outline"
      className="border-cyan-100/30 bg-cyan-100/10 text-cyan-100"
    >
      this process
    </Badge>
  )
}

// A dash, not a disabled button: dux and TOTAL have nothing to stop, and a
// disabled control would imply an action that does not exist.
function NoStop() {
  return (
    <span aria-hidden className="text-muted-foreground/50">
      -
    </span>
  )
}

// The gate is core's `has_breakdown`, never `children.length`: `children` always
// includes the row's own root, so a leaf has length 1 and would look expandable.
function rowHasBreakdown(row: TaskRow): boolean {
  return row.stats?.has_breakdown ?? false
}

function ExpandToggle({
  row,
  expanded,
  onToggle,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
}) {
  if (!rowHasBreakdown(row)) return <span className="inline-block w-4" />
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
      // 40px touch target on phones: this chevron sits beside Stop, the one control
      // a misclick would be worst on. Desktop keeps the compact density.
      className="inline-flex size-4 max-md:size-10 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground"
    >
      <Icon className="size-3.5" />
    </button>
  )
}

// Stat cells read as dashes when the row had no sample; the row stays stoppable.
// `pid` is the CALLER's: TOTAL is blank rather than dashed, a different nothing.
function statCells(stats: ResourceStatsView | null) {
  return {
    cpu: stats ? formatCpu(stats.cpu_percent) : "-",
    mem: stats ? formatBytes(stats.rss_bytes) : "-",
    procs: stats ? String(stats.process_count) : "-",
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
  // TOTAL has no pid at all (blank: it is a summary, not a process); every
  // other row shows the real pid, or a dash before the first sample lands.
  const pid = isTotal ? "" : row.stats?.pid != null ? String(row.stats.pid) : "-"
  // The body is gated on the same rule as the toggle: the expansion set outlives
  // the tree's shape, so a row that loses its children must stop rendering them.
  const children = rowHasBreakdown(row) ? (row.stats?.children ?? []) : []

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
            <RowIcon kind={row.kind} />
            {/* `whitespace-nowrap`, never truncate: the container scrolls. */}
            <span className="whitespace-nowrap">{row.name}</span>
            {row.kind === "dux" ? <DuxBadge /> : null}
            {row.detail ? (
              <span className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                {row.detail}
              </span>
            ) : null}
          </div>
        </td>
        {/* `tabular-nums` so digits do not jitter as the numbers change. */}
        <td className="py-1.5 pr-2 text-right tabular-nums">{pid}</td>
        <td className="py-1.5 pr-2 text-right tabular-nums">{procs}</td>
        <td className="py-1.5 pr-2 text-right tabular-nums">{cpu}</td>
        <td className="py-1.5 pr-2 text-right tabular-nums">{mem}</td>
        <td className="py-1.5 text-right">
          {row.stoppable ? (
            <Button
              size="sm"
              variant="outline"
              onClick={onStop}
              aria-label={row.stopLabel}
            >
              <CircleStop aria-hidden />
              Force stop
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
              data-testid={`child-row-${child.pid}`}
              className="text-xs text-muted-foreground"
            >
              <td className="py-0.5 pr-2">
                <div className={cn("pl-10", row.nested && "pl-15")}>
                  <span className="whitespace-nowrap font-mono">
                    {child.name}
                  </span>
                  {/* The root is in its own breakdown so the entries sum to the
                      row's total. Label it, or it reads as a phantom duplicate
                      of the row above. */}
                  {child.is_root ? (
                    <span className="ml-1.5 whitespace-nowrap text-muted-foreground/70">
                      (this process)
                    </span>
                  ) : null}
                </div>
              </td>
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {child.pid}
              </td>
              {/* A child is one process, so its Procs cell is intentionally empty. */}
              <td className="py-0.5 pr-2 text-right tabular-nums" />
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {formatCpu(child.cpu_percent)}
              </td>
              <td className="py-0.5 pr-2 text-right tabular-nums">
                {formatBytes(child.rss_bytes)}
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
  // Same body gate as the desktop row: see `DesktopRow`.
  const children = rowHasBreakdown(row) ? (row.stats?.children ?? []) : []

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
          <RowIcon kind={row.kind} />
          <div className="min-w-0">
            {/* Truncate here (a phone genuinely cannot fit a long branch name),
                but the tooltip keeps the full name reachable. */}
            <SimpleTooltip content={row.name}>
              <div className="flex min-w-0 items-center gap-1.5 truncate text-sm">
                <span className="truncate">{row.name}</span>
                {row.kind === "dux" ? <DuxBadge /> : null}
              </div>
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
            aria-label={row.stopLabel}
            className="max-md:min-h-10 shrink-0"
          >
            <CircleStop aria-hidden />
            Force stop
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
              <span className="truncate font-mono">
                {child.name}
                {/* Same root marker as desktop: the breakdown includes the row's
                    own process so the numbers add up, and it must not read as a
                    phantom duplicate. */}
                {child.is_root ? (
                  <span className="ml-1.5 text-muted-foreground/70">
                    (this process)
                  </span>
                ) : null}
              </span>
              {/* Cards label the PID inline because they have no PID column. */}
              <span className="shrink-0 tabular-nums">
                pid {child.pid} · {formatCpu(child.cpu_percent)} ·{" "}
                {formatBytes(child.rss_bytes)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

// The bulk stop's confirmation, nested inside the Task Manager and destructive-
// styled, unlike the Task Manager itself.
function ConfirmStopAllDialog({ open }: { open: boolean }) {
  const { spine } = useDux()
  const sessions = spine?.sessions ?? []
  const agents = sessions.filter((s) => s.status === "active").length
  // Every terminal of every owner, which is exactly what `stopAllRunning` will
  // stop: one flat collection, so the count cannot miss an owner kind.
  const terminals = spine?.terminals.length ?? 0

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
          <DialogTitle>Force stop everything?</DialogTitle>
          <DialogDescription>
            {`This stops ${formatRegularCount(agents, "agent")} and ${formatRegularCount(terminals, "terminal")} immediately, with no shutdown wait. `}
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
            Force stop everything
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

