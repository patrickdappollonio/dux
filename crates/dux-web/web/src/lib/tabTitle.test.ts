import { describe, expect, it } from "vitest"

import { tabPosition, tabTitle, type TabPositionInput } from "./tabTitle"
import type { Spine } from "./workspaceApi"

const INSTANCE = "dux @ main pc"

describe("tabTitle", () => {
  it("is the instance title alone with nothing selected", () => {
    expect(tabTitle({ kind: "home" }, INSTANCE, 0)).toBe(INSTANCE)
  })

  it("names an agent, then a docked terminal or a later tab before it", () => {
    expect(
      tabTitle({ kind: "agent", agent: "fix-login", tab: null, terminal: null }, INSTANCE, 0),
    ).toBe("fix-login — dux @ main pc")
    expect(
      tabTitle({ kind: "agent", agent: "fix-login", tab: null, terminal: "Terminal 2" }, INSTANCE, 0),
    ).toBe("Terminal 2 · fix-login — dux @ main pc")
    expect(
      tabTitle({ kind: "agent", agent: "fix-login", tab: "codex", terminal: null }, INSTANCE, 0),
    ).toBe("codex · fix-login — dux @ main pc")
  })

  it("names a project or standalone terminal", () => {
    expect(tabTitle({ kind: "terminal", terminal: "Terminal 1" }, INSTANCE, 0)).toBe(
      "Terminal 1 — dux @ main pc",
    )
  })

  it("puts the file first in the editor and marks unsaved changes", () => {
    const editor = { kind: "editor" as const, root: "fix-login", file: "main.rs", dirty: false }
    expect(tabTitle(editor, INSTANCE, 0)).toBe("main.rs · fix-login · Editor — dux @ main pc")
    expect(tabTitle({ ...editor, dirty: true }, INSTANCE, 0)).toBe(
      "● main.rs · fix-login · Editor — dux @ main pc",
    )
    expect(tabTitle({ ...editor, file: null }, INSTANCE, 0)).toBe(
      "fix-login · Editor — dux @ main pc",
    )
  })

  it("says what is missing on a not-found screen", () => {
    expect(tabTitle({ kind: "not-found", what: "agent" }, INSTANCE, 0)).toBe(
      "Agent not found — dux @ main pc",
    )
    expect(tabTitle({ kind: "not-found", what: "terminal" }, INSTANCE, 0)).toBe(
      "Terminal not found — dux @ main pc",
    )
  })

  it("keeps the attention prefix at the very front", () => {
    expect(
      tabTitle({ kind: "agent", agent: "fix-login", tab: null, terminal: null }, INSTANCE, 3),
    ).toBe("(3) fix-login — dux @ main pc")
    expect(tabTitle({ kind: "home" }, INSTANCE, 1)).toBe("(1) dux @ main pc")
  })

  it("collapses whitespace inside a name, since a tab title stops at a newline", () => {
    expect(tabTitle({ kind: "terminal", terminal: "build\nwatch " }, INSTANCE, 0)).toBe(
      "build watch — dux @ main pc",
    )
  })
})

const spine = {
  projects: [],
  sessions: [
    {
      id: "s1",
      title: "fix-login",
      workspace: { kind: "managed", branch_name: "fix-login" },
      slot_tab_id: "t1",
      tabs: [
        { id: "t1", provider: "claude", created_at_ms: 1 },
        { id: "t2", provider: "codex", created_at_ms: 2 },
      ],
    },
  ],
  terminals: [
    { id: "d1", label: "Terminal 1", owner: { kind: "session", session_id: "s1" } },
    { id: "p1", label: "Terminal 2", owner: { kind: "project", project_id: "p" } },
  ],
} as unknown as Spine

function input(patch: Partial<TabPositionInput>): TabPositionInput {
  return {
    spine,
    selectedTarget: null,
    routeNotFound: null,
    editorRoute: null,
    editorTabs: {},
    pendingSlotTab: {},
    ...patch,
  }
}

describe("tabPosition", () => {
  it("leaves the first tab out and names a later one", () => {
    expect(
      tabPosition(input({ selectedTarget: { kind: "agent", sessionId: "s1", tabId: "t1" } })),
    ).toEqual({ kind: "agent", agent: "fix-login", tab: null, terminal: null })
    expect(
      tabPosition(input({ selectedTarget: { kind: "agent", sessionId: "s1", tabId: "t2" } })),
    ).toEqual({ kind: "agent", agent: "fix-login", tab: "codex", terminal: null })
  })

  it("names a docked terminal under its agent and a project terminal alone", () => {
    expect(
      tabPosition(
        input({
          selectedTarget: { kind: "terminal", terminalId: "d1", owner: { kind: "session" } },
        }),
      ),
    ).toEqual({ kind: "agent", agent: "fix-login", tab: null, terminal: "Terminal 1" })
    expect(
      tabPosition(
        input({
          selectedTarget: { kind: "terminal", terminalId: "p1", owner: { kind: "project" } },
        }),
      ),
    ).toEqual({ kind: "terminal", terminal: "Terminal 2" })
  })

  it("reads the editor's active buffer and its dirty flag over the selection", () => {
    const position = tabPosition(
      input({
        selectedTarget: { kind: "agent", sessionId: "s1", tabId: "t1" },
        editorRoute: { root: { kind: "agent", sessionId: "s1" }, path: "src/old.rs" },
        editorTabs: {
          "agent:s1": {
            activeId: "e2",
            tabs: [
              { id: "e1", path: "src/old.rs", mode: "file", preview: false, dirty: false },
              { id: "e2", path: "src/lib/main.rs", mode: "diff", preview: false, dirty: true },
            ],
          },
        },
      }),
    )
    expect(position).toEqual({ kind: "editor", root: "fix-login", file: "main.rs", dirty: true })
  })

  it("is home until the spine names what is selected", () => {
    expect(
      tabPosition(
        input({ spine: null, selectedTarget: { kind: "agent", sessionId: "s1", tabId: "t1" } }),
      ),
    ).toEqual({ kind: "home" })
  })
})
