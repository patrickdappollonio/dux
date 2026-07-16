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
  // When the last poll actually succeeded, or `null` before the first sample
  // ever lands. Drives the staleness indicator below: a permanently failing
  // poll must not go on rendering the last good numbers as if they were live.
  const [lastSuccessAt, setLastSuccessAt] = useState<number | null>(null)
  // The time as of the last poll attempt (success or failure), or `null`
  // before any attempt has run. Rendering must stay pure (no `Date.now()` at
  // render time), so "now" is captured in an effect and stored as state; this
  // is also what forces a re-render to re-evaluate `statsAreStale` while
  // every fetch is failing (a failure alone touches no other state).
  const [now, setNow] = useState<number | null>(null)

  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  const projects = useMemo(() => spine?.projects ?? [], [spine])
  const rows = useMemo(
    () => taskManagerRows(sessions, stats, projects),
    [sessions, stats, projects],
  )
  const empty = nothingRunning(rows)
  const stale = now !== null && statsAreStale(now, lastSuccessAt)
  const summary = useMemo(() => taskManagerSummary(rows), [rows])

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
        if (!cancelled) {
          setStats(resp.rows)
          setLastSuccessAt(Date.now())
        }
      } catch {
        // A failed or aborted sample is not worth a toast: the next tick (one
        // second later) either recovers or the user closes the dialog. The
        // rows keep rendering from the spine with dashes meanwhile. A run of
        // failures surfaces as the "stats stalled" indicator instead, driven
        // by `lastSuccessAt` rather than a per-failure toast.
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
    if (row.targetId === null) return
    // Both paths open an EXISTING confirmation dialog rather than acting now.
    // Guard per KIND: a terminal (of either owner) needs only its target id.
    // A project terminal's `sessionId` is null, and a session-null early
    // return here would leave its Stop button dead.
    if (row.kind === "terminal") {
      openDeleteTerminal(row.targetId)
      return
    }
    if (row.sessionId === null) return
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
              // Destructive here, unlike a `⋯` menu's neutral destructive items
              // (CLAUDE.md scopes that tenet to DropdownMenuItems): this is a
              // dialog footer button, the exact surface the tenet reserves
              // `variant="destructive"` for.
              <Button variant="destructive" onClick={openStopAll}>
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

// The header's "still live" cue: a small muted pill with a blinking dot,
// reading "updating every Ns" with the interval read straight from the real
// poll constant so the copy cannot drift from the actual cadence. Reuses the
// existing `.agent-status-dot`/`--on` pulse (StatusBadge's streaming dot):
// same blink, same `prefers-reduced-motion` handling, one definition.
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

// The leading row icon: reuses the SAME icons the rest of the app already
// uses for these kinds, rather than inventing new ones (Sidebar.tsx /
// MobileShell.tsx / AgentTabsStrip.tsx all use `Bot` for an agent and
// `SquareTerminal` for a companion terminal). `dux` gets `Activity`, matching
// the app menu's own Task Manager entry (`lib/appMenu.ts`), so the app's own
// process reads as "the activity monitor icon", not another agent. TOTAL gets
// none: it is a summary row, not a process.
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

// Marks the dux row as "this process", not another agent. Reuses the shared
// `Badge` primitive (see StatusBadge.tsx) rather than a hand-rolled span.
// Cyan-tinted per the approved mock, but deliberately STATIC (no
// `animate-attention-pulse`): that blink is reserved for "needs attention"
// elsewhere (AttentionDot, Sidebar, MobileShell all pair cyan with the pulse
// and a tooltip), and dux never needs the user's attention, so this pill
// borrows the color, not the motion.
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
      —
    </span>
  )
}

// The expand caret. Every KIND expands, terminals included: the collector runs
// the same tree walk over every target. What decides is whether this row's tree
// actually has anything to break down.
//
// The gate is core's `has_breakdown`, never `children.length`: `children`
// always includes the row's own root process, so a leaf (a provider that
// spawned no subprocesses, the common case) has length 1 and a
// `length === 0` test leaves every single row expandable. Expanding one then
// reveals exactly one child: a duplicate of the row just expanded.
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
      // >=40px touch target on phones (CLAUDE.md's touch-target tenet): this
      // control sits directly beside the correctly-sized Stop button in the
      // mobile row, and a tiny chevron next to it is a misclick hazard on the
      // one control a misclick would be worst on. Desktop keeps the compact
      // 16px density.
      className="inline-flex size-4 max-md:size-10 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground"
    >
      <Icon className="size-3.5" />
    </button>
  )
}

// Stat cells read as dashes when this row had no sample: a dormant tab, or a
// process born since the last poll. The row still renders and stays stoppable.
//
// `pid` is handled by the CALLER, not here: TOTAL has no pid at all (blank,
// not a dash: it is not a single process), which is a different "nothing to
// show" than "no sample yet".
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
  // TOTAL has no pid at all (blank: it is a summary, not a process); every
  // other row shows the real pid, or a dash before the first sample lands.
  const pid = isTotal ? "" : row.stats?.pid != null ? String(row.stats.pid) : "—"
  // Gate the BODY on the same rule as the toggle, not just the toggle: an
  // expanded row whose subprocess exits between polls drops to a lone root
  // entry, and the caret vanishing is not enough to stop the duplicate from
  // rendering under it. The expansion set is keyed by row and outlives the
  // shape of the tree it was opened on.
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
              {/* A child process has no process count of its own (it IS one
                  process): render this cell empty rather than inventing a "1",
                  and never put the pid here, which is the bug being fixed
                  (a child's pid was previously rendered under Procs). */}
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
              {/* The phone layout is cards, not a table: there is no PID
                  column to put this in, so it is labelled inline rather than
                  presented as a bare number that could be misread as a
                  process count (the bug being fixed on desktop). */}
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

// The bulk stop's confirmation. Nested inside the Task Manager, and destructive-
// styled (unlike the Task Manager itself): this one really is only a dangerous
// action. Follows the established pattern: Cancel `autoFocus`, the confirm
// `variant="destructive"`, misclick-safe spacing.
function ConfirmStopAllDialog({ open }: { open: boolean }) {
  const { spine } = useDux()
  const sessions = spine?.sessions ?? []
  const agents = sessions.filter((s) => s.status === "active").length
  // Count terminals across BOTH owners: session terminals plus project
  // terminals, matching exactly what `stopAllRunning` will stop.
  const terminals =
    sessions.reduce((n, s) => n + s.terminals.length, 0) +
    (spine?.projects ?? []).reduce((n, p) => n + p.terminals.length, 0)

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
