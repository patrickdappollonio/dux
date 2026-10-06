// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { TerminalView } from "@/lib/types"

// Override `useDux` so the dialog reads our seeded spine + delete target, and spy
// `deleteTerminal`, while the other real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, deleteTerminal: vi.fn() }
})

// The real store boots on import (localStorage + bootstrap fetch). jsdom doesn't
// provide those as bare globals, so stub them before the component loads.
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
const { ConfirmDeleteTerminalDialog } = await import(
  "./ConfirmDeleteTerminalDialog"
)
const store = await import("@/lib/store")

function term(overrides: Partial<TerminalView>): TerminalView {
  return {
    id: "term-1",
    owner: { kind: "session", session_id: "s1" },
    label: "Terminal 1",
    has_output: true,
    foreground_cmd: null,
    ...overrides,
  } as TerminalView
}

function seed(terminal: TerminalView) {
  mockState = {
    deleteTerminalTarget: terminal.id,
    spine: {
      sessions: [{ id: "s1" }],
      projects: [{ id: "p1" }],
      terminals: [terminal],
    },
  } as unknown as DuxState
}

// Seed a PROJECT-owned terminal: it carries a project owner, and no session
// owns it.
function seedProjectTerminal(terminal: TerminalView) {
  mockState = {
    deleteTerminalTarget: terminal.id,
    spine: {
      sessions: [{ id: "s1" }],
      projects: [{ id: "p1" }],
      terminals: [
        { ...terminal, owner: { kind: "project", project_id: "p1" } },
      ],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmDeleteTerminalDialog", () => {
  it("warns that the running app will be killed when one is detected", () => {
    seed(term({ foreground_cmd: "vim" }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(
      screen.getByText(/is running in this terminal and will be killed/),
    ).toBeTruthy()
    expect(screen.getByText("vim")).toBeTruthy()
  })

  it("opens and STAYS open for a project-owned terminal", () => {
    // A session-only owner scan resolves a project terminal to `undefined`,
    // and the vanished-target guard would close the dialog the instant it
    // opened.
    seedProjectTerminal(term({ id: "pt-1", label: "Terminal 3" }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(screen.getByRole("heading").textContent).toBe("Close Terminal 3?")
    expect(screen.getByText("Terminal 3").tagName).toBe("CODE")
    expect(screen.getByText("Close terminal")).toBeTruthy()
  })

  it("warns about a project terminal's running app too", () => {
    seedProjectTerminal(term({ id: "pt-1", foreground_cmd: "htop" }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(
      screen.getByText(/is running in this terminal and will be killed/),
    ).toBeTruthy()
    expect(screen.getByText("htop")).toBeTruthy()
  })

  it("shows no kill warning when only the shell is running", () => {
    // The bare shell is not an app worth warning about, so an idle terminal
    // confirms with just the title and no "will be killed" line.
    seed(term({ foreground_cmd: null }))
    render(<ConfirmDeleteTerminalDialog />)
    expect(screen.getByRole("heading").textContent).toBe("Close Terminal 1?")
    expect(screen.getByText("Terminal 1").tagName).toBe("CODE")
    expect(screen.queryByText(/will be killed/)).toBeNull()
  })

  // Refused because somebody else is attached, the dialog names them and only
  // Close terminal anyway asks to go ahead over them.
  it("names who is attached when the close is refused, and closes over them only through the override", async () => {
    seedProjectTerminal(term({}))
    const deleteTerminal = vi.mocked(store.deleteTerminal)
    deleteTerminal.mockResolvedValueOnce([
      {
        surface: "browser",
        device: null,
        address: "192.0.2.7",
        verified: false,
        driving: false,
        target: { kind: "terminal", id: "term-1" },
        key: "k1",
      },
    ])
    render(<ConfirmDeleteTerminalDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Close terminal" }))
    expect(deleteTerminal).toHaveBeenCalledWith("term-1", null)
    const override = await screen.findByRole("button", {
      name: "Close terminal anyway",
    })
    expect(
      screen.getByText(/a browser at 192\.0\.2\.7 \(unverified\), watching terminal/),
    ).toBeTruthy()

    deleteTerminal.mockResolvedValueOnce(null)
    fireEvent.click(override)
    expect(deleteTerminal).toHaveBeenLastCalledWith("term-1", ["k1"])
  })
})
