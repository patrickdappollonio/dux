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

import { AttachedSection } from "@/components/AttachedSection"
import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal, useModalLifetime } from "@/components/modal"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { usePartChangeReason } from "@/hooks/use-agent-active-elsewhere"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { useIsMobile, useIsNarrowComputerLayout } from "@/hooks/use-mobile"
import { useIsTruncated } from "@/hooks/use-truncated"
import { FOCUS_RING } from "@/lib/focusRing"
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
import { useRowWatch } from "@/lib/paneCover"
import { renderProse } from "@/lib/prose"
import {
  closeStopAll,
  leftRunningProse,
  type StopAllRefusal,
  stopAllNothingReason,
  stopAllPlan,
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
  const life = useModalLifetime(taskManagerOpen)
  return (
    <>
      {/* Mounted only from an open until its close has finished, so the sample
          and the expansion state reset by unmounting rather than by being
          cleared, and the poll loop is torn down by the effect's own cleanup. */}
      {life.mounted ? (
        <TaskManager key={life.epoch} open={taskManagerOpen} onClosed={life.onClosed} />
      ) : null}
      <ConfirmStopAllDialog open={stopAllOpen} />
    </>
  )
}

function TaskManager({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const state = useDux()
  const { spine } = state
  // ONE layout renders at a time, not two hidden behind CSS: rendering both would
  // duplicate every row, and every Stop control, in the DOM. A computer layout
  // in a window narrower than the breakpoint has no room for the table's
  // columns beside a name either, so it stacks the rows as a phone does.
  const phone = useIsMobile()
  const isMobile = useIsNarrowComputerLayout() || phone
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
  const changeReason = usePartChangeReason()
  // Every running row this device may not change leaves the bulk stop nothing.
  const nothingToStop = stopAllNothingReason(stopAllPlan(state))
  // Why a row's Force stop is disabled: stopping changes the row it belongs to.
  function stopBlocked(row: TaskRow): string | null {
    if (!row.stoppable || row.targetId === null) return null
    return row.sessionId !== null
      ? changeReason({ sessionId: row.sessionId })
      : changeReason({ terminalId: row.targetId })
  }

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
    // A report, not a destructive confirm: the greyed backdrop suits a kill
    // modal but fights a monitor you read numbers off. The nested
    // confirmations keep it.
    <Modal
      kind="report"
      open={open}
      onClose={closeTaskManager}
      onClosed={onClosed}
      size="w704"
      title="Task Manager"
      description="What's running, what it costs, and how to stop it. Agents detach and can be reconnected; terminals are destroyed."
      // Whether the numbers are live, above the list and out of its scroll.
      // Never both at once: the pill claims the numbers are live and the
      // stalled message says they are not. A permanently failing poll must not
      // go on rendering the last good numbers as though they were live: the
      // message appears once a run of failures crosses the staleness threshold
      // and clears the moment a poll succeeds again. No toast: this is a
      // persistent state of the numbers, not a one-off event.
      toolbar={
        stale ? (
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
            Stats stalled: showing the last successful sample, not live numbers.
          </p>
        ) : (
          <LivePollPill intervalMs={RESOURCE_POLL_INTERVAL_MS} />
        )
      }
      closeLabel="Done"
      actions={
        empty
          ? undefined
          : [
              {
                label: "Force stop everything…",
                tone: "destructive",
                onSelect: openStopAll,
                disabledReason: nothingToStop,
              },
            ]
      }
      footerNote={summary ? <span className="tabular-nums">{summary}</span> : undefined}
    >
      {/* A name too long for its row is shortened, with the whole of it in a
          hint (see `ShortenedName`), so the list never scrolls sideways and no
          row's Force stop is ever out of reach. On a computer the list scrolls
          inside the card, with a stable gutter so the scrollbar never touches
          the Force stop buttons or the dashes; header and rows share one table,
          so their columns stay aligned whether or not the list scrolls. On a
          phone the cover's body is the scroller. */}
      <div
        data-testid="task-manager-scroll"
        className="md:max-h-96 md:overflow-y-auto md:pr-2 md:[scrollbar-gutter:stable]"
      >
        {/* Desktop: the table. */}
        {isMobile ? null : (
          <table className="w-full table-fixed border-collapse text-sm">
            <thead>
              {/* Each column but the name's is as wide as the widest value it
                  can hold ("4194304", "9999", "12800.0%" on a machine with
                  128 cores, "1023.9 MiB", the Force stop button), and the
                  fixed layout keeps it at that width whatever the rows hold,
                  so the width the name is left does not move as the numbers
                  change, and a name at the edge is not cut and whole by turns. */}
              <tr className="text-[10px] tracking-wide text-muted-foreground uppercase">
                <th className="py-1 pr-2 text-left font-medium">Name</th>
                <th className="w-[4.5rem] py-1 pr-2 text-right font-medium">PID</th>
                <th className="w-14 py-1 pr-2 text-right font-medium">Procs</th>
                <th className="w-20 py-1 pr-2 text-right font-medium">CPU</th>
                <th className="w-24 py-1 pr-2 text-right font-medium">Memory</th>
                <th className="w-28 py-1" />
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
                  stopBlocked={stopBlocked(row)}
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
                stopBlocked={stopBlocked(row)}
              />
            ))}
          </div>
        ) : null}

        {empty ? (
          <p className="px-2 py-6 text-center text-sm text-muted-foreground">
            Nothing is running.
          </p>
        ) : null}
      </div>
    </Modal>
  )
}

// The "still live" cue. The interval is read from the poll constant so
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
      // 40px touch target on phones and under a finger at any width: this chevron
      // sits beside Stop, the one control a misclick would be worst on. A mouse
      // keeps the compact density.
      className="inline-flex size-4 max-md:size-10 pointer-coarse:size-10 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground"
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

// A name in the list, cut with an ellipsis when its row has no room for all of
// it. While it is cut, the whole name is in the shared tooltip: on hover, on
// keyboard focus (the name is a tab stop only then) and, for a finger, on a tap.
function ShortenedName({ name, className }: { name: string; className?: string }) {
  const { ref, truncated } = useIsTruncated<HTMLSpanElement>(name)
  const [hovered, setHovered] = useState(false)
  const [tapped, setTapped] = useState(false)
  // A tap asked for the hint of a name that was cut; once it is whole the
  // hint has nothing to add, and it must not come back by itself if the
  // name is cut again.
  if (tapped && !truncated) setTapped(false)
  return (
    <SimpleTooltip
      content={name}
      open={truncated && (hovered || tapped)}
      onOpenChange={(next) => {
        setHovered(next)
        if (!next) setTapped(false)
      }}
    >
      <span
        ref={ref}
        data-shortened={truncated ? "" : undefined}
        tabIndex={truncated ? 0 : undefined}
        onClick={truncated ? () => setTapped(true) : undefined}
        className={cn("min-w-0 truncate rounded-sm", FOCUS_RING, className)}
      >
        {name}
      </span>
    </SimpleTooltip>
  )
}

function DesktopRow({
  row,
  expanded,
  onToggle,
  onStop,
  stopBlocked,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
  onStop: () => void
  /** Why the stop is disabled, or null while it is not. */
  stopBlocked: string | null
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
          "group/task hover:bg-muted/50",
          isTotal && "border-t border-border font-medium",
        )}
      >
        {/* The one column that gives way: it takes the width the others
            leave, and `max-w-0` lets it go below its content's, so a long
            name is shortened rather than widening the table. */}
        <td className="w-full max-w-0 py-1.5 pr-2">
          <div
            className={cn("flex min-w-0 items-center gap-1.5", row.nested && "pl-5")}
          >
            {isTotal ? (
              <span className="inline-block w-4" />
            ) : (
              <ExpandToggle row={row} expanded={expanded} onToggle={onToggle} />
            )}
            <RowIcon kind={row.kind} />
            <ShortenedName name={row.name} />
            {row.kind === "dux" ? <DuxBadge /> : null}
            {/* Kept whole while it is short, and never more than two fifths
                of the cell: the name is what gives way to it. */}
            {row.detail ? (
              <ShortenedName
                name={row.detail}
                className="max-w-[40%] shrink-0 font-mono text-xs text-muted-foreground"
              />
            ) : null}
          </div>
        </td>
        {/* `tabular-nums` so digits do not jitter as the numbers change, and
            `whitespace-nowrap` so "45.8 MiB" never breaks across two lines. */}
        <td className="py-1.5 pr-2 text-right whitespace-nowrap tabular-nums">{pid}</td>
        <td className="py-1.5 pr-2 text-right whitespace-nowrap tabular-nums">{procs}</td>
        <td className="py-1.5 pr-2 text-right whitespace-nowrap tabular-nums">{cpu}</td>
        <td className="py-1.5 pr-2 text-right whitespace-nowrap tabular-nums">{mem}</td>
        <td className="py-1.5 text-right">
          {row.stoppable ? (
            <ExplainedWhenDisabled disabled={stopBlocked !== null} hint={stopBlocked} className="rounded-lg">
              <Button
                size="sm"
                variant="outline"
                onClick={onStop}
                disabled={stopBlocked !== null}
                aria-label={row.stopLabel}
                className="max-md:min-h-10 pointer-coarse:min-h-10"
              >
                <CircleStop aria-hidden />
                Force stop
              </Button>
            </ExplainedWhenDisabled>
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
              <td className="w-full max-w-0 py-0.5 pr-2">
                <div className={cn("flex min-w-0 items-baseline pl-10", row.nested && "pl-15")}>
                  <ShortenedName name={child.name} className="font-mono" />
                  {/* The root is in its own breakdown so the entries sum to the
                      row's total. Label it, or it reads as a phantom duplicate
                      of the row above. */}
                  {child.is_root ? (
                    <span className="ml-1.5 shrink-0 whitespace-nowrap text-muted-foreground/70">
                      (this process)
                    </span>
                  ) : null}
                </div>
              </td>
              <td className="py-0.5 pr-2 text-right whitespace-nowrap tabular-nums">
                {child.pid}
              </td>
              {/* A child is one process, so its Procs cell is intentionally empty. */}
              <td className="py-0.5 pr-2 text-right whitespace-nowrap tabular-nums" />
              <td className="py-0.5 pr-2 text-right whitespace-nowrap tabular-nums">
                {formatCpu(child.cpu_percent)}
              </td>
              <td className="py-0.5 pr-2 text-right whitespace-nowrap tabular-nums">
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
  stopBlocked,
}: {
  row: TaskRow
  expanded: boolean
  onToggle: () => void
  onStop: () => void
  /** Why the stop is disabled, or null while it is not. */
  stopBlocked: string | null
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
            <div className="flex min-w-0 items-center gap-1.5 text-sm">
              <ShortenedName name={row.name} />
              {row.kind === "dux" ? <DuxBadge /> : null}
            </div>
            <div className="truncate text-xs text-muted-foreground tabular-nums">
              {cpu} · {mem} · {procs} procs
            </div>
          </div>
        </div>
        {row.stoppable ? (
          <ExplainedWhenDisabled disabled={stopBlocked !== null} hint={stopBlocked} className="rounded-lg">
            <Button
              size="sm"
              variant="outline"
              onClick={onStop}
              disabled={stopBlocked !== null}
              aria-label={row.stopLabel}
              className="max-md:min-h-10 pointer-coarse:min-h-10 shrink-0"
            >
              <CircleStop aria-hidden />
              Force stop
            </Button>
          </ExplainedWhenDisabled>
        ) : null}
      </div>
      {expanded ? (
        <ul className="mt-1 flex flex-col gap-0.5 pl-6">
          {children.map((child) => (
            <li
              key={child.pid}
              className="flex items-center justify-between gap-2 text-xs text-muted-foreground"
            >
              <span className="flex min-w-0 items-baseline font-mono">
                <ShortenedName name={child.name} />
                {/* Same root marker as desktop: the breakdown includes the row's
                    own process so the numbers add up, and it must not read as a
                    phantom duplicate. */}
                {child.is_root ? (
                  <span className="ml-1.5 shrink-0 text-muted-foreground/70">
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
  const state = useDux()
  // Re-read whenever a pane hears who drives its row.
  useRowWatch(null)
  // Exactly what `stopAllRunning` will stop, and the rows it leaves to the
  // device that drives them.
  const plan = stopAllPlan(state)
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    open,
    "stop-all",
  )
  // The stops refused for somebody attached, which are all going ahead
  // retries: everything else already stopped. Forgotten with the dialog.
  const [refused, setRefused] = useState<StopAllRefusal | null>(null)
  const [wasOpen, setWasOpen] = useState(open)
  if (wasOpen !== open) {
    setWasOpen(open)
    if (!open) setRefused(null)
  }
  // What the press will stop: the whole plan, or only the refused stops.
  const agents = (refused?.agents ?? plan.agents).length
  const terminals = (refused?.terminals ?? plan.terminals).length
  const nothingToStop = refused === null ? stopAllNothingReason(plan) : null

  async function handleConfirm() {
    const done = await confirm(async (accepted) => {
      const answer = await stopAllRunning(refused, accepted)
      if (answer !== null) setRefused(answer)
      return answer?.blockers ?? null
    })
    if (done) closeStopAll()
  }

  return (
    <Modal
      kind="confirm"
      open={open}
      onClose={closeStopAll}
      title="Force stop everything?"
      description={
        <>
          {`This stops ${formatRegularCount(agents, "agent")} and ${formatRegularCount(terminals, "terminal")} immediately, with no shutdown wait. `}
          Agents detach and stay in Projects, reopenable; terminals are
          destroyed and cannot be recovered.
        </>
      }
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Force stop everything",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
        drivenHint: nothingToStop,
      })}
    >
      {/* One child, null when there is nothing to add: a body of empty parts
          would still be drawn, as a gap under the question. */}
      {plan.left.length > 0 || blockers?.length ? (
        <>
          {plan.left.length > 0 ? (
            <p className="text-sm text-muted-foreground">
              {renderProse(leftRunningProse(plan.left))}
            </p>
          ) : null}
          <AttachedSection blockers={blockers} />
        </>
      ) : null}
    </Modal>
  )
}
