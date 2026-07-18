import { describe, expect, it } from "vitest"

import { matchesSessionQuery, matchesTerminalQuery, normalizeQuery } from "@/lib/agentSearch"
import type { SessionView, TerminalView } from "@/lib/types"

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    project_id: "p1",
    title: null,
    provider: "claude",
    branch_name: "main",
    initial_branch: "main",
    source_branch: "main",
    worktree_path: "/tmp/x",
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
    tabs: [],
    has_output: false,
    working: false,
    needs_attention: false,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function makeTerminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    label: "Terminal 1",
    has_output: false,
    foreground_cmd: null,
    ...over,
  } as TerminalView
}

describe("normalizeQuery", () => {
  it("trims and lowercases", () => {
    expect(normalizeQuery("  AuTh  ")).toBe("auth")
  })
})

describe("matchesSessionQuery", () => {
  const session = makeSession({
    id: "s1",
    title: "Auth refactor",
    branch_name: "feat/auth-v2",
    provider: "claude",
    tabs: [
      { id: "s1", provider: "claude", order: 0, working: false, needs_attention: false, has_output: false, has_live_process: true },
      { id: "t2", provider: "codex", order: 1, working: false, needs_attention: false, has_output: false, has_live_process: true },
    ],
  })

  it("matches everything for an empty query", () => {
    expect(matchesSessionQuery(session, "dux", "")).toBe(true)
    expect(matchesSessionQuery(session, "dux", "   ")).toBe(true)
  })

  it("matches on the display name", () => {
    expect(matchesSessionQuery(session, "dux", "refactor")).toBe(true)
  })

  it("falls back to the branch name when there is no title", () => {
    const noTitle = makeSession({ id: "s2", title: null, branch_name: "og-images" })
    expect(matchesSessionQuery(noTitle, "website", "og-im")).toBe(true)
  })

  it("matches on the project name", () => {
    expect(matchesSessionQuery(session, "dux", "du")).toBe(true)
  })

  it("matches on the branch", () => {
    expect(matchesSessionQuery(session, "dux", "auth-v2")).toBe(true)
  })

  it("matches on a tab's provider, not just the session provider", () => {
    expect(matchesSessionQuery(session, "dux", "codex")).toBe(true)
  })

  it("does not match unrelated text", () => {
    expect(matchesSessionQuery(session, "dux", "zzz")).toBe(false)
  })
})

describe("matchesTerminalQuery", () => {
  const terminal = makeTerminal({ id: "t1", label: "Terminal 2", foreground_cmd: "npm run dev" })

  it("matches on the running command", () => {
    expect(matchesTerminalQuery(terminal, "server-mode", "dux", "npm")).toBe(true)
  })

  it("matches on the owner label", () => {
    expect(matchesTerminalQuery(terminal, "server-mode", "dux", "server")).toBe(true)
  })

  it("matches on the project name", () => {
    expect(matchesTerminalQuery(terminal, "server-mode", "dux", "dux")).toBe(true)
  })

  it("does not match unrelated text", () => {
    expect(matchesTerminalQuery(terminal, "server-mode", "dux", "zzz")).toBe(false)
  })
})
