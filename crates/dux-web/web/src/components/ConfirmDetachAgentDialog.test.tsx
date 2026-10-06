// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Bootstrap } from "@/lib/bootstrapApi"
import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

const closeStopAgent = vi.fn()
const killSessionPty = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeStopAgent: () => closeStopAgent(),
    killSessionPty: (...args: unknown[]) => killSessionPty(...args),
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
const { ConfirmDetachAgentDialog } = await import("./ConfirmDetachAgentDialog")

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
    stopAgentTarget: null,
    spine: null,
    bootstrap: null,
    ...over,
  } as DuxState
}

beforeEach(() => {
  closeStopAgent.mockClear()
  killSessionPty.mockClear()
})

afterEach(cleanup)

describe("ConfirmDetachAgentDialog", () => {
  it("stays shut with no target", () => {
    seed({ stopAgentTarget: null })
    render(<ConfirmDetachAgentDialog />)
    expect(screen.queryByText("Detach agent?")).toBeNull()
  })

  it("says what will happen and quotes the configured wait", async () => {
    seed({
      stopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
      bootstrap: { shutdown_timeout_seconds: 45 } as Bootstrap,
    } as Partial<DuxState>)
    render(<ConfirmDetachAgentDialog />)

    expect(await screen.findByText("Detach agent?")).toBeTruthy()
    const body = await screen.findByText(/dux will ask/)
    // The agent's name is a chip, and the chip replaces the quotes.
    expect(within(body).getByText("fix-auth", { selector: "code" })).toBeTruthy()
    expect(body.textContent).toContain("dux will ask fix-auth to shut down")
    expect(body.textContent).toContain("wait up to 45 seconds")
    expect(body.textContent).toContain("stays in the list as Detached")
  })

  // The number is a setting, so it must never be baked into the copy.
  it("quotes the number the server projected, not a fixed one", async () => {
    seed({
      stopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
      bootstrap: { shutdown_timeout_seconds: 5 } as Bootstrap,
    } as Partial<DuxState>)
    render(<ConfirmDetachAgentDialog />)

    const body = await screen.findByText(/dux will ask/)
    expect(body.textContent).toContain("wait up to 5 seconds")
    expect(body.textContent).not.toContain("30 seconds")
  })

  it("names how many tabs stop together when the agent has more than one", async () => {
    seed({
      stopAgentTarget: "s1",
      spine: {
        sessions: [
          session({
            id: "s1",
            title: "fix-auth",
            tabs: [tab({ id: "s1" }), tab({ id: "b2", provider: "codex" })],
          }),
        ],
      },
      bootstrap: { shutdown_timeout_seconds: 30 } as Bootstrap,
    } as Partial<DuxState>)
    render(<ConfirmDetachAgentDialog />)

    const body = await screen.findByText(/dux will ask/)
    expect(body.textContent).toContain("All 2 running tabs stop together.")
  })

  // Refused while somebody else is attached, it stays open naming them and
  // detaches only through Detach anyway, which goes ahead over them.
  it("detaches on confirm and closes itself, over who is attached only once they are named", async () => {
    seed({
      stopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
    } as Partial<DuxState>)
    killSessionPty.mockResolvedValueOnce([{
        surface: "browser",
        device: "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
        address: "10.0.0.7",
        verified: false,
        driving: true,
        target: { kind: "tab", id: "s1-slot", agent: "s1" },
      }])
    render(<ConfirmDetachAgentDialog />)

    fireEvent.click(await screen.findByText("Detach"))
    expect(killSessionPty).toHaveBeenCalledWith("s1", false, false)
    const override = await screen.findByRole("button", { name: "Detach anyway" })
    expect(screen.getByText("Firefox on Linux").tagName).toBe("CODE")
    expect(closeStopAgent).not.toHaveBeenCalled()

    killSessionPty.mockResolvedValueOnce(null)
    fireEvent.click(override)
    expect(killSessionPty).toHaveBeenLastCalledWith("s1", false, true)
    await vi.waitFor(() => expect(closeStopAgent).toHaveBeenCalled())
  })

  it("cancels without detaching, and Cancel is the default focus", async () => {
    seed({
      stopAgentTarget: "s1",
      spine: { sessions: [session({ id: "s1", title: "fix-auth" })] },
    } as Partial<DuxState>)
    render(<ConfirmDetachAgentDialog />)

    const cancel = await screen.findByText("Cancel")
    expect(document.activeElement).toBe(cancel.closest("button"))
    fireEvent.click(cancel)
    expect(killSessionPty).not.toHaveBeenCalled()
    expect(closeStopAgent).toHaveBeenCalled()
  })

  // Target-keyed dialogs close themselves when their entity leaves the live view
  // model, so a concurrent delete cannot leave a dialog pointing at nothing.
  it("closes itself when the agent vanishes from the view model", () => {
    seed({
      stopAgentTarget: "s1",
      spine: { sessions: [] },
    } as Partial<DuxState>)
    render(<ConfirmDetachAgentDialog />)
    expect(screen.queryByText("Detach agent?")).toBeNull()
    expect(closeStopAgent).toHaveBeenCalled()
  })
})
