// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The project-scoped forms an agent's or a project's menu opens. On a phone each
// covers the screen with its own close button, and under a finger none focuses
// a text field by itself, because that raises the keyboard over the form.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeAgentEnv: vi.fn(),
    closeAgentStartupCommand: vi.fn(),
    closeProjectSettings: vi.fn(),
    updateProjectSettings: vi.fn(),
  }
})

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

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
// The pointer is asked through `matchMedia`, which jsdom lacks.
function coarsePointer() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query === "(pointer: coarse)",
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
}

installBootStubs()
const { AgentEnvDialog } = await import("./AgentEnvDialog")
const { AgentStartupCommandDialog } = await import("./AgentStartupCommandDialog")
const { ProjectSettingsDialog } = await import("./ProjectSettingsDialog")
const store = await import("@/lib/store")

const session = {
  id: "s1",
  title: "quacky-mallard",
  workspace: {
    kind: "managed",
    project_id: "p1",
    branch_name: "dux/s1",
    initial_branch: "dux/s1",
    branch_provenance: "created",
    source_branch: "",
    worktree_path: "",
  },
}
// In byte order, as the server sends it, which is not the order the dialogs
// write the text in: an untouched form must still have nothing to save.
const project = {
  id: "p1",
  name: "acme",
  env: { PATH: "/usr/bin", http_proxy: "http://proxy" },
  startup_command: null,
  explicit_default_provider: null,
  auto_reopen_agents: null,
}

beforeEach(() => {
  installBootStubs()
  mockState = {
    spine: { sessions: [session], projects: [project] },
    bootstrap: { available_providers: [] },
    agentEnvTarget: "s1",
    agentStartupCommandTarget: "s1",
    projectSettingsTarget: "p1",
  } as unknown as DuxState
})
afterEach(() => {
  cleanup()
  phone = false
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe.each([
  ["the agent's environment", AgentEnvDialog, "KEY=VALUE"],
  ["the agent's startup command", AgentStartupCommandDialog, "npm run dev"],
])("%s", (_name, Dialog, placeholder) => {
  it("puts the caret in the text under a mouse", async () => {
    render(<Dialog />)
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByPlaceholderText(placeholder)),
    )
  })

  it("does not focus the text under a finger, and closes from its own button on a phone", async () => {
    phone = true
    coarsePointer()
    render(<Dialog />)
    await waitFor(() =>
      expect(document.activeElement?.getAttribute("data-slot")).toBe("dialog-content"),
    )
    expect(document.activeElement).not.toBe(screen.getByPlaceholderText(placeholder))
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy()
  })
})

describe("project settings", () => {
  it("covers the screen and closes from its own button on a phone", async () => {
    phone = true
    coarsePointer()
    render(<ProjectSettingsDialog />)
    await waitFor(() =>
      expect(document.activeElement?.getAttribute("data-slot")).toBe("dialog-content"),
    )
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy()
    expect(document.activeElement).not.toBe(screen.getByPlaceholderText("KEY=VALUE"))
  })
})

// These forms hold edits nothing else keeps, so every way out asks before it
// loses them, in words that name what discarding leaves as it was.
describe.each([
  [
    "the agent's environment",
    AgentEnvDialog,
    "KEY=VALUE",
    () => store.closeAgentEnv,
    "The environment has 1 unsaved change. Discarding it leaves the environment of project acme as it was.",
  ],
  [
    "the agent's startup command",
    AgentStartupCommandDialog,
    "npm run dev",
    () => store.closeAgentStartupCommand,
    "The startup command has 1 unsaved change. Discarding it leaves the startup command of project acme as it was.",
  ],
  [
    "the project settings",
    ProjectSettingsDialog,
    "npm run dev",
    () => store.closeProjectSettings,
    "The project settings have 1 unsaved change. Discarding it leaves the settings of project acme as it was.",
  ],
])("closing %s", (_name, Dialog, placeholder, close, sentence) => {
  it("closes at once while nothing was changed", () => {
    render(<Dialog />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(close()).toHaveBeenCalled()
  })

  it("asks before Cancel or Escape loses an edit, and closes only on Discard", async () => {
    render(<Dialog />)
    const field = screen.getByPlaceholderText(placeholder) as HTMLTextAreaElement
    fireEvent.change(field, { target: { value: `${field.value}\nFOO=1` } })
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    const question = await screen.findByText(/Discarding (it|them) leaves/)
    expect(question.textContent).toBe(sentence)
    // The project's name is the chip, not part of the words.
    expect(within(question).getByText("acme")).not.toBe(question)
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
    await waitFor(() => expect(screen.queryByText("Discard unsaved changes?")).toBeNull())
    fireEvent.keyDown(screen.getByPlaceholderText(placeholder), { key: "Escape" })
    fireEvent.click(await screen.findByRole("button", { name: "Discard changes" }))
    expect(close()).toHaveBeenCalledTimes(1)
  })
})

// One Save for all three: held with its reason while nothing would change,
// and busy while the request is out, so no way out claims to leave the
// settings as they were under a save that is about to land.
describe.each([
  [
    "the agent's environment",
    AgentEnvDialog,
    "KEY=VALUE",
    () => store.closeAgentEnv,
    "Nothing to save yet: no variable has changed.",
    { env: { PATH: "/usr/bin", http_proxy: "http://proxy", FOO: "1" } },
  ],
  [
    "the agent's startup command",
    AgentStartupCommandDialog,
    "npm run dev",
    () => store.closeAgentStartupCommand,
    "Nothing to save yet: the startup command has not changed.",
    { startup_command: "\nFOO=1" },
  ],
  [
    "the project settings",
    ProjectSettingsDialog,
    "npm run dev",
    () => store.closeProjectSettings,
    "Nothing to save yet: no setting has changed.",
    { startup_command: "\nFOO=1" },
  ],
])("%s' Save", (_name, Dialog, placeholder, close, held, patch) => {
  it("says why it is held while nothing was changed", async () => {
    render(<Dialog />)
    const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.click(save.parentElement!)
    expect((await screen.findAllByText(held)).length).toBeGreaterThan(0)
  })

  it("is busy while a save is on its way: no way out asks or closes, and a second press sends nothing", async () => {
    let finish: (ok: boolean) => void = () => {}
    vi.mocked(store.updateProjectSettings).mockReturnValue(
      new Promise<boolean>((resolve) => {
        finish = resolve
      }),
    )
    render(<Dialog />)
    const field = screen.getByPlaceholderText(placeholder) as HTMLTextAreaElement
    fireEvent.change(field, { target: { value: `${field.value}\nFOO=1` } })
    const save = screen.getByRole("button", { name: "Save" })
    fireEvent.click(save)
    await waitFor(() => expect(save.getAttribute("aria-busy")).toBe("true"))
    fireEvent.keyDown(field, { key: "Escape" })
    fireEvent.click(save)
    expect(screen.queryByText("Discard unsaved changes?")).toBeNull()
    expect(close()).not.toHaveBeenCalled()
    expect(store.updateProjectSettings).toHaveBeenCalledTimes(1)
    expect(vi.mocked(store.updateProjectSettings).mock.calls[0].slice(0, 2)).toEqual(["p1", patch])
    await act(async () => finish(true))
    expect(close()).toHaveBeenCalledTimes(1)
  })
})

// Text that Save cannot use is still an edit the question asks about, and the
// form says which lines Save leaves out.
describe.each([
  ["the agent's environment", AgentEnvDialog, () => store.closeAgentEnv],
  ["the project settings", ProjectSettingsDialog, () => store.closeProjectSettings],
])("a line %s cannot use", (_name, Dialog, close) => {
  it("asks before it is thrown away, and says Save leaves it out", async () => {
    render(<Dialog />)
    const field = screen.getByPlaceholderText("KEY=VALUE") as HTMLTextAreaElement
    fireEvent.change(field, { target: { value: `${field.value}\n# the proxy is for CI` } })
    expect(screen.getByText("Line 3 is not KEY=VALUE, so Save leaves it out.")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect((await screen.findByText(/Discarding/)).textContent).toMatch(/ 1 unsaved change\./)
    expect(close()).not.toHaveBeenCalled()
  })
})
