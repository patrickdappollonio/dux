// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// What this file exists for, and it is one property.
//
// `website/docs/web-editor.md` promises that a save refused for being too large
// KEEPS your text: the tab stays dirty with everything you typed still in it, so
// you can trim it down and save again or copy it out. The code delivers that by
// doing nothing in the write's `.catch` (it touches neither the buffers nor the
// dirty flag), and until now that was asserted only by a comment sitting next to
// the code, which is exactly the kind of thing a later cleanup pass "tidies" by
// resetting the buffer. Nothing else in the suite mounted this component at all.
//
// So the test drives the real save path and asserts the draft survives the
// refusal. Monaco, the file API and the store are stubbed, because none of them
// is the subject: the subject is what EditorBody does with a rejected write.

// base-ui's ScrollArea viewport probes `getAnimations` on a timer and jsdom does
// not implement it. Same shim the FileTree suite installs.
if (!Element.prototype.getAnimations) {
  Element.prototype.getAnimations = () => []
}

const SESSION = "s1"
const TAB_ID = "tab-1"
const PATH = "src/big.txt"
const ON_DISK = "original contents\n"
const TYPED = "original contents\nplus a great deal more that the server refuses\n"

let mockState: DuxState
const editorSetTabDirtyMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    editorSetTabDirty: editorSetTabDirtyMock,
  }
})

const writeMock = vi.fn()
const readMock = vi.fn(async () => ({
  path: PATH,
  content: ON_DISK,
  binary: false,
  read_only: false,
}))
vi.mock("@/lib/fileApi", () => ({
  fileApi: {
    list: vi.fn(async () => ({ files: [PATH], truncated: false })),
    tree: vi.fn(async () => ({ dir: "", entries: [] })),
    read: () => readMock(),
    // The real builder's shape, duplicated here because the module is fully
    // mocked: the image-pane test asserts the exact URL the <img> gets.
    rawUrl: (sessionId: string, path: string) =>
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/files/raw?path=${encodeURIComponent(path)}`,
    diff: vi.fn(async () => ({ head: "", working: "", binary: false })),
    write: (...args: unknown[]) => writeMock(...args),
    openInEditor: vi.fn(),
    createFile: vi.fn(),
    createDir: vi.fn(),
    rename: vi.fn(),
    remove: vi.fn(),
  },
}))

// Monaco cannot run in jsdom and is not what is under test. A textarea carrying
// the same value/onChange contract stands in, which also makes the buffer's
// contents directly readable from the DOM: that IS the "your text is kept"
// assertion.
//
// EditorOverlay lazy-imports it by RELATIVE path, so that specifier is what has
// to be mocked; the alias is mocked too, so a later import-style change cannot
// silently unmock it and drag real Monaco into jsdom.
function codeEditorStub() {
  return {
    default: ({
      value,
      onChange,
    }: {
      value: string
      onChange: (v: string) => void
    }) => (
      <textarea
        data-testid="code-editor"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    ),
  }
}
vi.mock("@/components/CodeEditor", codeEditorStub)
vi.mock("./CodeEditor", codeEditorStub)

const toastError = vi.fn()
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: (...a: unknown[]) => toastError(...a),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}))

function installBootStubs() {
  // jsdom has no ResizeObserver; FileTree's viewport-height probe constructs one
  // on mount. Same stub the FileTree suite uses.
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

async function mountWithDirtyTab() {
  const { EditorOverlay } = await import("@/components/EditorOverlay")
  const { getSnapshot } = await import("@/lib/store")
  // The real store's own state, with only the editor slice replaced. Taking the
  // baseline from the store itself means a new required field cannot make this
  // file silently stale.
  mockState = {
    ...getSnapshot(),
    editorTarget: { sessionId: SESSION, initialPath: PATH },
    editorTabs: {
      [SESSION]: {
        tabs: [
          {
            id: TAB_ID,
            path: PATH,
            // Already dirty: `save()` early-returns on a clean tab, so this is
            // the state a user is in when they press Save.
            dirty: true,
            preview: false,
            mode: "file",
          },
        ],
        activeId: TAB_ID,
      },
    },
  } as DuxState

  render(<EditorOverlay />)
  const box = await screen.findByTestId("code-editor")
  await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(ON_DISK))
  fireEvent.change(box, { target: { value: TYPED } })
  await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(TYPED))
  return box as HTMLTextAreaElement
}

describe("a save the server refuses", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("keeps the text the user typed and leaves the tab dirty", async () => {
    writeMock.mockRejectedValue(
      new Error(
        "This file is too large for the editor to save: the request body is over the 10 MB limit. Edit it outside dux instead.",
      ),
    )
    const box = await mountWithDirtyTab()

    fireEvent.click(screen.getByRole("button", { name: /save/i }))
    await waitFor(() => expect(writeMock).toHaveBeenCalled())
    await waitFor(() => expect(toastError).toHaveBeenCalled())

    // The whole promise of the docs page: the buffer is untouched.
    expect(box.value).toBe(TYPED)
    // And the tab is never marked clean, so the unsaved-changes guard still
    // protects it on close.
    expect(editorSetTabDirtyMock).not.toHaveBeenCalledWith(
      SESSION,
      TAB_ID,
      false,
    )
    // The refusal is reported, and it says what the limit is.
    expect(String(toastError.mock.calls[0][0])).toContain("too large")
  })

  // The companion, so the negative assertion above cannot pass because nothing
  // ever happened: an ACCEPTED save on the very same setup does clear the tab's
  // dirty flag. If this stops firing, the test above has stopped proving
  // anything and both fail together.
  it("is distinguishable from an accepted save, which does clear the flag", async () => {
    writeMock.mockResolvedValue(undefined)
    await mountWithDirtyTab()

    fireEvent.click(screen.getByRole("button", { name: /save/i }))
    await waitFor(() =>
      expect(editorSetTabDirtyMock).toHaveBeenCalledWith(
        SESSION,
        TAB_ID,
        false,
      ),
    )
    expect(toastError).not.toHaveBeenCalled()
  })
})

// (d) image + SVG preview. An image tab must never fetch /read: the server
// refuses anything over the 5 MiB editable cap BEFORE the binary flag exists,
// so a buffer-gated image tab would park on a spinner forever. Instead the
// pane renders straight from /raw. SVG stays a TEXT tab (Monaco) whose
// Preview toggle renders the current draft through a Blob URL.
async function mountWithTab(path: string) {
  const { EditorOverlay } = await import("@/components/EditorOverlay")
  const { getSnapshot } = await import("@/lib/store")
  mockState = {
    ...getSnapshot(),
    editorTarget: { sessionId: SESSION, initialPath: path },
    editorTabs: {
      [SESSION]: {
        tabs: [{ id: TAB_ID, path, dirty: false, preview: false, mode: "file" }],
        activeId: TAB_ID,
      },
    },
  } as DuxState
  render(<EditorOverlay />)
}

describe("image and svg preview", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
    // useObjectUrl needs the createObjectURL pair jsdom does not implement.
    let n = 0
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: vi.fn(() => `blob:test-${++n}`),
      revokeObjectURL: vi.fn(),
    })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("an image tab renders from /raw, never calls /read, and hides Save", async () => {
    await mountWithTab("assets/logo.png")
    const img = await screen.findByAltText("assets/logo.png")
    expect(img.getAttribute("src")).toBe(
      "/api/v1/sessions/s1/files/raw?path=assets%2Flogo.png",
    )
    expect(readMock).not.toHaveBeenCalled()
    // Buffer-derived controls have no buffer to act on.
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull()
    expect(screen.queryByRole("button", { name: /preview/i })).toBeNull()
  })

  it("an svg tab opens as text and its Preview renders the draft as an image", async () => {
    await mountWithTab("icons/logo.svg")
    // SVG is a text tab: /read is fetched and Monaco (the stub) mounts.
    const box = await screen.findByTestId("code-editor")
    await waitFor(() =>
      expect((box as HTMLTextAreaElement).value).toBe(ON_DISK),
    )
    expect(readMock).toHaveBeenCalled()
    // The preview toggle extends beyond markdown to .svg.
    fireEvent.click(screen.getByRole("button", { name: /preview/i }))
    const img = await screen.findByAltText("icons/logo.svg")
    expect(img.getAttribute("src")).toMatch(/^blob:/)
  })
})

// (a) the explorer is a collapsible resizable panel with an explicit header
// toggle. Layout drag behavior belongs to the preview-env visual pass; what
// is pinned here is that the toggle exists, meets the touch floor, and the
// overlay starts expanded when nothing is stored.
describe("file explorer collapse toggle", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("renders in the header, starts expanded, and meets the touch floor", async () => {
    await mountWithTab(PATH)
    const btn = await screen.findByRole("button", {
      name: /hide the file explorer/i,
    })
    expect(btn.className).toContain("max-md:size-10")
    // Expanded means the explorer's search box is on screen.
    expect(screen.getByPlaceholderText("Search files…")).toBeTruthy()
  })
})
