// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { RecreateWire } from "@/lib/agentWorkspace"
import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeRecreateWorkingCopy: vi.fn(),
    recreateWorkingCopy: vi.fn(),
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
const { ConfirmRecreateWorkingCopyDialog } = await import(
  "./ConfirmRecreateWorkingCopyDialog"
)
const store = await import("@/lib/store")
const closeRecreateWorkingCopy = vi.mocked(store.closeRecreateWorkingCopy)
const recreateWorkingCopy = vi.mocked(store.recreateWorkingCopy)

/** What the server says about the running tabs: Claude alone by default. */
const claudeOnly: RecreateWire = {
  running: { follows: ["claude"], restartable: [], left_running: [] },
  restart_offer: [],
}

function session(
  missing: boolean,
  recreate: RecreateWire | null = claudeOnly,
): SessionView {
  return {
    id: "s1",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "feat",
      initial_branch: "feat",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/home/someone/.config/dux/worktrees/repo/feat",
      worktree_label: "~/.config/dux/worktrees/repo/feat",
      worktree_missing: missing,
      quiet_reason: missing ? "The working copy no longer exists on disk." : "",
      // The fixture agent runs claude, which resumes per directory.
      conversation_resumes: true,
      recreate: missing ? recreate : null,
    },
    title: null,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    needs_attention: false,
    slot_tab_id: "s1",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  } as unknown as SessionView
}

function seed(target: string | null, sessions: SessionView[] = []) {
  mockState = {
    recreateWorkingCopyTarget: target,
    spine: { sessions, projects: [] },
  } as unknown as DuxState
}

beforeEach(() => {
  closeRecreateWorkingCopy.mockClear()
  recreateWorkingCopy.mockReset()
  recreateWorkingCopy.mockResolvedValue(null)
})

afterEach(cleanup)

describe("ConfirmRecreateWorkingCopyDialog", () => {
  it("stays shut with no target", () => {
    seed(null)
    render(<ConfirmRecreateWorkingCopyDialog />)
    expect(screen.queryByText("Recreate working copy?")).toBeNull()
  })

  // The three things a user must know before agreeing: where it lands, that the
  // code changes are gone, and what a tab still running makes of the new folder.
  it("says what is lost, what may survive, and what keeps running", () => {
    seed("s1", [session(true)])
    render(<ConfirmRecreateWorkingCopyDialog />)

    const element = screen.getByText(/Recreate the working copy/)
    const body = element.textContent ?? ""
    // The path, every branch and the running provider are chips, and the chips
    // replace the quotes.
    const chips = [...element.querySelectorAll("code")].map((c) => c.textContent)
    expect(chips).toEqual([
      "~/.config/dux/worktrees/repo/feat",
      "feat",
      "origin/feat",
      "main",
      "Claude",
    ])
    expect(body).toContain("branch feat still exists")
    expect(body).toContain("from main,")
    expect(body).not.toContain('"')
    expect(body).toContain("are gone either way")
    expect(body).toContain("same path")
    expect(body).toContain(
      "A running Claude tab keeps working in the recreated copy by itself.",
    )
    // Claude follows the folder, so there is nothing to restart.
    expect(screen.queryByRole("checkbox")).toBeNull()
  })

  // Codex cannot follow the folder but can pick its conversation back up, so
  // the restart is offered, ticked; Copilot would lose its conversation and is
  // named as left running instead.
  it("offers to restart a Codex tab, ticked, and says Copilot is left running", () => {
    seed("s1", [
      session(true, {
        running: {
          follows: [],
          restartable: ["codex"],
          left_running: ["copilot"],
        },
        restart_offer: ["codex"],
      }),
    ])
    render(<ConfirmRecreateWorkingCopyDialog />)

    const body = screen.getByText(/Recreate the working copy/).textContent ?? ""
    expect(body).toContain(
      "A running Codex tab cannot follow the folder by itself; restarting it " +
        "continues its conversation in the recreated copy.",
    )
    expect(body).toContain(
      "A running Copilot tab is left running, because restarting it would " +
        "lose its conversation",
    )
    const box = screen.getByRole("checkbox")
    expect(box.getAttribute("aria-checked")).toBe("true")
    const label = screen.getByText(/so it continues in the new copy/)
    expect(label.textContent).toBe(
      "Restart the Codex tab so it continues in the new copy",
    )
    expect(label.querySelector("code")?.textContent).toBe("Codex")

    fireEvent.click(screen.getByRole("button", { name: "Recreate" }))
    expect(recreateWorkingCopy).toHaveBeenCalledWith("s1", true, null)
  })

  it("sends no restart once the box is unticked", async () => {
    seed("s1", [
      session(true, {
        running: { follows: [], restartable: ["codex"], left_running: [] },
        restart_offer: ["codex"],
      }),
    ])
    render(<ConfirmRecreateWorkingCopyDialog />)

    fireEvent.click(screen.getByRole("checkbox"))
    expect(screen.getByRole("checkbox").getAttribute("aria-checked")).toBe(
      "false",
    )
    fireEvent.click(screen.getByRole("button", { name: "Recreate" }))
    expect(recreateWorkingCopy).toHaveBeenCalledWith("s1", false, null)
    await vi.waitFor(() => expect(closeRecreateWorkingCopy).toHaveBeenCalled())
  })

  // The restart stops the tab, so it is guarded like a stop: refused while
  // somebody else is attached, the dialog stays open naming them, and only
  // Recreate anyway goes ahead over exactly those it named.
  it("names who is attached and goes ahead only over them", async () => {
    recreateWorkingCopy.mockResolvedValueOnce([
      {
        surface: "browser",
        device:
          "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
        address: "10.0.0.7",
        verified: false,
        driving: true,
        target: { kind: "tab", id: "s1", agent: "s1" },
        key: "k1",
      },
    ])
    seed("s1", [
      session(true, {
        running: { follows: [], restartable: ["codex"], left_running: [] },
        restart_offer: ["codex"],
      }),
    ])
    render(<ConfirmRecreateWorkingCopyDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Recreate" }))
    const override = await screen.findByRole("button", {
      name: "Recreate anyway",
    })
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Cancel" }),
    )
    expect(screen.getByText("Firefox on Linux").tagName).toBe("CODE")
    expect(closeRecreateWorkingCopy).not.toHaveBeenCalled()

    fireEvent.click(override)
    expect(recreateWorkingCopy).toHaveBeenLastCalledWith("s1", true, ["k1"])
    await vi.waitFor(() => expect(closeRecreateWorkingCopy).toHaveBeenCalled())
  })

  it("recreates on confirm and closes itself", async () => {
    seed("s1", [session(true)])
    render(<ConfirmRecreateWorkingCopyDialog />)

    fireEvent.click(screen.getByText("Recreate"))
    expect(recreateWorkingCopy).toHaveBeenCalledWith("s1", false, null)
    await vi.waitFor(() => expect(closeRecreateWorkingCopy).toHaveBeenCalled())
  })

  it("cancels without acting, and Cancel is the default focus", () => {
    seed("s1", [session(true)])
    render(<ConfirmRecreateWorkingCopyDialog />)

    expect(document.activeElement?.textContent).toBe("Cancel")
    fireEvent.click(screen.getByText("Cancel"))
    expect(recreateWorkingCopy).not.toHaveBeenCalled()
    expect(closeRecreateWorkingCopy).toHaveBeenCalled()
  })

  // A working copy that came back on its own counts as the target vanishing:
  // the menu entry is gone by then and this dialog has nothing left to do.
  it("closes itself when the working copy comes back", () => {
    seed("s1", [session(false)])
    render(<ConfirmRecreateWorkingCopyDialog />)

    expect(screen.queryByText("Recreate working copy?")).toBeNull()
    expect(closeRecreateWorkingCopy).toHaveBeenCalled()
  })
})
