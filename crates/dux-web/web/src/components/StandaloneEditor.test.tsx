// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import {
  EXPLORER_DEFAULT_SIZE_PX,
  EXPLORER_MIN_SIZE_PX,
} from "@/lib/editorLayout"
import { agentRoot, rootKey } from "@/lib/editorRoot"

// The standalone editor surface: a whole browser tab that is nothing but the
// editor (plan (b)). What is pinned here: the shell composes EditorBody (the
// code editor mounts), names the agent, deliberately offers NO in-app exit
// (the browser's own Back/close-tab controls are the way out), renders
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
  }
})

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
        { id: SESSION, title: "My agent", branch_name: "feat/x", tabs: [{ id: SESSION }] },
      ],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as DuxState["spine"],
    ...overrides,
  })
}

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

  it("composes EditorBody full-viewport with the agent name and no in-app exit", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    // The body mounted: the (stubbed) code editor is on screen.
    await screen.findByTestId("code-editor")
    // The agent is named.
    expect(screen.getByText("My agent")).toBeTruthy()
    // No "Open in dux" link: the exit is deliberately the browser's own
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
    expect(screen.getByText("vim")).toBeTruthy()
    expect(screen.getByText("~/code")).toBeTruthy()
  })

  it("keeps the desktop header inline: mode toggle, preview-capable controls, Open local editor", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    await screen.findByTestId("code-editor")
    // The File/Diff segmented control renders inline.
    expect(screen.getByRole("group", { name: "View mode" })).toBeTruthy()
    // The GUI-editor dropdown is named "Open local editor" (it spawns an
    // editor on the machine dux runs on, not in the browser).
    expect(screen.getByText("Open local editor")).toBeTruthy()
    expect(screen.queryByText(/^Open editor$/)).toBeNull()
    // The mobile fold's ⋯ trigger exists but is desktop-hidden by class.
    const fold = screen.getByRole("button", { name: /more editor actions/i })
    expect(fold.className).toContain("md:hidden")
  })

  it("folds the phone header's secondary controls into one ⋯ menu", async () => {
    await seedState({})
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
    // active mode readable), the preview toggle, and Open local editor.
    fireEvent.click(
      screen.getByRole("button", { name: /more editor actions/i }),
    )
    const fileItem = await screen.findByRole("menuitem", { name: /file view/i })
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
