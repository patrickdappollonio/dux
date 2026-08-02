import { describe, expect, it } from "vitest"

import { taskManagerRows, taskManagerSummary } from "./resourceRows"
import type { ResourceStatsView } from "./resourcesApi"
import type { AgentTabView, ProjectView, SessionView, TerminalView } from "./types"

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

function terminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    owner: { kind: "session", session_id: "s1" },
    label: "term",
    has_output: false,
    foreground_cmd: null,
    ...over,
  } as TerminalView
}

// A project-owned terminal, the same builder with the owner flipped.
function projectTerminal(
  over: Partial<TerminalView> & { id: string; projectId: string },
): TerminalView {
  const { projectId, ...rest } = over
  return terminal({ ...rest, owner: { kind: "project", project_id: projectId } })
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
    tabs: [tab({ id: over.id })],
    has_output: false,
    working: false,
    needs_attention: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  } as SessionView
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
  id: null,
  kind: "dux",
  label: "dux (this process)",
  cpu_percent: 1.2,
  rss_bytes: 50_000_000,
})
const totalStat = stat({ id: null, kind: "total", label: "TOTAL" })

function project(over: Partial<ProjectView> & { id: string }): ProjectView {
  return {
    name: over.id,
    ...over,
  } as unknown as ProjectView
}

describe("taskManagerRows", () => {
  it("emits_project_terminal_rows_with_stats_joined_and_project_detail", () => {
    // The trap this guards (T5): project terminals never appeared in the Task
    // Manager at all, even though the server samples them (the resource
    // monitor iterates the whole terminal map).
    const projects = [project({ id: "p1", name: "Repo" })]
    const terminals = [
      projectTerminal({ id: "pt-1", projectId: "p1", label: "Terminal 2" }),
    ]
    const stats = [
      duxStat,
      stat({
        id: "pt-1",
        kind: "terminal",
        label: "Terminal: Terminal 2",
        cpu_percent: 2.5,
        rss_bytes: 10_485_760,
        process_count: 2,
      }),
      totalStat,
    ]
    const rows = taskManagerRows([], stats, projects, terminals)
    const row = rows.find((r) => r.key === "term:pt-1")
    expect(row).toBeDefined()
    expect(row?.kind).toBe("terminal")
    expect(row?.detail).toBe("Repo")
    expect(row?.sessionId).toBeNull()
    expect(row?.projectId).toBe("p1")
    expect(row?.targetId).toBe("pt-1")
    expect(row?.stoppable).toBe(true)
    // A broken stats join would render blank CPU/RSS for a sampled terminal.
    expect(row?.stats?.cpu_percent).toBe(2.5)
    expect(row?.stats?.rss_bytes).toBe(10_485_760)
  })

  it("a_lone_project_terminal_means_something_is_running", () => {
    // The trap this guards (T6): with only a project terminal live the dialog
    // said "Nothing is running." and auto-closed.
    const projects = [project({ id: "p1" })]
    const terminals = [projectTerminal({ id: "pt-1", projectId: "p1" })]
    const rows = taskManagerRows([], [duxStat, totalStat], projects, terminals)
    expect(rows.some((r) => r.stoppable)).toBe(true)
  })

  it("joins_stats_to_tabs_by_id", () => {
    const sessions = [session({ id: "s1", title: "fix-auth" })]
    const stats = [
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
    ]

    const rows = taskManagerRows(sessions, stats, [], [])
    const agent = rows.find((r) => r.key === "tab:s1")
    expect(agent?.stats?.cpu_percent).toBe(3.4)
    expect(agent?.stats?.process_count).toBe(7)
  })

  it("does_not_join_by_label_when_two_agents_share_a_title", () => {
    // The whole reason stats carry an id: two agents with the same title must
    // not be conflated. A label-matching join would give both the same numbers.
    const sessions = [
      session({ id: "s1", title: "fix-auth" }),
      session({ id: "s2", title: "fix-auth", tabs: [tab({ id: "s2" })] }),
    ]
    const stats = [
      duxStat,
      stat({ id: "s1", label: "Agent (claude): fix-auth", cpu_percent: 1 }),
      stat({ id: "s2", label: "Agent (claude): fix-auth", cpu_percent: 9 }),
      totalStat,
    ]

    const rows = taskManagerRows(sessions, stats, [], [])
    expect(rows.find((r) => r.key === "tab:s1")?.stats?.cpu_percent).toBe(1)
    expect(rows.find((r) => r.key === "tab:s2")?.stats?.cpu_percent).toBe(9)
  })

  it("renders_row_without_stats_as_dashes", () => {
    // A dormant tab (or one born since the last poll) has no stats. It must
    // still render, and must still be stoppable. Never drop a killable row for
    // lack of numbers.
    const sessions = [
      session({
        id: "s1",
        tabs: [tab({ id: "s1" }), tab({ id: "t2", has_live_process: false })],
      }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], [])
    const dormant = rows.find((r) => r.key === "tab:t2")
    expect(dormant).toBeDefined()
    expect(dormant?.stats).toBeNull()
    expect(dormant?.stoppable).toBe(true)
  })

  it("drops_orphan_stats_not_in_spine", () => {
    // A runtime killed between the poll and the spine refetch. The spine is
    // authoritative for existence, so its row is gone.
    const sessions = [session({ id: "s1" })]
    const stats = [
      duxStat,
      stat({ id: "s1", label: "Agent (claude): s1" }),
      stat({ id: "ghost", label: "Agent (claude): ghost" }),
      totalStat,
    ]
    const rows = taskManagerRows(sessions, stats, [], [])
    expect(rows.some((r) => r.key === "tab:ghost")).toBe(false)
  })

  it("pins_dux_first_and_total_last", () => {
    const sessions = [session({ id: "s1" })]
    const rows = taskManagerRows(sessions, [
      duxStat,
      stat({ id: "s1", label: "Agent (claude): s1" }),
      totalStat,
    ], [], [])
    expect(rows[0].kind).toBe("dux")
    expect(rows[rows.length - 1].kind).toBe("total")
  })

  it("dux_and_total_render_even_with_no_runtimes", () => {
    const rows = taskManagerRows([], [duxStat, totalStat], [], [])
    expect(rows.map((r) => r.kind)).toEqual(["dux", "total"])
  })

  it("groups_tabs_under_their_agent", () => {
    // The session-slot tab leads; extra tabs nest under it in `order`, never
    // reordered by a stat value.
    const sessions = [
      session({
        id: "s1",
        title: "fix-auth",
        tabs: [
          tab({ id: "s1", order: 0 }),
          tab({ id: "t3", order: 2, provider: "codex" }),
          tab({ id: "t2", order: 1, provider: "opencode" }),
        ],
      }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], [])
    const keys = rows.map((r) => r.key)
    expect(keys).toEqual(["dux", "tab:s1", "tab:t2", "tab:t3", "total"])
    // The session-slot tab is the group's lead row; extra tabs are nested.
    expect(rows.find((r) => r.key === "tab:s1")?.nested).toBe(false)
    expect(rows.find((r) => r.key === "tab:t2")?.nested).toBe(true)
  })

  it("orders_terminals_after_their_agents_tabs", () => {
    const sessions = [
      session({ id: "s1", tabs: [tab({ id: "s1" })] }),
    ]
    const terminals = [terminal({ id: "term-1", label: "dev server" })]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], terminals)
    expect(rows.map((r) => r.key)).toEqual([
      "dux",
      "tab:s1",
      "term:term-1",
      "total",
    ])
  })

  // The Task Manager's bulk confirmation counts EVERY terminal in the flat
  // collection, so a terminal the row walk fails to place would make the rows
  // and the count disagree about what "Stop all" is about to stop. Rows are
  // therefore emitted from the flat list itself: a terminal whose owner the walk
  // over sessions and projects never reaches is emitted at the END, still
  // stoppable, rather than dropped.
  it("emits_a_row_for_every_terminal_even_when_its_owner_is_not_walked", () => {
    const sessions = [session({ id: "s1", tabs: [tab({ id: "s1" })] })]
    const terminals = [
      terminal({ id: "term-known", owner: { kind: "session", session_id: "s1" } }),
      // An owner the walk never visits (here a session missing from the spine's
      // own list, which stands in for any owner this walk has no section for).
      terminal({ id: "term-orphan", owner: { kind: "session", session_id: "gone" } }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], terminals)
    expect(rows.map((r) => r.key)).toEqual([
      "dux",
      "tab:s1",
      "term:term-known",
      "term:term-orphan",
      "total",
    ])
    const orphan = rows.find((r) => r.key === "term:term-orphan")
    expect(orphan?.stoppable).toBe(true)
    expect(orphan?.targetId).toBe("term-orphan")
    // The count of terminal rows always equals the count of terminals, which is
    // the number the "Stop all" confirmation puts in front of the user.
    expect(rows.filter((r) => r.kind === "terminal")).toHaveLength(
      terminals.length,
    )
  })

  it("keeps_row_order_stable_regardless_of_stat_values", () => {
    // R7: rows must never sort by a stat, or they would reorder under the
    // cursor on every poll.
    const sessions = [
      session({ id: "s1", tabs: [tab({ id: "s1" }), tab({ id: "t2", order: 1 })] }),
    ]
    const hot = [
      duxStat,
      stat({ id: "s1", cpu_percent: 0.1 }),
      stat({ id: "t2", cpu_percent: 99 }),
      totalStat,
    ]
    const cold = [
      duxStat,
      stat({ id: "s1", cpu_percent: 99 }),
      stat({ id: "t2", cpu_percent: 0.1 }),
      totalStat,
    ]
    expect(taskManagerRows(sessions, hot, [], []).map((r) => r.key)).toEqual(
      taskManagerRows(sessions, cold, [], []).map((r) => r.key),
    )
  })

  it("carries_the_session_id_so_a_tab_row_can_be_stopped", () => {
    const sessions = [
      session({ id: "s1", tabs: [tab({ id: "s1" }), tab({ id: "t2", order: 1 })] }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], [])
    expect(rows.find((r) => r.key === "tab:t2")?.sessionId).toBe("s1")
  })

  it("reports_nothing_running_only_when_no_agents_or_terminals", () => {
    expect(taskManagerRows([], [duxStat, totalStat], [], []).some((r) => r.stoppable)).toBe(
      false,
    )
    const sessions = [session({ id: "s1" })]
    expect(
      taskManagerRows(sessions, [duxStat, totalStat], [], []).some((r) => r.stoppable),
    ).toBe(true)
  })

  it("lists_only_agents_with_a_live_or_dormant_tab_not_exited_ones", () => {
    // An exited/detached agent has no live tabs; it is not a running task, so
    // the Task Manager does not list it.
    const sessions = [
      session({
        id: "s1",
        status: "detached",
        tabs: [tab({ id: "s1", has_live_process: false })],
      }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], [])
    expect(rows.map((r) => r.kind)).toEqual(["dux", "total"])
  })

  it("lists_a_detached_agents_companion_terminal_because_detach_leaves_it_running", () => {
    // Detaching an agent deliberately leaves its companion terminals running:
    // only the agent TABS are gated on session status, never terminals. A
    // live terminal on a detached agent must stay visible and stoppable, or
    // it becomes an invisible, unstoppable resource drain.
    const sessions = [
      session({
        id: "s1",
        status: "detached",
        tabs: [tab({ id: "s1", has_live_process: false })],
      }),
    ]
    const terminals = [terminal({ id: "term-1", label: "dev server" })]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], terminals)
    expect(rows.map((r) => r.kind)).toEqual(["dux", "terminal", "total"])
    const term = rows.find((r) => r.key === "term:term-1")
    expect(term?.stoppable).toBe(true)
  })

  it("gives_each_same_provider_extra_tab_a_unique_meaningful_stop_label", () => {
    // Two extra tabs on the same provider is a supported configuration. Their
    // Stop control's accessible name must not collide, and must say more than
    // just the provider.
    const sessions = [
      session({
        id: "s1",
        title: "fix-auth",
        tabs: [
          tab({ id: "s1", order: 0 }),
          tab({ id: "t2", order: 1, provider: "claude" }),
          tab({ id: "t3", order: 2, provider: "claude" }),
        ],
      }),
    ]
    const rows = taskManagerRows(sessions, [duxStat, totalStat], [], [])
    const t2 = rows.find((r) => r.key === "tab:t2")
    const t3 = rows.find((r) => r.key === "tab:t3")
    expect(t2?.stopLabel).toBeTruthy()
    expect(t3?.stopLabel).toBeTruthy()
    expect(t2?.stopLabel).not.toBe(t3?.stopLabel)
    expect(t2?.stopLabel).toContain("fix-auth")
    expect(t3?.stopLabel).toContain("fix-auth")
  })
})

describe("taskManagerSummary", () => {
  it("is_null_when_nothing_is_running", () => {
    // Nothing to stop, so nothing to total: the footer's summary disappears
    // along with "Stop all…" in this state.
    const rows = taskManagerRows([], [duxStat, totalStat], [], [])
    expect(taskManagerSummary(rows)).toBeNull()
  })

  it("counts_running_rows_and_reads_the_total_straight_off_the_total_row", () => {
    const sessions = [
      session({ id: "s1", title: "fix-auth" }),
    ]
    const terminals = [terminal({ id: "term-1", label: "dev server" })]
    const rows = taskManagerRows(sessions, [
      duxStat,
      stat({ id: "s1", cpu_percent: 3.4 }),
      stat({ id: "term-1", cpu_percent: 1 }),
      stat({
        kind: "total",
        label: "TOTAL",
        process_count: 14,
        cpu_percent: 65.6,
        rss_bytes: 1_400_000_000,
      }),
    ], [], terminals)
    // Two stoppable rows: the agent tab and its terminal.
    expect(taskManagerSummary(rows)).toBe("2 running · 14 processes · 65.6% CPU · 1.3 GiB")
  })

  it("omits_the_total_figures_when_no_sample_has_landed_yet", () => {
    // Before the first poll response, the TOTAL row has no stats: the summary
    // still reports what is running from the spine, without inventing numbers.
    const sessions = [session({ id: "s1" })]
    const rows = taskManagerRows(sessions, [], [], [])
    expect(taskManagerSummary(rows)).toBe("1 running")
  })
})
