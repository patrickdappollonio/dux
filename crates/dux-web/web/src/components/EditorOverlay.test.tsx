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

// Panel props recorded at render time, so the panel-unit tests can assert the
// editor hands react-resizable-panels STRING percentages. v4 reads a bare
// number as PIXELS (defaultSize={22} mounted the explorer ~22px wide), so a
// future bare number must fail here. The real Panel still renders (spread
// actual), keeping every other test on the genuine library.
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

// The editor's panel sizes must be STRING percentages: react-resizable-panels
// v4 treats a bare number as PIXELS, which is the "explorer opens ~20px wide"
// bug. And each panel's inner wrapper (the div the library gives
// `overflow: auto`) must be clipped to `hidden` so the only scroll surface in
// the content pane is Monaco's own (and each preview pane's own); the
// wrapper's auto scrollbars are what stacked nested scrollbars around the
// diff view. The pixel truth of both is the preview-env screenshot pass; what
// is pinned here is the contract the components hand the library.
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

  it("hands the panel library string percentages, never bare numbers", async () => {
    await mountWithTab(PATH)
    const explorer = recordedPanelProps.find((p) => p.id === "editor-explorer")
    const content = recordedPanelProps.find((p) => p.id === "editor-content")
    expect(explorer).toBeTruthy()
    expect(content).toBeTruthy()
    // A bare number here is pixels and re-opens the sliver-explorer bug.
    expect(explorer!.defaultSize).toBe("22%")
    expect(explorer!.minSize).toBe("12%")
    expect(content!.minSize).toBe("30%")
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
