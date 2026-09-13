// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

const closeForceStopAgent = vi.fn()
// Both arguments, because the FORCE flag is the whole difference between this
// dialog and the polite one beside it.
const killSessionPty = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeForceStopAgent: () => closeForceStopAgent(),
    killSessionPty: (s: string, force?: boolean) => killSessionPty(s, force),
  }
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
}
installBootStubs()
const { ConfirmForceStopAgentDialog } = await import(
  "./ConfirmForceStopAgentDialog"
)

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
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "feat",
      initial_branch: "feat",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/wt",
    },
    title: null,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [tab({ id: over.id })],
    has_output: false,
    working: false,
    needs_attention: false,
    slot_tab_id: over.id,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  } as SessionView
}

function seed(over: Partial<DuxState>) {
  mockState = {
    forceStopAgentTarget: null,
    spine: null,
    bootstrap: null,
    ...over,
  } as DuxState
}

beforeEach(() => {
  closeForceStopAgent.mockClear()
  killSessionPty.mockClear()
})

afterEach(cleanup)

describe("ConfirmForceStopAgentDialog", () => {
  it("stays shut with no target", () => {
    seed({ forceStopAgentTarget: null })
    render(<ConfirmForceStopAgentDialog />)
    expect(screen.queryByText("Force stop agent?")).toBeNull()
  })

  it("names the agent and promises no shutdown wait", async () => {
    seed({
      forceStopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
    } as Partial<DuxState>)
    render(<ConfirmForceStopAgentDialog />)

    expect(await screen.findByText("Force stop agent?")).toBeTruthy()
    const body = await screen.findByText(/dux will stop/)
    expect(body.textContent).toContain('"fix-auth"')
    expect(body.textContent).toContain("immediately, with no shutdown wait")
    expect(body.textContent).toContain("stays in the list as Detached")
    // The polite dialog's promise must not leak into this one.
    expect(body.textContent).not.toContain("wait up to")
  })

  it("forces on confirm and closes itself", async () => {
    seed({
      forceStopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
    } as Partial<DuxState>)
    render(<ConfirmForceStopAgentDialog />)

    fireEvent.click(await screen.findByText("Force stop"))
    expect(killSessionPty).toHaveBeenCalledWith("s1", true)
    expect(closeForceStopAgent).toHaveBeenCalled()
  })

  it("cancels without stopping, and Cancel is the default focus", async () => {
    seed({
      forceStopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
    } as Partial<DuxState>)
    render(<ConfirmForceStopAgentDialog />)

    const cancel = await screen.findByText("Cancel")
    expect(document.activeElement).toBe(cancel.closest("button"))
    fireEvent.click(cancel)
    expect(killSessionPty).not.toHaveBeenCalled()
    expect(closeForceStopAgent).toHaveBeenCalled()
  })

  // Target-keyed dialogs close themselves when their entity leaves the live view
  // model, so a concurrent delete cannot leave a dialog pointing at nothing.
  it("closes itself when the agent vanishes from the view model", () => {
    seed({
      forceStopAgentTarget: "s1",
      spine: { sessions: [] },
    } as Partial<DuxState>)
    render(<ConfirmForceStopAgentDialog />)
    expect(screen.queryByText("Force stop agent?")).toBeNull()
    expect(closeForceStopAgent).toHaveBeenCalled()
  })
})
