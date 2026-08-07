// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The standalone editor surface: a whole browser tab that is nothing but the
// editor (plan (b)). What is pinned here: the shell composes EditorBody (the
// code editor mounts), names the agent, offers the open-in-dux anchor as a
// PLAIN hash link (the URL is what swaps surfaces), renders not-found for a
// vanished agent, and the overlay Dialog stands down while the tab is the
// standalone surface so EditorBody can never mount twice.

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
    rawUrl: (sessionId: string, path: string) =>
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/files/raw?path=${encodeURIComponent(path)}`,
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
    editorTarget: { sessionId: SESSION, initialPath: PATH, initialMode: "file" },
    editorRoute: { sessionId: SESSION, mode: "file", path: PATH },
    editorTabs: {
      [SESSION]: {
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
    const { clearSessionDrafts } = await import("@/lib/editorDrafts")
    clearSessionDrafts(SESSION)
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("composes EditorBody full-viewport with the agent name and an open-in-dux link", async () => {
    await seedState({})
    const { StandaloneEditorShell } = await import(
      "@/components/StandaloneEditor"
    )
    render(<StandaloneEditorShell />)
    // The body mounted: the (stubbed) code editor is on screen.
    await screen.findByTestId("code-editor")
    // The agent is named.
    expect(screen.getByText("My agent")).toBeTruthy()
    // The way out is a PLAIN hash anchor (no target=_blank): the hash change
    // fires popstate and the URL decides which surface renders.
    const link = screen.getByRole("link", { name: /open in dux/i })
    expect(link.getAttribute("href")).toBe("#/agent/s1")
    expect(link.getAttribute("target")).toBeNull()
    // And the body knows it IS the tab: no Close button, no open-in-new-tab.
    expect(screen.queryByRole("button", { name: /^close$/i })).toBeNull()
    expect(
      screen.queryByRole("link", { name: /open editor in new tab/i }),
    ).toBeNull()
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
    const { clearSessionDrafts } = await import("@/lib/editorDrafts")
    clearSessionDrafts(SESSION)
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
