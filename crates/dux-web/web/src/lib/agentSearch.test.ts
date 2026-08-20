import { describe, expect, it } from "vitest"

import {
  agentSearchLocation,
  matchCharRange,
  matchesSessionQuery,
  matchesTerminalQuery,
  normalizeQuery,
} from "@/lib/agentSearch"
import type { SessionView, TerminalView } from "@/lib/types"

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "main",
      initial_branch: "main",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/tmp/x",
    },
    title: null,
    provider: "claude",
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
    workspace: {
      kind: "managed",
      project_id: "",
      branch_name: "feat/auth-v2",
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: "",
    },
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
    const noTitle = makeSession({
      id: "s2",
      title: null,
      workspace: {
        kind: "managed",
        project_id: "",
        branch_name: "og-images",
        initial_branch: "",
        branch_provenance: "created",
        source_branch: "",
        worktree_path: "",
      },
    })
    expect(matchesSessionQuery(noTitle, "website", "og-im")).toBe(true)
  })

  it("matches on the project name", () => {
    expect(matchesSessionQuery(session, "dux", "du")).toBe(true)
  })

  it("matches on the branch", () => {
    expect(matchesSessionQuery(session, "dux", "auth-v2")).toBe(true)
  })

  it("does NOT match on provider names (session or tab)", () => {
    // Provider names ("claude", "codex", ...) are far too generic as search
    // terms; a provider-only hit would surface almost every agent. Pins the
    // removal, in lockstep with the Rust vector `provider_names_do_not_match`.
    expect(matchesSessionQuery(session, "dux", "codex")).toBe(false)
    expect(matchesSessionQuery(session, "dux", "claude")).toBe(false)
  })

  it("does not match unrelated text", () => {
    expect(matchesSessionQuery(session, "dux", "zzz")).toBe(false)
  })

  // A standalone agent's row shows its FOLDER where an ordinary agent shows its
  // project, so that is the field the query matches. Its label is the name dux
  // derived from the folder, and it has no branch to fall back to. In lockstep
  // with the Rust vectors for `agent_search_location`.
  it("matches a standalone agent on its folder and its name", () => {
    const standalone = makeSession({
      id: "sa1",
      title: "My Notes",
      workspace: {
        kind: "folder",
        folder_path: "/home/someone/My Notes",
        folder_label: "~/My Notes",
        repo_status: "no_repo",
        quiet_reason: "This folder has no git repository.",
      },
    })
    // The location the row shows for this agent, resolved the way the row
    // resolves it, so the test cannot match a field the row never displays.
    const location = agentSearchLocation(standalone, () => "unused-project")
    expect(location).toBe("~/My Notes")
    expect(matchesSessionQuery(standalone, location, "notes")).toBe(true)
    expect(matchesSessionQuery(standalone, location, "~/my")).toBe(true)
    // It has no branch and no project, so neither is a way to reach it.
    expect(matchesSessionQuery(standalone, location, "unused-project")).toBe(
      false,
    )
    expect(matchesSessionQuery(standalone, location, "zzz")).toBe(false)
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

// The highlight range helper, the TS twin of dux-core's `match_char_range`
// (shared vectors). CODE-POINT indices, never UTF-16 units or bytes, so a
// label with emoji or CJK highlights the right characters.
describe("matchCharRange", () => {
  it("finds a case-insensitive hit in code-point indices", () => {
    expect(matchCharRange("API-Refactor", "refactor")).toEqual({ start: 4, end: 12 })
    expect(matchCharRange("feature/login", "LOGIN")).toEqual({ start: 8, end: 13 })
    expect(matchCharRange("abc", "abc")).toEqual({ start: 0, end: 3 })
  })

  it("returns null for no hit or an empty/whitespace query", () => {
    expect(matchCharRange("api", "zzz")).toBeNull()
    expect(matchCharRange("api", "")).toBeNull()
    expect(matchCharRange("api", "   ")).toBeNull()
  })

  it("counts code points, not UTF-16 units, for astral-plane labels", () => {
    // The duck emoji is ONE code point (two UTF-16 units); "duck" starts at
    // code-point index 2 (emoji + space).
    expect(matchCharRange("🦆 duck", "duck")).toEqual({ start: 2, end: 6 })
    expect(matchCharRange("höhe-fix", "fix")).toEqual({ start: 5, end: 8 })
    expect(matchCharRange("日本語テスト", "語テ")).toEqual({
      start: 2,
      end: 4,
    })
  })
})
