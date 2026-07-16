// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { RESOURCE_POLL_INTERVAL_MS, STALE_STATS_THRESHOLD_MS } from "@/lib/resourcePoll"
import type { ResourceStatsView } from "@/lib/resourcesApi"
import type { AgentTabView, SessionView, TerminalView } from "@/lib/types"

// Spy on the store actions the dialog routes stops through, while `useDux` reads
// our seeded state. The stops MUST open existing confirmations rather than act,
// so these are the assertions that prove "every stop confirms".
const openCloseTab = vi.fn()
const openDeleteTerminal = vi.fn()
const openStopAll = vi.fn()
const closeStopAll = vi.fn()
const stopAllRunning = vi.fn()
const closeTaskManager = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openCloseTab: (s: string, t: string) => openCloseTab(s, t),
    openDeleteTerminal: (t: string) => openDeleteTerminal(t),
    openStopAll: () => openStopAll(),
    closeStopAll: () => closeStopAll(),
    stopAllRunning: () => stopAllRunning(),
    closeTaskManager: () => closeTaskManager(),
  }
})

const getResources = vi.fn()
vi.mock("@/lib/resourcesApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/resourcesApi")>()
  return { ...actual, resourcesApi: { get: () => getResources() } }
})

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
  // jsdom lacks matchMedia, which `useIsMobile` reads. `matches: false` puts
  // these tests on the desktop (table) layout.
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
installBootStubs()
const { TaskManagerDialog } = await import("./TaskManagerDialog")

function tab(over: Partial<AgentTabView> & { id: string }): AgentTabView {
  return {
    provider: "claude",
    order: 0,
    working: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
    ...over,
  }
}

function session(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    project_id: "p1",
    title: null,
    provider: "claude",
    branch_name: "feat",
    initial_branch: "feat",
    source_branch: "main",
    worktree_path: "/wt",
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
    tabs: [tab({ id: over.id })],
    has_output: false,
    working: false,
    needs_attention: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  } as SessionView
}

function terminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return { label: "Terminal 1", has_output: true, foreground_cmd: null, ...over }
}

function stat(over: Partial<ResourceStatsView>): ResourceStatsView {
  return {
    id: null,
    kind: "agent",
    label: "row",
    pid: 1,
    cpu_percent: 0,
    rss_bytes: 0,
    process_count: 1,
    children: [],
    ...over,
  }
}

const duxStat = stat({
  kind: "dux",
  label: "dux (this process)",
  cpu_percent: 1.2,
  rss_bytes: 50_648_678,
})
const totalStat = stat({ kind: "total", label: "TOTAL" })

function seed(over: Partial<DuxState> = {}) {
  mockState = {
    taskManagerOpen: true,
    stopAllOpen: false,
    spine: { sessions: [] },
    ...over,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  vi.clearAllMocks()
  getResources.mockResolvedValue({ rows: [duxStat, totalStat] })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("TaskManagerDialog", () => {
  it("renders_rows_with_cpu_memory_and_proc_count", async () => {
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({
          id: "s1",
          kind: "agent",
          label: "Agent (claude): fix-auth",
          cpu_percent: 3.4,
          rss_bytes: 402653184,
          process_count: 7,
        }),
        totalStat,
      ],
    })
    seed({ spine: { sessions: [session({ id: "s1", title: "fix-auth" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    await waitFor(() => expect(screen.getByText("3.4%")).toBeTruthy())
    expect(screen.getByText("384.0 MiB")).toBeTruthy()
    expect(screen.getByText("7")).toBeTruthy()
    expect(screen.getAllByText("fix-auth").length).toBeGreaterThan(0)
  })

  it("dux_row_has_no_stop_control", async () => {
    seed({ spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    await waitFor(() => expect(screen.getByText("dux")).toBeTruthy())
    // dux and TOTAL never offer a Stop; only the agent row does.
    expect(screen.queryByLabelText("Stop dux")).toBeNull()
    expect(screen.queryByLabelText("Stop TOTAL")).toBeNull()
  })

  it("stop_agent_confirms_then_kills", async () => {
    // The stop must OPEN the close-tab confirmation, never kill on the click.
    seed({ spine: { sessions: [session({ id: "s1", title: "fix-auth" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const stop = await screen.findByLabelText("Stop fix-auth")
    fireEvent.click(stop)
    expect(openCloseTab).toHaveBeenCalledWith("s1", "s1")
  })

  it("stop_terminal_confirms_then_deletes", async () => {
    seed({
      spine: {
        sessions: [
          session({
            id: "s1",
            terminals: [terminal({ id: "term-1", label: "Terminal 1" })],
          }),
        ],
      },
    } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const stop = await screen.findByLabelText("Stop Terminal 1")
    fireEvent.click(stop)
    // Routes through the EXISTING terminal confirmation, not a direct delete.
    expect(openDeleteTerminal).toHaveBeenCalledWith("term-1")
  })

  it("stop_all_confirms_before_killing", async () => {
    seed({ spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    const { rerender } = render(<TaskManagerDialog />)

    fireEvent.click(await screen.findByText("Stop all…"))
    expect(openStopAll).toHaveBeenCalledOnce()
    // Nothing is stopped by opening the confirmation.
    expect(stopAllRunning).not.toHaveBeenCalled()

    // With the confirmation up, the confirm button is what actually stops.
    seed({
      stopAllOpen: true,
      spine: { sessions: [session({ id: "s1" })] },
    } as Partial<DuxState>)
    rerender(<TaskManagerDialog />)
    fireEvent.click(await screen.findByText("Stop all"))
    expect(stopAllRunning).toHaveBeenCalledOnce()
  })

  it("auto_closes_when_last_runtime_stops", async () => {
    seed({ spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    const { rerender } = render(<TaskManagerDialog />)
    await screen.findByLabelText("Stop feat")
    expect(closeTaskManager).not.toHaveBeenCalled()

    // The last runtime went away while the dialog was open.
    seed({ spine: { sessions: [] } } as Partial<DuxState>)
    rerender(<TaskManagerDialog />)
    await waitFor(() => expect(closeTaskManager).toHaveBeenCalledOnce())
  })

  it("does_not_auto_close_when_opened_already_empty", async () => {
    // Opening with nothing running must show "Nothing is running.", not flash
    // shut before it can be read.
    seed({ spine: { sessions: [] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    await waitFor(() => expect(screen.getByText("Nothing is running.")).toBeTruthy())
    expect(closeTaskManager).not.toHaveBeenCalled()
  })

  it("shows_nothing_is_running_when_empty", async () => {
    seed({ spine: { sessions: [] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    await waitFor(() => expect(screen.getByText("Nothing is running.")).toBeTruthy())
    // The bulk stop is pointless with nothing to stop.
    expect(screen.queryByText("Stop all…")).toBeNull()
  })

  it("renders_a_row_without_stats_as_dashes_but_still_stoppable", async () => {
    // A dormant tab has no sample. It must still render and still be stoppable.
    seed({
      spine: {
        sessions: [
          session({
            id: "s1",
            tabs: [
              tab({ id: "s1" }),
              tab({ id: "t2", order: 1, provider: "codex", has_live_process: false }),
            ],
          }),
        ],
      },
    } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const row = await screen.findByTestId("task-row-tab:t2")
    expect(row.textContent).toContain("—")
    // The extra tab's Stop label carries the owning agent and its position,
    // not just the bare provider (finding 4): "Stop codex" alone would
    // collide with any other codex extra tab on any other agent.
    expect(screen.getByLabelText("Stop codex tab 1 in feat")).toBeTruthy()
  })

  it("expands_child_processes_for_a_terminal", async () => {
    // Child rows expand for EVERY kind, terminals included.
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({
          id: "term-1",
          kind: "terminal",
          label: "Terminal (npm): dev server",
          cpu_percent: 12.1,
          rss_bytes: 201_700_000,
          process_count: 4,
          children: [
            { name: "node", pid: 4242, cpu_percent: 11.0, rss_bytes: 180_000_000 },
          ],
        }),
        totalStat,
      ],
    })
    seed({
      spine: {
        sessions: [
          session({ id: "s1", terminals: [terminal({ id: "term-1", label: "Terminal 1" })] }),
        ],
      },
    } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const toggle = await screen.findByLabelText("Show Terminal 1 child processes")
    expect(screen.queryByText("node")).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getAllByText("node").length).toBeGreaterThan(0)
  })

  it("does_not_poll_while_closed", async () => {
    // A closed dialog costs the server nothing: that backpressure is the reason
    // stats are a REST read rather than a pushed event.
    seed({ taskManagerOpen: false, spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    await new Promise((r) => setTimeout(r, 20))
    expect(getResources).not.toHaveBeenCalled()
  })

  it("mobile_expand_toggle_meets_the_touch_target_floor_beside_stop", async () => {
    // The chevron sits directly beside the Stop button in the mobile row; a
    // sub-40px target there is a misclick hazard on the one control a
    // misclick would be worst on (CLAUDE.md's touch-target tenet).
    vi.stubGlobal(
      "matchMedia",
      vi.fn((query: string) => ({
        matches: true,
        media: query,
        onchange: null,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false,
      })),
    )
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({
          id: "s1",
          kind: "agent",
          label: "Agent (claude): fix-auth",
          children: [
            { name: "node", pid: 4242, cpu_percent: 1, rss_bytes: 1024 },
          ],
        }),
        totalStat,
      ],
    })
    seed({ spine: { sessions: [session({ id: "s1", title: "fix-auth" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const toggle = await screen.findByLabelText("Show fix-auth child processes")
    expect(toggle.className).toMatch(/max-md:size-10|max-md:min-h-10/)
  })

  it("shows_a_stale_indicator_after_repeated_poll_failures_and_stops_after_a_recovery", async () => {
    // The poll's catch is empty by design: a single dropped sample renders the
    // last good numbers, unremarked. But a run of failures must not render
    // those numbers as fresh forever (finding 7).
    vi.useFakeTimers({ shouldAdvanceTime: true })
    seed({ spine: { sessions: [session({ id: "s1", title: "fix-auth" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    // First sample succeeds, so the numbers land and the indicator is absent.
    await vi.waitFor(() => expect(getResources).toHaveBeenCalledTimes(1))
    expect(screen.queryByText(/stalled/i)).toBeNull()

    // Every poll after that fails. Advance past the staleness threshold.
    getResources.mockRejectedValue(new Error("offline"))
    await vi.advanceTimersByTimeAsync(STALE_STATS_THRESHOLD_MS + 2000)

    expect(screen.getByText(/stalled/i)).toBeTruthy()

    // A later success clears the indicator: the numbers are fresh again.
    getResources.mockResolvedValue({ rows: [duxStat, totalStat] })
    await vi.advanceTimersByTimeAsync(2000)
    expect(screen.queryByText(/stalled/i)).toBeNull()

    vi.useRealTimers()
  })

  it("does_not_clamp_cpu_above_one_hundred_percent", async () => {
    // A busy tree across cores legitimately exceeds 100%; the Task Manager must
    // show the runaway, not hide it behind a clamp.
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({ id: "s1", kind: "agent", cpu_percent: 129.5, process_count: 3 }),
        totalStat,
      ],
    })
    seed({ spine: { sessions: [session({ id: "s1", title: "hot" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    await waitFor(() => expect(screen.getByText("129.5%")).toBeTruthy())
  })

  it("stop_all_trigger_is_destructive", async () => {
    // The user's own words: "The 'Stop all' button should also be red." Unlike
    // a `⋯` menu's neutral destructive items, a dialog footer button IS where
    // CLAUDE.md's menu tenet reserves `variant="destructive"`.
    seed({ spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    const trigger = await screen.findByText("Stop all…")
    expect(trigger.className).toContain("destructive")
  })

  it("each_stoppable_rows_stop_button_renders_an_icon", async () => {
    seed({ spine: { sessions: [session({ id: "s1", title: "fix-auth" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    const stop = await screen.findByLabelText("Stop fix-auth")
    expect(stop.querySelector("svg")).toBeTruthy()
  })

  it("dux_row_renders_its_badge_and_still_has_no_stop_control", async () => {
    seed({ spine: { sessions: [session({ id: "s1" })] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    await screen.findByText("dux")
    expect(screen.getByText("this process")).toBeTruthy()
    expect(screen.queryByLabelText("Stop dux")).toBeNull()
  })

  it("header_shows_the_live_interval_derived_from_the_poll_constant", async () => {
    // Never a hand-typed number: the pill's text must trace back to the real
    // poll cadence, so it cannot silently drift from `resourcePoll.ts`.
    seed({ spine: { sessions: [] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    expect(
      await screen.findByText(`Updating every ${RESOURCE_POLL_INTERVAL_MS / 1000}s`),
    ).toBeTruthy()
  })

  it("the_live_pills_dot_reuses_the_already_reduced_motion_safe_status_dot", async () => {
    // Rather than a fresh, unguarded animation, the blinking dot reuses
    // StatusBadge's `.agent-status-dot`/`--on` mechanism, whose
    // `prefers-reduced-motion` handling (index.css) already applies here for
    // free.
    seed({ spine: { sessions: [] } } as Partial<DuxState>)
    render(<TaskManagerDialog />)
    const pill = await screen.findByText(/Updating every/)
    const dot = pill.querySelector("svg")
    expect(dot?.getAttribute("class")).toContain("agent-status-dot")
    expect(dot?.getAttribute("class")).toContain("agent-status-dot--on")
  })

  it("child_row_renders_its_pid_under_pid_and_an_empty_procs_cell", async () => {
    // Regression: a child process has no process count of its own. Putting
    // its pid under Procs read as "over 2 million subprocesses" in the wild.
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({
          id: "term-1",
          kind: "terminal",
          label: "Terminal (npm): dev server",
          process_count: 4,
          children: [
            { name: "node", pid: 4242, cpu_percent: 11.0, rss_bytes: 180_000_000 },
          ],
        }),
        totalStat,
      ],
    })
    seed({
      spine: {
        sessions: [
          session({ id: "s1", terminals: [terminal({ id: "term-1", label: "Terminal 1" })] }),
        ],
      },
    } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const toggle = await screen.findByLabelText("Show Terminal 1 child processes")
    fireEvent.click(toggle)

    const childRow = await screen.findByTestId("child-row-4242")
    const cells = within(childRow).getAllByRole("cell")
    // Name, PID, Procs, CPU, Memory, (action).
    expect(cells[1].textContent).toBe("4242")
    expect(cells[2].textContent).toBe("")
  })

  it("child_rows_procs_cell_never_contains_the_pid_value", async () => {
    getResources.mockResolvedValue({
      rows: [
        duxStat,
        stat({
          id: "term-1",
          kind: "terminal",
          label: "Terminal (npm): dev server",
          process_count: 4,
          children: [
            { name: "node", pid: 4242, cpu_percent: 11.0, rss_bytes: 180_000_000 },
          ],
        }),
        totalStat,
      ],
    })
    seed({
      spine: {
        sessions: [
          session({ id: "s1", terminals: [terminal({ id: "term-1", label: "Terminal 1" })] }),
        ],
      },
    } as Partial<DuxState>)
    render(<TaskManagerDialog />)

    const toggle = await screen.findByLabelText("Show Terminal 1 child processes")
    fireEvent.click(toggle)

    const childRow = await screen.findByTestId("child-row-4242")
    const cells = within(childRow).getAllByRole("cell")
    expect(cells[2].textContent).not.toContain("4242")
  })
})
