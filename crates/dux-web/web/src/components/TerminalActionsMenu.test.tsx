// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { TerminalOwnerRef } from "@/lib/store"

// THE TERMINAL'S OWN ACTIONS, on their own. The merged pane menu's suite covers
// where these rows appear; what is pinned here is what they DO and what they
// are about, which is the half that is the same wherever they are rendered.

const openEditorMock = vi.fn()
const openDeleteTerminalMock = vi.fn()
// The workspace lists the terminal unless a test says it has just been created.
let listedTerminals = ["t7"]
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => ({
      ...actual.getSnapshot(),
      spine: { sessions: [], terminals: listedTerminals.map((id) => ({ id })) },
    }),
    openEditor: openEditorMock,
    openDeleteTerminal: openDeleteTerminalMock,
  }
})

function installStubs() {
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
installStubs()

const { TerminalActionsMenu } = await import("./TerminalActionsMenu")
const { editorRootForTarget } = await import("@/lib/editorRoot")
const { standaloneEditorHash } = await import("@/lib/store")
const { agentRowKey, registerPaneCover, resetPaneCovers } = await import("@/lib/paneCover")
const {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} = await import("./ui/dropdown-menu")

afterEach(() => {
  cleanup()
  listedTerminals = ["t7"]
  openEditorMock.mockClear()
  openDeleteTerminalMock.mockClear()
  resetPaneCovers()
  vi.unstubAllGlobals()
})

async function open(owner: TerminalOwnerRef) {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <TerminalActionsMenu terminalId="t7" owner={owner} />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  await screen.findByRole("menu")
}

function item(text: string): HTMLElement {
  return screen
    .getAllByRole("menuitem")
    .find((el) => el.textContent?.includes(text))!
}

describe("the terminal's own actions", () => {
  it("sends the in-app editor to the root the target resolves to", async () => {
    const owner = { kind: "standalone" as const }
    await open(owner)
    fireEvent.click(item("Open editor here"))
    expect(openEditorMock).toHaveBeenCalledWith(
      editorRootForTarget({ kind: "terminal", terminalId: "t7", owner }),
    )
  })

  // A SESSION-OWNED terminal resolves to its AGENT's editor: same worktree, and
  // the agent's editor is the one with the git surface. The helper owns that
  // rule; this pins that these rows go through it rather than around it.
  it("sends an agent's terminal to the agent's editor", async () => {
    const owner = { kind: "session" as const, sessionId: "s1" }
    await open(owner)
    const href = item("Open editor in new tab")
      .closest("a")
      ?.getAttribute("href")
    expect(href).toBe(
      standaloneEditorHash(
        editorRootForTarget({ kind: "terminal", terminalId: "t7", owner }),
      ),
    )
  })

  // A real anchor, so a long-press keeps its native open-in-new-tab.
  it("opens the new-tab editor through a real link", async () => {
    await open({ kind: "standalone" })
    const anchor = item("Open editor in new tab").closest("a")
    expect(anchor?.getAttribute("target")).toBe("_blank")
    expect(anchor?.getAttribute("rel")).toBe("opener")
  })

  // Close routes into the existing confirm target rather than closing anything:
  // a destructive action always gets its dialog.
  it("routes Close through the confirm dialog's target", async () => {
    await open({ kind: "standalone" })
    fireEvent.click(item("Close…"))
    expect(openDeleteTerminalMock).toHaveBeenCalledWith("t7")
  })

  // A terminal just created is on screen before the workspace lists it, and
  // its confirm has nothing to confirm until then: no Close… that does nothing.
  it("offers no Close… for a terminal the workspace does not list yet", async () => {
    listedTerminals = []
    await open({ kind: "session", sessionId: "s1" })
    expect(screen.getAllByRole("menuitem").some((el) => el.textContent?.includes("Close"))).toBe(false)
  })

  // The in-app overlay is desktop-only, so on a phone its row would be a dead
  // no-op; it is hidden rather than offered.
  it("hides the in-app editor row on a phone", async () => {
    await open({ kind: "standalone" })
    expect(item("Open editor here").className).toContain("max-md:hidden")
  })

  // The heading is part of the group, so a terminal's own menu and the group in
  // its agent's menu cannot say different things, and Close… sits straight under
  // the editor rows with no rule between them.
  it("heads the group and keeps Close… in it without a separator", async () => {
    await open({ kind: "standalone" })
    expect(screen.getByText("Terminal")).toBeTruthy()
    expect(screen.queryByRole("separator")).toBeNull()
  })

  // Closing changes the terminal's row, so while this device does not drive
  // it, a card being up with nobody driving included, Close… is disabled and
  // the menu says why.
  it("disables Close… on a terminal row nobody drives, saying why", async () => {
    registerPaneCover("t7", "card", { row: "terminal:t7", driver: "nobody" })
    await open({ kind: "standalone" })
    expect(item("Close…").getAttribute("aria-disabled")).toBe("true")
    expect(screen.getByText(/Nobody is driving this terminal/)).toBeTruthy()
  })

  // An agent's terminal belongs to the agent's row, whose own menu already
  // says why its changes are disabled.
  it("disables an agent terminal's Close… while another device drives the agent", async () => {
    registerPaneCover("s1", "card", { row: agentRowKey("s1"), driver: "other" })
    await open({ kind: "session", sessionId: "s1" })
    expect(item("Close…").getAttribute("aria-disabled")).toBe("true")
  })
})
