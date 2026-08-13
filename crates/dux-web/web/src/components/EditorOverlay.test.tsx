// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import {
  EXPLORER_DEFAULT_SIZE_PX,
  EXPLORER_LAYOUT_KEY,
  EXPLORER_MIN_SIZE_PX,
} from "@/lib/editorLayout"

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
const closeEditorMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    editorSetTabDirty: editorSetTabDirtyMock,
    closeEditor: (...a: unknown[]) => closeEditorMock(...a),
  }
})

const writeMock = vi.fn()
const readMock = vi.fn(async () => ({
  path: PATH,
  content: ON_DISK,
  binary: false,
  read_only: false,
}))
// The real endpoint's FileDiffContents shape; per-test overrides set the sides
// the diff-mode preview tests assert against.
const diffMock = vi.fn(async () => ({
  path: PATH,
  original: "",
  modified: "",
  binary: false,
}))
// The mutation calls, captured so the toast tests can resolve or reject them
// and read what the editor said afterwards. `vi.clearAllMocks` clears calls
// and keeps implementations, so these keep resolving across tests.
const treeMock = vi.fn(async () => ({
  dir: "",
  entries: [] as unknown[],
}))
const createFileMock = vi.fn(async (..._a: unknown[]) => {})
const createDirMock = vi.fn(async (..._a: unknown[]) => {})
const renameMock = vi.fn(async (..._a: unknown[]) => {})
const removeMock = vi.fn(async (..._a: unknown[]) => {})
vi.mock("@/lib/fileApi", () => ({
  fileApi: {
    list: vi.fn(async () => ({ files: [PATH], truncated: false })),
    tree: (...a: unknown[]) => (treeMock as unknown as (...x: unknown[]) => unknown)(...a),
    read: () => readMock(),
    // The real builder's shape, duplicated here because the module is fully
    // mocked: the image-pane test asserts the exact URL the <img> gets.
    rawUrl: (sessionId: string, path: string) =>
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/files/raw?path=${encodeURIComponent(path)}`,
    diff: () => diffMock(),
    write: (...args: unknown[]) => writeMock(...args),
    openInEditor: vi.fn(),
    createFile: (...a: unknown[]) => createFileMock(...a),
    createDir: (...a: unknown[]) => createDirMock(...a),
    rename: (...a: unknown[]) => renameMock(...a),
    remove: (...a: unknown[]) => removeMock(...a),
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
      language,
    }: {
      value: string
      onChange: (v: string) => void
      language?: string
    }) => (
      <textarea
        data-testid="code-editor"
        // The language override the header's picker resolved, surfaced so the
        // picker tests can read what was actually handed to Monaco. Empty
        // means no override, which is "let Monaco infer from the URI".
        data-language={language ?? ""}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    ),
  }
}
vi.mock("@/components/CodeEditor", codeEditorStub)
vi.mock("./CodeEditor", codeEditorStub)

// DiffViewer is Monaco's diff editor and cannot run in jsdom either. The stub
// exposes both sides as data attributes so the diff-mode preview tests can
// assert the diff view is what the tab returns to when Preview toggles off.
function diffViewerStub() {
  return {
    default: ({
      original,
      modified,
      allDelete,
      language,
    }: {
      original: string
      modified: string
      allDelete?: boolean
      language?: string
    }) => (
      <div
        data-testid="diff-viewer"
        data-original={original}
        data-modified={modified}
        data-all-delete={String(allDelete ?? false)}
        data-language={language ?? ""}
      />
    ),
  }
}
vi.mock("@/components/DiffViewer", diffViewerStub)
vi.mock("./DiffViewer", diffViewerStub)

// Panel props recorded at render time, so the panel-unit tests can assert the
// sizes the editor hands react-resizable-panels: pixels for the explorer (so
// the modal and the standalone tab render the same tree) and a percentage for
// the content pane, every one with its unit spelled out. The real Panel still
// renders (spread actual), keeping every other test on the genuine library.
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

// The language registry the picker reads at runtime. The real module pulls
// the multi-MB Monaco bundle and cannot load under vitest at all (see
// lib/pathExt.ts), and EditorBody reaches it through a DYNAMIC import, which
// vi.mock intercepts just the same.
const LANGUAGES = [
  { id: "plaintext", aliases: ["Plain Text", "text"], extensions: [".txt"] },
  { id: "typescript", aliases: ["TypeScript"], extensions: [".ts"] },
  { id: "toml", aliases: ["TOML"], extensions: [".toml"] },
]
vi.mock("@/lib/monacoSetup", () => ({
  monaco: { languages: { getLanguages: () => LANGUAGES } },
  monacoLanguageForPath: () => undefined,
}))

const toastError = vi.fn()
const toastSuccess = vi.fn()
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: (...a: unknown[]) => toastSuccess(...a),
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
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    // The draft cache is module-level and would otherwise carry one test's
    // typed text into the next mount of the same session.
    const { clearSessionDrafts } = await import("@/lib/editorDrafts")
    clearSessionDrafts(SESSION)
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

// (f) closing the editor is non-destructive: drafts move to the module-level
// cache (lib/editorDrafts.ts), so the overlay-close discard dialog is retired
// (Close/Esc/Back close immediately) and reopening restores the typed text
// without another /read.
describe("drafts survive the editor closing", () => {
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

  it("closing with a dirty tab does not prompt, it just closes", async () => {
    await mountWithDirtyTab()
    fireEvent.click(screen.getByRole("button", { name: /^close$/i }))
    // No discard dialog: the close is immediate and non-destructive.
    expect(screen.queryByText(/discard unsaved changes/i)).toBeNull()
    expect(closeEditorMock).toHaveBeenCalledTimes(1)
  })

  it("reopening restores the typed draft without refetching the file", async () => {
    const { unmount } = await (async () => {
      const { EditorOverlay } = await import("@/components/EditorOverlay")
      const { getSnapshot } = await import("@/lib/store")
      mockState = {
        ...getSnapshot(),
        editorTarget: { sessionId: SESSION, initialPath: PATH },
        editorTabs: {
          [SESSION]: {
            tabs: [
              { id: TAB_ID, path: PATH, dirty: true, preview: false, mode: "file" },
            ],
            activeId: TAB_ID,
          },
        },
      } as DuxState
      return render(<EditorOverlay />)
    })()
    const box = (await screen.findByTestId("code-editor")) as HTMLTextAreaElement
    await waitFor(() => expect(box.value).toBe(ON_DISK))
    fireEvent.change(box, { target: { value: TYPED } })
    await waitFor(() => expect(box.value).toBe(TYPED))
    expect(readMock).toHaveBeenCalledTimes(1)

    // Close (unmount the body entirely), then reopen the same session.
    unmount()
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    render(<EditorOverlay />)
    const again = (await screen.findByTestId(
      "code-editor",
    )) as HTMLTextAreaElement
    // The draft is back, from the cache: no second /read was needed (a
    // re-read could silently clobber the unsaved edit).
    await waitFor(() => expect(again.value).toBe(TYPED))
    expect(readMock).toHaveBeenCalledTimes(1)
  })
})

// (d) image + SVG preview. An image tab must never fetch /read: the server
// refuses anything over the 5 MiB editable cap BEFORE the binary flag exists,
// so a buffer-gated image tab would park on a spinner forever. Instead the
// pane renders straight from /raw. SVG stays a TEXT tab (Monaco) whose
// Preview toggle renders the current draft through a Blob URL.
async function mountWithTab(path: string, mode: "file" | "diff" = "file") {
  const { EditorOverlay } = await import("@/components/EditorOverlay")
  const { getSnapshot } = await import("@/lib/store")
  mockState = {
    ...getSnapshot(),
    editorTarget: { sessionId: SESSION, initialPath: path },
    editorTabs: {
      [SESSION]: {
        tabs: [{ id: TAB_ID, path, dirty: false, preview: false, mode }],
        activeId: TAB_ID,
      },
    },
  } as DuxState
  return render(<EditorOverlay />)
}

// The createObjectURL pair jsdom does not implement, installed onto the REAL
// URL class (never a `{ ...URL }` spread global, which would destroy the URL
// constructor for everything else in the render). Created Blobs are captured
// so the SVG draft-accuracy test can read back what was actually rendered.
const createdBlobs: Blob[] = []
let objectUrlCounter = 0
const createObjectURLMock = vi.fn((blob: Blob) => {
  createdBlobs.push(blob)
  objectUrlCounter += 1
  return `blob:test-${objectUrlCounter}`
})
const revokeObjectURLMock = vi.fn()
function installObjectUrlMocks() {
  createdBlobs.length = 0
  objectUrlCounter = 0
  Object.assign(URL, {
    createObjectURL: createObjectURLMock,
    revokeObjectURL: revokeObjectURLMock,
  })
}
function removeObjectUrlMocks() {
  const u = URL as { createObjectURL?: unknown; revokeObjectURL?: unknown }
  delete u.createObjectURL
  delete u.revokeObjectURL
}

describe("image and svg preview", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
    installObjectUrlMocks()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    removeObjectUrlMocks()
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
    // And the File/Diff segmented toggle is hidden: an image has no text to
    // diff, so offering the switch would only lead to the binary-diff dead
    // end the store-side mode coercion exists to prevent.
    expect(screen.queryByRole("group", { name: "View mode" })).toBeNull()
  })

  it("an image tab stuck in diff mode still shows the picture (defense in depth)", async () => {
    // The store coerces image opens to file mode; if a diff-mode image tab
    // reaches the render anyway, the image arm sits ABOVE the diff arm so
    // the picture wins over the binary-diff refusal.
    await mountWithTab("assets/logo.png", "diff")
    await screen.findByAltText("assets/logo.png")
    expect(screen.queryByText(/diffed here/i)).toBeNull()
  })

  it("a failed image load offers Retry, which re-fires the request", async () => {
    await mountWithTab("assets/logo.png")
    const img = await screen.findByAltText("assets/logo.png")
    fireEvent.error(img)
    // Softened copy: the cap is one possibility, not the diagnosed cause.
    expect(screen.getByText(/could not be loaded/i)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: /retry/i }))
    // A fresh <img> element (nonce-keyed) re-fires the request; same URL, no
    // cache-busting param (/raw already sends no-cache).
    const again = await screen.findByAltText("assets/logo.png")
    expect(again).not.toBe(img)
    expect(again.getAttribute("src")).toBe(
      "/api/v1/sessions/s1/files/raw?path=assets%2Flogo.png",
    )
  })

  it("the caption shows the pixel dimensions once the image loads", async () => {
    await mountWithTab("assets/logo.png")
    const img = (await screen.findByAltText("assets/logo.png")) as HTMLImageElement
    // jsdom never decodes images; stamp the natural size and fire load.
    Object.defineProperty(img, "naturalWidth", { value: 640 })
    Object.defineProperty(img, "naturalHeight", { value: 480 })
    fireEvent.load(img)
    expect(screen.getByText(/640\s*×\s*480/)).toBeTruthy()
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

  it("the svg preview renders the EDITED draft, not the loaded file", async () => {
    // The plan's core SVG decision: preview parity with markdown means the
    // Blob is built from the CURRENT DRAFT, unsaved edits included. This
    // pins it against a draft-to-loaded regression.
    const EDITED = '<svg xmlns="http://www.w3.org/2000/svg"><g/></svg>'
    await mountWithTab("icons/logo.svg")
    const box = await screen.findByTestId("code-editor")
    await waitFor(() =>
      expect((box as HTMLTextAreaElement).value).toBe(ON_DISK),
    )
    fireEvent.change(box, { target: { value: EDITED } })
    fireEvent.click(screen.getByRole("button", { name: /preview/i }))
    await screen.findByAltText("icons/logo.svg")
    const lastBlob = createdBlobs[createdBlobs.length - 1]
    expect(lastBlob.type).toBe("image/svg+xml")
    expect(await lastBlob.text()).toBe(EDITED)
  })

  it("preview-replacing an image tab back to text refetches (no stale buffer)", async () => {
    // README -> logo.png -> README on the SAME tab id: the image early-return
    // must also drop the tab's stale buffer, or the return trip would render
    // the old buffer without a refetch.
    const { rerender } = await mountWithTab(PATH)
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    await screen.findByTestId("code-editor")
    await waitFor(() => expect(readMock).toHaveBeenCalledTimes(1))

    mockState = {
      ...mockState,
      editorTabs: {
        [SESSION]: {
          tabs: [
            {
              id: TAB_ID,
              path: "assets/logo.png",
              dirty: false,
              preview: true,
              mode: "file",
            },
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    rerender(<EditorOverlay />)
    await screen.findByAltText("assets/logo.png")

    mockState = {
      ...mockState,
      editorTabs: {
        [SESSION]: {
          tabs: [
            { id: TAB_ID, path: PATH, dirty: false, preview: true, mode: "file" },
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    rerender(<EditorOverlay />)
    await screen.findByTestId("code-editor")
    await waitFor(() => expect(readMock).toHaveBeenCalledTimes(2))
  })
})

// Preview in DIFF mode: a previewable file (markdown, SVG) opened straight
// into diff mode (e.g. clicking a changed file in the Changes pane) offers
// the same Preview toggle as file mode, rendering the END STATE of the file:
// the unsaved draft when the tab has one, else the diff's MODIFIED side (the
// file as on disk). Toggling off returns to the diff; the tab's mode never
// changes. A diff tab may have NO file buffer at all (diff mode never calls
// /read), so the toggle gates on the diff being loaded, not on fileReady.
describe("preview in diff mode", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    installObjectUrlMocks()
    const { clearSessionDrafts } = await import("@/lib/editorDrafts")
    clearSessionDrafts(SESSION)
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    removeObjectUrlMocks()
  })

  it("a markdown diff tab offers Preview immediately and renders the MODIFIED side", async () => {
    diffMock.mockResolvedValue({
      path: "README.md",
      original: "# The old heading\n",
      modified: "# From the modified side\n",
      binary: false,
    })
    await mountWithTab("README.md", "diff")
    await screen.findByTestId("diff-viewer")
    // Diff mode never loads a file buffer; the toggle must not wait on one.
    expect(readMock).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: /preview/i }))
    // The end state renders: the modified side, never the original.
    await screen.findByText("From the modified side")
    expect(screen.queryByText("The old heading")).toBeNull()
  })

  it("an svg diff tab renders a blob URL built from the MODIFIED side", async () => {
    const MODIFIED = '<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>'
    diffMock.mockResolvedValue({
      path: "icons/logo.svg",
      original: "<svg/>",
      modified: MODIFIED,
      binary: false,
    })
    await mountWithTab("icons/logo.svg", "diff")
    await screen.findByTestId("diff-viewer")
    fireEvent.click(screen.getByRole("button", { name: /preview/i }))
    const img = await screen.findByAltText("icons/logo.svg")
    expect(img.getAttribute("src")).toMatch(/^blob:/)
    const lastBlob = createdBlobs[createdBlobs.length - 1]
    expect(lastBlob.type).toBe("image/svg+xml")
    expect(await lastBlob.text()).toBe(MODIFIED)
  })

  it("a dirty draft wins over the modified side", async () => {
    diffMock.mockResolvedValue({
      path: "README.md",
      original: "# The old heading\n",
      modified: "# From the modified side\n",
      binary: false,
    })
    // Edit in file mode first (diff mode never loads the buffer the draft
    // lives in), then flip the tab to diff mode as the store would.
    const { rerender } = await mountWithTab("README.md", "file")
    const box = (await screen.findByTestId(
      "code-editor",
    )) as HTMLTextAreaElement
    await waitFor(() => expect(box.value).toBe(ON_DISK))
    fireEvent.change(box, { target: { value: "# The unsaved draft\n" } })
    await waitFor(() => expect(box.value).toBe("# The unsaved draft\n"))

    mockState = {
      ...mockState,
      editorTabs: {
        [SESSION]: {
          tabs: [
            {
              id: TAB_ID,
              path: "README.md",
              dirty: true,
              preview: false,
              mode: "diff",
            },
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    rerender(<EditorOverlay />)
    await screen.findByTestId("diff-viewer")

    fireEvent.click(screen.getByRole("button", { name: /preview/i }))
    await screen.findByText("The unsaved draft")
    expect(screen.queryByText("From the modified side")).toBeNull()
  })

  it("toggling Preview off returns to the diff view, not the editor", async () => {
    diffMock.mockResolvedValue({
      path: "README.md",
      original: "",
      modified: "# The rendered body\n",
      binary: false,
    })
    await mountWithTab("README.md", "diff")
    await screen.findByTestId("diff-viewer")
    const toggle = screen.getByRole("button", { name: /preview/i })
    fireEvent.click(toggle)
    await screen.findByText("The rendered body")
    expect(screen.queryByTestId("diff-viewer")).toBeNull()
    // Off again: back to the diff, never the Monaco editor (the tab's mode
    // was never changed by previewing).
    fireEvent.click(toggle)
    await screen.findByTestId("diff-viewer")
    expect(screen.queryByTestId("code-editor")).toBeNull()
  })

  it("a non-previewable file in diff mode still has no Preview toggle", async () => {
    diffMock.mockResolvedValue({
      path: PATH,
      original: "a\n",
      modified: "b\n",
      binary: false,
    })
    await mountWithTab(PATH, "diff")
    await screen.findByTestId("diff-viewer")
    expect(screen.queryByRole("button", { name: /preview/i })).toBeNull()
  })
})

// A DELETED file clicked in the Changes pane opens the editor in diff mode.
// The diff must render as a deletion (HEAD content vs an empty modified side),
// and every way the load can fail must SETTLE into a visible state with a
// Retry action — never a permanent spinner. File mode on the same path keeps
// the existing fileError + Retry arm.
describe("a deleted file in the editor", () => {
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

  it("a deleted file's diff renders the DiffViewer as an all-delete diff", async () => {
    diffMock.mockResolvedValue({
      path: "src/gone.txt",
      original: "the content at HEAD\n",
      modified: "",
      binary: false,
    })
    await mountWithTab("src/gone.txt", "diff")
    const viewer = await screen.findByTestId("diff-viewer")
    expect(viewer.getAttribute("data-original")).toBe("the content at HEAD\n")
    expect(viewer.getAttribute("data-modified")).toBe("")
    // Diff mode never fetches /read, so a missing working copy can't error it.
    expect(readMock).not.toHaveBeenCalled()
    // The phantom-inserted-line suppression is armed: the wrapper carries the
    // CSS marker class and the viewer gets the option-level flag.
    expect(viewer.closest(".dux-diff-all-delete")).not.toBeNull()
    expect(viewer.getAttribute("data-all-delete")).toBe("true")
  })

  it("a normal modified file's diff arms no all-delete suppression", async () => {
    diffMock.mockResolvedValue({
      path: "src/kept.txt",
      original: "old\n",
      modified: "new\n",
      binary: false,
    })
    await mountWithTab("src/kept.txt", "diff")
    const viewer = await screen.findByTestId("diff-viewer")
    expect(viewer.closest(".dux-diff-all-delete")).toBeNull()
    expect(viewer.getAttribute("data-all-delete")).toBe("false")
  })

  it("a rejecting diff fetch settles into an error with a Retry action", async () => {
    diffMock.mockRejectedValue(
      new Error("file not found in the worktree or at HEAD: src/gone.txt"),
    )
    await mountWithTab("src/gone.txt", "diff")
    await screen.findByText(/file not found in the worktree or at HEAD/i)
    const callsAfterSettle = diffMock.mock.calls.length

    // Retry re-fires the diff request; on success the viewer renders.
    diffMock.mockResolvedValue({
      path: "src/gone.txt",
      original: "the content at HEAD\n",
      modified: "",
      binary: false,
    })
    fireEvent.click(screen.getByRole("button", { name: /retry/i }))
    await screen.findByTestId("diff-viewer")
    expect(diffMock.mock.calls.length).toBeGreaterThan(callsAfterSettle)
  })

  it("preview-replacing a diff tab onto another path loads the new diff", async () => {
    // The Changes-pane flow that reuses a tab id: open file A in diff mode,
    // then the store preview-replaces the SAME tab onto path B (rule 2 in
    // lib/editorTabs.ts). The tab's cached buffer still carries A's path, and
    // the diff fetch for B must not be dropped on that stale buffer — that
    // drop is a permanent spinner (nothing re-triggers the load effect).
    diffMock.mockResolvedValue({
      path: "src/a.txt",
      original: "a at HEAD\n",
      modified: "a on disk\n",
      binary: false,
    })
    const { rerender } = await mountWithTab("src/a.txt", "diff")
    const viewer = await screen.findByTestId("diff-viewer")
    expect(viewer.getAttribute("data-original")).toBe("a at HEAD\n")

    diffMock.mockResolvedValue({
      path: "src/gone.txt",
      original: "b at HEAD\n",
      modified: "",
      binary: false,
    })
    mockState = {
      ...mockState,
      editorTabs: {
        [SESSION]: {
          tabs: [
            {
              id: TAB_ID,
              path: "src/gone.txt",
              dirty: false,
              preview: true,
              mode: "diff",
            },
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    rerender(<EditorOverlay />)
    await waitFor(() => {
      const v = screen.getByTestId("diff-viewer")
      expect(v.getAttribute("data-original")).toBe("b at HEAD\n")
      expect(v.getAttribute("data-modified")).toBe("")
    })
  })

  it("file mode on a deleted path settles into the fileError arm with Retry", async () => {
    readMock.mockRejectedValue(
      new Error("file not found: src/gone.txt") as never,
    )
    await mountWithTab("src/gone.txt", "file")
    await screen.findByText(/file not found: src\/gone\.txt/i)
    expect(screen.getByRole("button", { name: /retry/i })).toBeTruthy()
  })
})

// The editor's explorer is sized in PIXELS and the content pane in percent,
// each with its unit spelled out, so the modal (capped at min(80rem,
// 100%-2rem)) and the standalone tab (uncapped) render the SAME tree: 22% was
// ~281px in one and ~563px in the other. The matching assertion for the other
// shell lives in StandaloneEditor.test.tsx, against the same constants.
//
// And each panel's inner wrapper (the div the library gives `overflow: auto`)
// must be clipped to `hidden` so the only scroll surface in the content pane
// is Monaco's own (and each preview pane's own); the wrapper's auto scrollbars
// are what stacked nested scrollbars around the diff view. The pixel truth of
// both is the preview-env screenshot pass; what is pinned here is the contract
// the components hand the library.
describe("editor panel units and scroll surfaces", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    recordedPanelProps.length = 0
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("sizes the explorer in pixels and the content pane in percent", async () => {
    await mountWithTab(PATH)
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    const content = recordedPanelProps.find((p) => p.id === "editor-content")
    expect(explorer).toBeTruthy()
    expect(content).toBeTruthy()
    expect(explorer!.defaultSize).toBe(`${EXPLORER_DEFAULT_SIZE_PX}px`)
    expect(explorer!.minSize).toBe(`${EXPLORER_MIN_SIZE_PX}px`)
    expect(content!.minSize).toBe("30%")
    // A percentage here is the bug: it is a different width in each shell.
    expect(String(explorer!.defaultSize)).not.toContain("%")
    // And the width must not rescale when the group does, or it would be
    // container-relative again by another route.
    expect(explorer!.groupResizeBehavior).toBe("preserve-pixel-size")
  })

  it("restores a persisted pixel width, so a resize in either shell carries over", async () => {
    localStorage.setItem(
      EXPLORER_LAYOUT_KEY,
      JSON.stringify({ px: 420, collapsed: false }),
    )
    await mountWithTab(PATH)
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    expect(explorer!.defaultSize).toBe("420px")
  })

  it("ignores a layout left behind by the percentage era instead of exploding", async () => {
    localStorage.setItem(
      EXPLORER_LAYOUT_KEY,
      JSON.stringify({ "editor-explorer": 22, "editor-content": 78 }),
    )
    await mountWithTab(PATH)
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    expect(explorer!.defaultSize).toBe(`${EXPLORER_DEFAULT_SIZE_PX}px`)
  })

  it("mounts collapsed when that is what was persisted, keeping the width to reopen at", async () => {
    localStorage.setItem(
      EXPLORER_LAYOUT_KEY,
      JSON.stringify({ px: 420, collapsed: true }),
    )
    await mountWithTab(PATH)
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    // The width survives the collapse (it is what reopening restores); the
    // collapse itself is carried by the group's mount layout, which the
    // toggle's accessible name reflects from the first frame.
    expect(explorer!.defaultSize).toBe("420px")
    expect(
      await screen.findByRole("button", { name: /show the file explorer/i }),
    ).toBeTruthy()
  })

  it("clips both panel wrappers so panes own their scrolling", async () => {
    await mountWithTab(PATH)
    // The overlay Dialog portals to document.body, so query the document.
    const panels = document.querySelectorAll("[data-panel]")
    expect(panels.length).toBe(2)
    for (const panel of panels) {
      const wrapper = panel.firstElementChild as HTMLElement
      expect(wrapper.style.overflow).toBe("hidden")
    }
  })
})

// The two "open" controls in the header sit next to each other and do
// completely different things: one spawns a GUI editor on the machine dux
// runs on, the other opens a link in a new browser tab. Both carried the
// external-link arrow, so the pair read as one control accidentally
// duplicated. The link keeps the arrow, which is what the arrow means on the
// web; the local spawn takes a laptop.
describe("the header's two open controls are told apart by their icons", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("gives them different icons, and the arrow to the one that is a link", async () => {
    await mountWithTab(PATH)
    const local = screen.getByText("Open local editor").closest("button")!
    const newTab = screen.getByText("Open in new tab").closest("a")!
    // Exact class match, not a substring: lucide names are prefixes of each
    // other ("lucide-laptop" of "lucide-laptop-minimal").
    expect(local.querySelector("svg.lucide-laptop")).toBeTruthy()
    expect(local.querySelector("svg.lucide-external-link")).toBeNull()
    expect(newTab.querySelector("svg.lucide-external-link")).toBeTruthy()
    expect(newTab.querySelector("svg.lucide-laptop")).toBeNull()
  })
})

// The header row must not change height as its controls come and go: the
// File/Diff segmented control (an h-7 button inside p-0.5 + border) is the
// tallest thing the row can hold, and without a floor the row shrinks when no
// file is open and jumps when one opens. jsdom cannot measure pixels, so what
// is pinned is the min-h floor class; the pixel truth is the screenshot pass.
describe("editor header keeps a stable height", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  async function headerRow(): Promise<HTMLElement> {
    const btn = await screen.findByRole("button", {
      name: /the file explorer/i,
    })
    const row = btn.closest("div.border-b") as HTMLElement
    expect(row).toBeTruthy()
    return row
  }

  it("carries the min-h floor with a file open (Save present)", async () => {
    await mountWithTab(PATH)
    await screen.findByRole("button", { name: /save/i })
    const row = await headerRow()
    expect(row.className).toContain("min-h-12.75")
  })

  it("carries the same floor with no file open (Save absent)", async () => {
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    const { getSnapshot } = await import("@/lib/store")
    mockState = {
      ...getSnapshot(),
      editorTarget: { sessionId: SESSION, initialPath: null },
      editorTabs: { [SESSION]: { tabs: [], activeId: null } },
    } as DuxState
    render(<EditorOverlay />)
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull()
    const row = await headerRow()
    expect(row.className).toContain("min-h-12.75")
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

  it("offers Open in new tab as a labeled anchor to the standalone address", async () => {
    await mountWithTab(PATH)
    // A visible label, not an icon-only button: the accessible name IS the
    // on-screen text, and the control is a real anchor so middle-click and
    // ctrl/cmd-click keep their native semantics.
    const link = await screen.findByRole("link", { name: /open in new tab/i })
    expect(link.getAttribute("href")).toContain("#/editor/agent/")
    expect(link.getAttribute("target")).toBe("_blank")
    expect(link.getAttribute("rel")).toBe("noopener")
  })

  it("renders in the header, starts expanded, and meets the touch floor", async () => {
    await mountWithTab(PATH)
    const btn = await screen.findByRole("button", {
      name: /hide the file explorer/i,
    })
    expect(btn.className).toContain("max-md:size-10")
    // The control's state is carried by its CHANGING accessible name
    // (Hide/Show); aria-pressed on top of a changing name would contradict
    // it, so it must not be present.
    expect(btn.getAttribute("aria-pressed")).toBeNull()
    // Expanded means the explorer's search box is on screen.
    expect(screen.getByPlaceholderText("Search files…")).toBeTruthy()
  })
})

// A file drop that MISSES the file tree must not navigate the tab away.
//
// The browser's default action for a dropped file is to open it, which
// discards the whole SPA and every unsaved in-memory buffer with it. The
// editor now invites exactly this drag (the tree takes file drops), so a drop
// landing on Monaco, the tab strip or the panel chrome is an ordinary
// near-miss rather than an exotic one. `lib/editorDrafts.ts` puts a
// `beforeunload` prompt in the way, so the work is not lost silently, but a
// prompt the user has to answer is not the feature.
describe("a file drop that misses the tree", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  // What the browser reads to decide whether to handle the drop itself:
  // whether anything called preventDefault.
  function fileDrag() {
    return { dataTransfer: { types: ["Files"], files: [], items: [] } }
  }

  it("is cancelled by the overlay, so the browser never opens the file", async () => {
    await mountWithTab(PATH)
    // The overlay's own surface. Anything inside it that does not claim the
    // drop itself bubbles here.
    const surface = await screen.findByRole("dialog")

    const over = fireEvent.dragOver(surface, fileDrag())
    const dropped = fireEvent.drop(surface, fileDrag())
    // `fireEvent` returns false when a handler called preventDefault, which is
    // the whole signal: with no handler the browser takes the drop and
    // navigates.
    expect(over).toBe(false)
    expect(dropped).toBe(false)
  })

  it("leaves an in-app drag alone, so nothing else is disturbed", async () => {
    await mountWithTab(PATH)
    const surface = await screen.findByRole("dialog")
    const textDrag = {
      dataTransfer: { types: ["text/plain"], files: [], items: [] },
    }
    expect(fireEvent.dragOver(surface, textDrag)).toBe(true)
    expect(fireEvent.drop(surface, textDrag)).toBe(true)
  })
})

// Every file mutation the editor performs now CONFIRMS itself. They used to
// land in silence: the dialog closed, the tree refetched, and nothing said
// what had happened. Delete is the sharp one, because its dialog closes the
// moment it is confirmed rather than when the request settles, so a
// successful delete left no trace on screen at all.
describe("file mutations confirm themselves", () => {
  const ENTRY = {
    name: "notes.md",
    path: "notes.md",
    is_dir: false,
    is_symlink: false,
    expandable: false,
  }

  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    // The tree mock's implementation survives clearAllMocks, so put the
    // default back for the rest of the file.
    treeMock.mockImplementation(async () => ({ dir: "", entries: [] }))
  })

  // Opens the tree row's context menu and clicks one of its items.
  async function rowMenu(item: RegExp): Promise<void> {
    treeMock.mockImplementation(async () => ({ dir: "", entries: [ENTRY] }))
    await mountWithTab(PATH)
    const row = await screen.findByText("notes.md")
    fireEvent.contextMenu(row)
    fireEvent.click(await screen.findByText(item))
  }

  it("says what it created, naming the file", async () => {
    await mountWithTab(PATH)
    fireEvent.click(screen.getByRole("button", { name: /new file/i }))
    fireEvent.change(await screen.findByPlaceholderText("example.ts"), {
      target: { value: "new.ts" },
    })
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }))
    await waitFor(() => expect(createFileMock).toHaveBeenCalled())
    await waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        "Created file new.ts",
        expect.anything(),
      ),
    )
    expect(toastSuccess).toHaveBeenCalledTimes(1)
    expect(toastError).not.toHaveBeenCalled()
  })

  it("says what it renamed, and to what", async () => {
    await rowMenu(/^Rename…$/)
    const field = await screen.findByDisplayValue("notes.md")
    fireEvent.change(field, { target: { value: "notes.txt" } })
    fireEvent.click(screen.getByRole("button", { name: /^rename$/i }))
    await waitFor(() => expect(renameMock).toHaveBeenCalled())
    await waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        "Renamed notes.md to notes.txt",
        expect.anything(),
      ),
    )
    expect(toastSuccess).toHaveBeenCalledTimes(1)
  })

  it("says what it deleted, even though the dialog is already gone", async () => {
    await rowMenu(/^Delete…$/)
    fireEvent.click(await screen.findByRole("button", { name: /^delete$/i }))
    await waitFor(() => expect(removeMock).toHaveBeenCalled())
    await waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        "Deleted file notes.md",
        expect.anything(),
      ),
    )
    expect(toastSuccess).toHaveBeenCalledTimes(1)
    expect(toastError).not.toHaveBeenCalled()
  })

  it("a refused delete still reports the failure, and never claims success", async () => {
    removeMock.mockRejectedValueOnce(new Error("permission denied"))
    await rowMenu(/^Delete…$/)
    fireEvent.click(await screen.findByRole("button", { name: /^delete$/i }))
    await waitFor(() => expect(toastError).toHaveBeenCalled())
    expect(String(toastError.mock.calls[0][0])).toContain("permission denied")
    expect(toastSuccess).not.toHaveBeenCalled()
  })
})

// The language picker. Monaco's inference from the file's URI is unchanged
// and remains the default; this is the per-file escape hatch for the file it
// guesses wrong, which for a ".lock" or an extensionless script is every
// time. The override is session-lived and keyed by path.
describe("the header's language picker", () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    // The draft cache is module-level and outlives a render, so an earlier
    // describe's settled fileError for this session would be seeded straight
    // back in and the pane would never reach the editor arm at all.
    const { clearSessionDrafts } = await import("@/lib/editorDrafts")
    clearSessionDrafts(SESSION)
    // `clearAllMocks` clears calls and KEEPS implementations, and earlier
    // describes in this file leave `readMock` rejecting and `diffMock`
    // returning their own sides. Put both back, or the panes never reach the
    // arm that renders an editor at all.
    readMock.mockResolvedValue({
      path: PATH,
      content: ON_DISK,
      binary: false,
      read_only: false,
    })
    diffMock.mockResolvedValue({
      path: PATH,
      original: "a\n",
      modified: "b\n",
      binary: false,
    })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  async function picker(): Promise<HTMLElement> {
    return await screen.findByRole("button", { name: /^syntax language:/i })
  }

  it("names the language Monaco would infer, with no override in force", async () => {
    // src/big.txt: the registry claims ".txt" as plaintext.
    await mountWithTab(PATH)
    await screen.findByTestId("code-editor")
    expect((await picker()).textContent).toContain("Plain text")
    // And nothing is forced on the editor: the prop is absent, so Monaco's
    // own URI inference is what decides.
    expect(
      screen.getByTestId("code-editor").getAttribute("data-language"),
    ).toBe("")
  })

  it("applies a pick to the open editor, and says so on the trigger", async () => {
    await mountWithTab(PATH)
    await screen.findByTestId("code-editor")
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^TOML$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe("toml"),
    )
    expect((await picker()).textContent).toContain("TOML")
  })

  it("Auto clears the override and hands the file back to Monaco", async () => {
    await mountWithTab(PATH)
    await screen.findByTestId("code-editor")
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^TOML$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe("toml"),
    )
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^Auto$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe(""),
    )
    expect((await picker()).textContent).toContain("Plain text")
  })

  it("applies to the diff view too, which is just as wrong about the file", async () => {
    await mountWithTab(PATH, "diff")
    await screen.findByTestId("diff-viewer")
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^TOML$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("diff-viewer").getAttribute("data-language"),
      ).toBe("toml"),
    )
  })

  it("does not render for an image tab, which has no language to pick", async () => {
    await mountWithTab("assets/logo.png")
    await screen.findByRole("img")
    expect(
      screen.queryByRole("button", { name: /^syntax language:/i }),
    ).toBeNull()
  })
})
