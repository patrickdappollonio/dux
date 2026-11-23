// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import {
  EXPLORER_DEFAULT_SIZE_PX,
  EXPLORER_LAYOUT_KEY,
  EXPLORER_MIN_SIZE_PX,
} from "@/lib/editorLayout"
import { agentRoot, rootKey } from "@/lib/editorRoot"

// These cases render the whole editor shell and then wait on `waitFor` polls, so
// a busy machine can push a perfectly healthy run past the default per-test
// window and fail on the clock rather than on the assertion. The larger window
// is scoped to this file: it buys nothing anywhere else, and a global raise
// would hide a genuinely hung test in every other file.
vi.setConfig({ testTimeout: 20_000, hookTimeout: 20_000 })

// The standalone editor surface: a whole browser tab that is nothing but the
// editor (plan (b)). What is pinned here: the shell composes EditorBody (the
// code editor mounts), names the agent in the body's header, offers an in-app
// exit only on a phone (the generic Back, which pushes a named screen; on a
// computer the browser's own Back/close-tab controls are the way out), renders
// not-found for a vanished agent, and the overlay Dialog stands down while
// the tab is the standalone surface so EditorBody can never mount twice.
// Also pinned: the phone header folds its secondary controls into one ⋯
// menu (the row-actions tenet), keeping only the explorer toggle and Save
// inline, while the desktop header keeps today's inline controls.

if (!Element.prototype.getAnimations) {
  Element.prototype.getAnimations = () => []
}

const SESSION = "s1"
const PATH = "src/a.ts"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    leaveStandaloneEditor: (...a: unknown[]) => leaveStandaloneEditor(...a),
  }
})
const leaveStandaloneEditor = vi.fn()

vi.mock("@/lib/fileApi", () => ({
  fileApi: {
    list: vi.fn(async () => ({ files: [PATH], truncated: false })),
    tree: vi.fn(async () => ({ dir: "", entries: [] })),
    read: vi.fn(async () => ({
      path: PATH,
      content: "hello\n",
      binary: false,
      read_only: false,
    })),
    rawUrl: (_root: unknown, path: string) =>
      `/api/v1/sessions/s1/files/raw?path=${encodeURIComponent(path)}`,
    diff: vi.fn(async () => ({ head: "", working: "", binary: false })),
    write: vi.fn(),
    openInEditor: vi.fn(),
    createFile: vi.fn(),
    createDir: vi.fn(),
    rename: vi.fn(),
    remove: vi.fn(),
    writable: vi.fn(async () => false),
  },
}))

function codeEditorStub() {
  return {
    default: ({ value }: { value: string }) => (
      <textarea data-testid="code-editor" defaultValue={value} readOnly />
    ),
  }
}
vi.mock("@/components/CodeEditor", codeEditorStub)
vi.mock("./CodeEditor", codeEditorStub)
// The language picker's registry read would otherwise load real Monaco, which
// cannot run under jsdom.
let registeredLanguages: Array<{ id: string; aliases?: string[]; extensions?: string[] }> = []
vi.mock("@/lib/monacoLanguages", () => ({
  loadRegisteredLanguages: async () => registeredLanguages,
}))

// Panel props recorded at render time. This shell is the UNCAPPED half of the
// width question: the overlay's DialogContent caps at min(80rem, 100%-2rem)
// and this one fills the tab, so a PERCENTAGE explorer was ~281px there and
// ~563px here. The assertion is that both shells hand the library the same
// pixel sizes; its twin lives in EditorOverlay.test.tsx, against the same
// constants. The real Panel still renders (spread actual).
const recordedPanelProps: Array<Record<string, unknown>> = []
vi.mock("react-resizable-panels", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("react-resizable-panels")>()
  const Panel = (props: Parameters<typeof actual.Panel>[0]) => {
    recordedPanelProps.push(props as unknown as Record<string, unknown>)
    return <actual.Panel {...props} />
  }
  return { ...actual, Panel }
})

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}))

function installBootStubs() {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
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
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}

function baseState(overrides: Partial<DuxState>): void {
  mockState = {
    ...(mockState ?? {}),
    ...overrides,
  } as DuxState
}

async function seedState(overrides: Partial<DuxState>) {
  const { getSnapshot } = await import("@/lib/store")
  mockState = { ...getSnapshot() } as DuxState
  baseState({
    standaloneEditor: true,
    editorTarget: { root: agentRoot(SESSION), initialPath: PATH, initialMode: "file" },
    editorRoute: { root: agentRoot(SESSION), mode: "file", path: PATH },
    editorTabs: {
      [rootKey(agentRoot(SESSION))]: {
        tabs: [
          { id: "t1", path: PATH, dirty: false, preview: false, mode: "file" },
        ],
        activeId: "t1",
      },
    },
    spine: {
      projects: [],
      sessions: [
        {
          id: SESSION,
          title: "My agent",
          workspace: { kind: "managed", project_id: "p1", branch_name: "feat/x", initial_branch: "feat/x", branch_provenance: "created", source_branch: "main", worktree_path: "/wt/s1" },
          tabs: [{ id: SESSION }],
        },
      ],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as DuxState["spine"],
    ...overrides,
  })
}

// Loaded at collection, where no test timeout applies: the first load of the
// editor and the store behind it is the slowest step in the file, and inside a
// test it is charged to whichever case happens to run first.
await import("@/components/StandaloneEditor")

describe("the standalone editor shell", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("composes EditorBody full-viewport, naming the agent in the body's own header", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    // The body mounted: the (stubbed) code editor is on screen.
    await screen.findByTestId("code-editor")
    // The agent is named once, as a chip after "Editor for", in the one
    // header the body draws: there is no second identity bar above it.
    const chips = screen.getAllByText("My agent")
    expect(chips).toHaveLength(1)
    expect(chips[0].getAttribute("data-slot")).toBe("inline-code")
    expect(
      chips[0].parentElement!.parentElement!.textContent!.replace(/\s+/g, ""),
    ).toBe("EditorforMyagent")
    // No "Open in dux" link: on a computer the exit is the browser's own
    // controls (Back, or closing the tab), so the shell ships no anchor.
    expect(screen.queryByRole("link", { name: /open in dux/i })).toBeNull()
    // And the body knows it IS the tab: no Close button, no open-in-new-tab.
    expect(screen.queryByRole("button", { name: /^close$/i })).toBeNull()
    expect(
      screen.queryByRole("link", { name: /open editor in new tab/i }),
    ).toBeNull()
  })

  it("names a terminal root by its row identity, directory and all", async () => {
    // The tab and the sidebar row must call the same thing the same thing, so
    // the header is drawn from the same facts: the terminal's own identity,
    // with the second line's owner text beside it, which for a standalone
    // terminal is the home-collapsed directory it opened in.
    const terminalRoot = {
      kind: "terminal" as const,
      terminalId: "t1",
      owner: { kind: "standalone" as const },
    }
    await seedState({
      editorTarget: { root: terminalRoot, initialPath: PATH, initialMode: "file" },
      editorRoute: { root: terminalRoot, mode: "file", path: PATH },
      editorTabs: {
        [rootKey(terminalRoot)]: {
          tabs: [
            { id: "t1", path: PATH, dirty: false, preview: false, mode: "file" },
          ],
          activeId: "t1",
        },
      },
      spine: {
        projects: [],
        sessions: [],
        terminals: [
          {
            id: "t1",
            owner: { kind: "standalone", cwd_label: "~/code" },
            label: "Terminal 1",
            has_output: false,
            working: false,
            typing: false,
            foreground_cmd: "vim",
            sort_order: 1,
            created_at: "2026-07-17T12:00:00Z",
            updated_at: "2026-07-17T12:00:00Z",
          },
        ],
        sidebar: { groups: [], agentless_start: null },
      },
    } as unknown as Partial<DuxState>)
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    const chip = screen.getByText("vim")
    expect(chip.getAttribute("data-slot")).toBe("inline-code")
    // The owner text sits after the chip, quieter.
    // Where it lives is a name too, so it is a chip, joined by plain words:
    // "Editor for vim in ~/code".
    const detail = screen.getByText("~/code")
    expect(detail.getAttribute("data-slot")).toBe("inline-code")
    expect(chip.nextElementSibling!.textContent).toBe("in")
    expect(chip.nextElementSibling!.nextElementSibling).toBe(detail)
    // The terminal's glyph, not the agent's.
    const header = chip.closest("div.border-b") as HTMLElement
    expect(header.querySelector("svg.lucide-square-terminal")).toBeTruthy()
  })

  it("leads the phone header with the generic Back, which pushes the agent's Changes screen", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    const back = screen.getByRole("button", { name: "Back" })
    // The same control every phone screen leads with, and a phone's alone.
    expect(back.className).toContain("size-10")
    expect(back.className).toContain("md:hidden")
    expect(back.querySelector("svg.lucide-chevron-left")).toBeTruthy()
    const header = back.closest("div.border-b") as HTMLElement
    expect(header.firstElementChild).toBe(back)
    const historyBack = vi.spyOn(history, "back")
    fireEvent.click(back)
    expect(leaveStandaloneEditor).toHaveBeenCalledWith(agentRoot(SESSION))
    expect(historyBack).not.toHaveBeenCalled()
    historyBack.mockRestore()
  })

  it("moves the path to the status bar and keeps the language picker out of the header", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    const path = screen.getByText(PATH)
    expect(path.closest("div.border-t")).toBeTruthy()
    expect(path.closest("div.border-b")).toBeNull()
    const header = screen
      .getByRole("button", { name: "Back" })
      .closest("div.border-b") as HTMLElement
    expect(header.querySelector("[aria-label^='Syntax language']")).toBeNull()
  })

  it("keeps the desktop header inline: mode toggle and preview-capable controls", async () => {
    await seedState({
      changes: {
        sessionId: SESSION,
        phase: "loaded",
        rev: 1,
        staged: [],
        unstaged: [{ path: PATH, status: "M", additions: 1, deletions: 0, staged: false }],
        error: null,
      } as unknown as DuxState["changes"],
    })
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    // The File/Diff segmented control renders inline.
    expect(screen.getByRole("group", { name: "View mode" })).toBeTruthy()
    // The mobile fold's ⋯ trigger exists but is desktop-hidden by class.
    const fold = screen.getByRole("button", { name: /more editor actions/i })
    expect(fold.className).toContain("md:hidden")
  })

  it("folds the phone header's secondary controls into one ⋯ menu", async () => {
    await seedState({
      changes: {
        sessionId: SESSION,
        phase: "loaded",
        rev: 1,
        staged: [],
        unstaged: [{ path: PATH, status: "M", additions: 1, deletions: 0, staged: false }],
        error: null,
      } as unknown as DuxState["changes"],
    })
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    // The inline secondary controls carry the phone-hidden class; only the
    // explorer toggle and Save stay visible inline on a phone.
    expect(
      screen.getByRole("group", { name: "View mode" }).className,
    ).toContain("max-md:hidden")
    expect(
      screen.getByRole("button", { name: /show the file explorer|hide the file explorer/i })
        .className,
    ).not.toContain("max-md:hidden")
    expect(
      screen.getByRole("button", { name: /^save$/i }).className,
    ).not.toContain("max-md:hidden")
    // The ⋯ menu carries the folded controls: the mode switch (with the
    // active mode readable), the preview toggle, and, because jsdom's host is
    // localhost, Open local editor.
    fireEvent.click(
      screen.getByRole("button", { name: /more editor actions/i }),
    )
    const fileItem = await screen.findByRole("menuitem", {
      name: /file view/i,
    })
    expect(fileItem.getAttribute("aria-current")).toBe("true")
    const diffItem = screen.getByRole("menuitem", { name: /diff view/i })
    expect(diffItem.getAttribute("aria-current")).toBeNull()
    // A .ts file offers no draft preview, so the fold (like the inline
    // header) carries no preview item for it.
    expect(screen.queryByRole("menuitem", { name: /show preview/i })).toBeNull()
    const openLocal = screen.getByRole("menuitem", {
      name: /open local editor/i,
    })
    expect(openLocal).toBeTruthy()
    // The same laptop the desktop trigger carries: one action, one glyph,
    // whichever surface it is reached from. It must not be the external-link
    // arrow, which belongs to the control that opens a browser tab.
    expect(openLocal.querySelector("svg.lucide-laptop")).toBeTruthy()
    expect(openLocal.querySelector("svg.lucide-external-link")).toBeNull()
  })

  it("the ⋯ menu offers the preview toggle for a previewable file", async () => {
    await seedState({
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: {
          tabs: [
            {
              id: "t1",
              path: "README.md",
              dirty: false,
              preview: false,
              mode: "file",
            },
          ],
          activeId: "t1",
        },
      },
    })
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    fireEvent.click(
      screen.getByRole("button", { name: /more editor actions/i }),
    )
    expect(
      await screen.findByRole("menuitem", { name: /show preview/i }),
    ).toBeTruthy()
  })

  it("gives the explorer the same pixel width the modal shell gives it", async () => {
    recordedPanelProps.length = 0
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    expect(explorer).toBeTruthy()
    expect(explorer!.defaultSize).toBe(`${EXPLORER_DEFAULT_SIZE_PX}px`)
    expect(explorer!.minSize).toBe(`${EXPLORER_MIN_SIZE_PX}px`)
    // The whole point: no percentage, because this shell is a different width
    // from the modal and a percentage would resolve differently in each.
    expect(String(explorer!.defaultSize)).not.toContain("%")
  })

  it("renders the not-found screen when the address names a vanished agent", async () => {
    await seedState({
      editorTarget: null,
      editorRoute: null,
      routeNotFound: { kind: "agent", sessionId: "gone" },
    })
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    expect(screen.getByText(/agent not found/i)).toBeTruthy()
  })
})

// The phone's language picker: the fold's entry opens the shared searchable
// list as a bottom sheet.
describe("the phone's language sheet", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    vi.stubGlobal("innerWidth", 390)
    registeredLanguages = [
      { id: "typescript", aliases: ["TypeScript"], extensions: [".ts"] },
      { id: "yaml", aliases: ["YAML", "yaml"], extensions: [".yaml", ".yml"] },
    ]
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    registeredLanguages = []
  })

  async function openSheet() {
    const fold = screen.getByRole("button", { name: /more editor actions/i })
    fireEvent.click(fold)
    fireEvent.click(await screen.findByRole("menuitem", { name: /typescript…/i }))
    const field = await screen.findByRole("combobox", { name: "Search languages" })
    return { fold, field }
  }

  it("searches in a sheet, and hands focus back to the fold after a pick", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    const { fold, field } = await openSheet()
    const sheet = field.closest("[data-slot=popover-content]") as HTMLElement
    expect(sheet.className).toContain("rounded-t-2xl")
    // Positioned, so shown: an unanchored positioner stays transparent.
    await waitFor(() => expect(sheet.parentElement?.style.opacity).not.toBe("0"))
    // The search takes focus at once, so the soft keyboard comes up with it.
    await waitFor(() => expect(document.activeElement).toBe(field))
    fireEvent.change(field, { target: { value: "yml" } })
    fireEvent.click(screen.getByRole("option", { name: /^YAML$/ }))
    await waitFor(() => expect(document.activeElement).toBe(fold))
    fireEvent.click(fold)
    expect(await screen.findByRole("menuitem", { name: /^YAML…$/ })).toBeTruthy()
  })

  it("does not reopen by itself once its file has gone", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    const { openModals } = await import("@/lib/modalHistory")
    const view = render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    await openSheet()
    const showTab = (path: string) => {
      baseState({
        editorTabs: {
          [rootKey(agentRoot(SESSION))]: {
            tabs: [{ id: "t1", path, dirty: false, preview: false, mode: "file" }],
            activeId: "t1",
          },
        },
      })
      view.rerender(<StandaloneEditorShell />)
    }
    showTab("assets/logo.png")
    await screen.findByRole("img")
    await waitFor(() => expect(openModals()).toEqual([]))
    // Focus goes back to the fold the sheet came from, not the page body.
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: /more editor actions/i }),
    )
    showTab(PATH)
    await screen.findByTestId("code-editor")
    expect(screen.queryByRole("combobox", { name: "Search languages" })).toBeNull()
  })
})

describe("the overlay stands down on the standalone surface", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("EditorOverlay renders nothing while standaloneEditor is set", async () => {
    await seedState({})
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    const { container } = render(<EditorOverlay />)
    expect(container.innerHTML).toBe("")
    expect(screen.queryByTestId("code-editor")).toBeNull()
  })
})

// A standalone editor tab has no pane of the agent's row. Opened cold, the
// agents list is its only word on who drives: another device driving makes it
// read-only, and nobody driving leaves it writable. Opened from a page, it
// writes as that page, so the server says whether the driver is its opener.
describe("the standalone editor tab and who drives the agent", () => {
  const READ_ONLY =
    "Read-only: this agent is active on another device, so its files cannot be changed here. Take over the agent to edit them."

  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  async function mountDrivenBy(inputOwner: string | undefined) {
    await seedState({})
    const spine = mockState.spine!
    baseState({
      ownPtyConnIds: {},
      ptyOwnership: {},
      spine: {
        ...spine,
        sessions: spine.sessions.map((s) => ({
          ...s,
          tabs: [{ id: SESSION, input_owner: inputOwner }],
        })),
      } as DuxState["spine"],
    })
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
  }

  it("is read-only while the agents list says another device drives the agent", async () => {
    await mountDrivenBy("another-page")
    expect(screen.getByText(READ_ONLY)).toBeTruthy()
    expect(screen.getByRole("button", { name: /new file/i }).hasAttribute("disabled")).toBe(true)
  })

  /// Stand a page in as this tab's opener, answering the handshake with `page`.
  function openedBy(page: string): void {
    const frame = document.createElement("iframe")
    document.body.append(frame)
    const opener = frame.contentWindow as Window
    opener.postMessage = ((message: { type?: string }) => {
      if (message.type !== "dux:pages?") return
      window.dispatchEvent(
        new MessageEvent("message", {
          data: { type: "dux:pages", pages: [page] },
          origin: window.location.origin,
          source: opener,
        }),
      )
    }) as Window["postMessage"]
    vi.stubGlobal("opener", opener)
  }

  async function driver(inputOwner: string): Promise<void> {
    const spine = mockState.spine!
    baseState({
      spine: {
        ...spine,
        sessions: spine.sessions.map((s) => ({
          ...s,
          tabs: [{ id: SESSION, input_owner: inputOwner }],
        })),
      } as DuxState["spine"],
    })
  }

  it("opened from the page that drives the agent, writes as that page until another device takes over", async () => {
    const { fileApi } = await import("@/lib/fileApi")
    const { resetOpenerVerdicts } = await import("@/lib/openerPage")
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    resetOpenerVerdicts()
    // The store is a double here, so nothing has said this tab is the editor.
    ;(await import("@/lib/pageSeat")).setEditorSurface(true)
    openedBy("a".repeat(32))
    vi.mocked(fileApi.writable).mockResolvedValue(true)
    await seedState({ ownPtyConnIds: {}, ptyOwnership: {} })
    await driver("the-opener")
    const { rerender } = render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")

    const newFile = () => screen.getByRole("button", { name: /new file/i })
    await waitFor(() => expect(newFile().hasAttribute("disabled")).toBe(false))
    expect(screen.queryByText(/^Read-only/)).toBeNull()
    expect(screen.queryByText(/another device/)).toBeNull()

    vi.mocked(fileApi.writable).mockResolvedValue(false)
    await driver("another-device")
    rerender(<StandaloneEditorShell />)
    expect(await screen.findByText(READ_ONLY)).toBeTruthy()
    expect(newFile().hasAttribute("disabled")).toBe(true)

    // The opener takes the agent back.
    vi.mocked(fileApi.writable).mockResolvedValue(true)
    await driver("the-opener-again")
    rerender(<StandaloneEditorShell />)
    await waitFor(() => expect(newFile().hasAttribute("disabled")).toBe(false))
    expect(screen.queryByText(/^Read-only/)).toBeNull()
  })

  it("opened from a page that only watches, is read-only as that page is", async () => {
    const { fileApi } = await import("@/lib/fileApi")
    const { resetOpenerVerdicts } = await import("@/lib/openerPage")
    resetOpenerVerdicts()
    openedBy("b".repeat(32))
    vi.mocked(fileApi.writable).mockResolvedValue(false)
    await mountDrivenBy("another-page")
    expect(await screen.findByText(READ_ONLY)).toBeTruthy()
  })

  it("stays writable while nobody drives the agent", async () => {
    await mountDrivenBy(undefined)
    expect(screen.queryByText(/^Read-only/)).toBeNull()
    expect(screen.getByRole("button", { name: /new file/i }).hasAttribute("disabled")).toBe(false)
  })
})

// The phone layout gives an open file the whole width.
describe("the phone's explorer", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    vi.stubGlobal("innerWidth", 390)
    window.dispatchEvent(new Event("resize"))
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    window.dispatchEvent(new Event("resize"))
  })

  async function mount(fileOpen: boolean, storedCollapsed: boolean) {
    localStorage.setItem(EXPLORER_LAYOUT_KEY, JSON.stringify({ px: 300, collapsed: storedCollapsed }))
    await seedState(
      fileOpen
        ? {}
        : { editorTabs: { [rootKey(agentRoot(SESSION))]: { tabs: [], activeId: null } } },
    )
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    return render(<StandaloneEditorShell />)
  }
  it("is shown while no file is open", async () => {
    await mount(false, true)
    expect(await screen.findByRole("button", { name: /hide the file explorer/i })).toBeTruthy()
  })

  it("is hidden while a file is open", async () => {
    await mount(true, false)
    expect(await screen.findByRole("button", { name: /show the file explorer/i })).toBeTruthy()
  })

  const explorerHidden = () => screen.getByTestId("editor-explorer").style.flexGrow === "0"

  it("hides when the layout switches to the phone's with a file open", async () => {
    vi.stubGlobal("innerWidth", 1280)
    window.dispatchEvent(new Event("resize"))
    await mount(true, false)
    await screen.findByRole("button", { name: /hide the file explorer/i })
    expect(explorerHidden()).toBe(false)
    vi.stubGlobal("innerWidth", 390)
    act(() => {
      window.dispatchEvent(new Event("resize"))
    })
    await waitFor(() => expect(explorerHidden()).toBe(true))
  })

  // The tab keeps its id through a rename and through a preview tab replaced
  // in place; only the second is a file being opened.
  async function openTreeThen(change: (tab: { id: string; path: string }) => Partial<DuxState>) {
    const { StandaloneEditorShell } = await import("@/components/StandaloneEditor")
    const { rerender } = await mount(true, false)
    fireEvent.click(await screen.findByRole("button", { name: /show the file explorer/i }))
    await waitFor(() => expect(explorerHidden()).toBe(false))
    act(() => {
      baseState(change({ id: "t1", path: PATH }))
      rerender(<StandaloneEditorShell />)
    })
  }
  const tabs = (path: string): Partial<DuxState> => ({
    editorTabs: {
      [rootKey(agentRoot(SESSION))]: {
        tabs: [{ id: "t1", path, dirty: false, preview: true, mode: "file" }],
        activeId: "t1",
      },
    },
  })

  it("stays open when the open file is renamed", async () => {
    await openTreeThen(() => tabs("src/renamed.ts"))
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(explorerHidden()).toBe(false)
  })

  it("hides when a file opens in place of the preview tab", async () => {
    await openTreeThen(() => ({ ...tabs("src/other.ts"), editorFileOpens: mockState.editorFileOpens + 1 }))
    await waitFor(() => expect(explorerHidden()).toBe(true))
  })

  // The panel library reports a collapse to the toggle from the browser's
  // layout pass, which jsdom does not run; the panel's own size says it.
  it("hides when a file is picked from the tree", async () => {
    const { fileApi } = await import("@/lib/fileApi")
    vi.mocked(fileApi.tree).mockResolvedValue({
      dir: "",
      entries: [{ name: "b.ts", path: "b.ts", is_dir: false, is_symlink: false, expandable: false }],
    })
    await mount(false, false)
    const explorer = screen.getByTestId("editor-explorer")
    expect(explorer.style.flexGrow).not.toBe("0")
    fireEvent.click(await screen.findByText("b.ts"))
    await waitFor(() => expect(explorer.style.flexGrow).toBe("0"))
  })
})
