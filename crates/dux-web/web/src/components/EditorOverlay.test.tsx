// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import {
  EXPLORER_DEFAULT_SIZE_PX,
  EXPLORER_LAYOUT_KEY,
  EXPLORER_MIN_SIZE_PX,
} from "@/lib/editorLayout"
import { agentRoot, rootKey } from "@/lib/editorRoot"
import { OPEN_IN_EDITORS } from "@/lib/editors"

// These cases render the whole editor shell and then wait on `waitFor` polls, so
// a busy machine can push a perfectly healthy run past the default per-test
// window and fail on the clock rather than on the assertion. The larger window
// is scoped to this file: it buys nothing anywhere else, and a global raise
// would hide a genuinely hung test in every other file.
vi.setConfig({ testTimeout: 20_000, hookTimeout: 20_000 })

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
// The two ways a tab can be closed, spied so the deleted-file banner's "Close
// tab" can be checked for taking the SAME route the tab strip's own close
// takes: straight through when there is nothing to lose, and through the
// dirty-tab confirm when there is.
const editorCloseTabMock = vi.fn()
const openEditorCloseTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    editorSetTabDirty: editorSetTabDirtyMock,
    closeEditor: (...a: unknown[]) => closeEditorMock(...a),
    editorCloseTab: (...a: unknown[]) => editorCloseTabMock(...a),
    openEditorCloseTab: (...a: unknown[]) => openEditorCloseTabMock(...a),
  }
})

// Resolves with the save route's real success body (the file's fresh stamp),
// which the editor adopts as its new baseline. Tests that care override it.
const writeMock = vi.fn(async (..._a: unknown[]) => ({
  modified: "2026-01-01T00:00:00+00:00",
  size: 0,
}))
const readMock = vi.fn(async (..._a: unknown[]) => ({
  path: PATH,
  content: ON_DISK,
  binary: false,
  read_only: false,
}))
// The freshness check's endpoint. The default answer is deliberately shaped
// like a file whose stamp the buffer does NOT have (the unstamped `readMock`
// above): with no baseline there is nothing to compare, so no check runs and
// the pre-existing suites see no extra requests. The disk-freshness suite at
// the bottom of this file installs a real mock disk behind both.
const infoMock = vi.fn(async (..._a: unknown[]) => ({
  path: PATH,
  kind: "file" as const,
  size: ON_DISK.length,
  modified: "2026-01-01T00:00:00+00:00",
  mode: "644",
  permissions: "rw-r--r--",
  symlink_target: null,
  git: { state: "clean" as const },
}))
// The real endpoint's FileDiffContents shape; per-test overrides set the sides
// the diff-mode preview tests assert against.
const diffMock = vi.fn(async () => ({
  path: PATH,
  original: "",
  modified: "",
  binary: false,
}))

function pending<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

type MockDiff = Awaited<ReturnType<typeof diffMock>>
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
// Only `fileApi` is replaced: the module also exports the error CLASSES the
// editor routes on (`FileApiError`, `FileConflictError`), and a factory that
// dropped them would make an `instanceof` check throw at runtime instead of
// failing a test honestly.
vi.mock("@/lib/fileApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/fileApi")>()),
  fileApi: {
    list: vi.fn(async () => ({ files: [PATH], truncated: false })),
    info: (...a: unknown[]) => infoMock(...a),
    tree: (...a: unknown[]) => (treeMock as unknown as (...x: unknown[]) => unknown)(...a),
    read: (...a: unknown[]) => readMock(...a),
    // The real builder's shape, duplicated here because the module is fully
    // mocked: the image-pane test asserts the exact URL the <img> gets.
    rawUrl: (_root: unknown, path: string) =>
      `/api/v1/sessions/s1/files/raw?path=${encodeURIComponent(path)}`,
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
// Whether the stand-in editor should report a non-empty selection. Monaco is
// the only thing that knows in production, so the stub hands `onReady` a fake
// instance shaped like the one narrow read `EditorBody` makes of it.
let selectionActive = false
function codeEditorStub() {
  return {
    default: ({
      value,
      onChange,
      language,
      onReady,
    }: {
      value: string
      onChange: (v: string) => void
      language?: string
      onReady?: (mon: unknown) => void
    }) => (
      <textarea
        ref={() => {
          // The whole surface `EditorBody` touches on the instance: the
          // selection read the freshness check makes, and the model lookup
          // its disposal effect makes.
          onReady?.({
            Uri: { parse: (p: string) => p },
            editor: {
              getEditors: () => [
                { getSelection: () => ({ isEmpty: () => !selectionActive }) },
              ],
              getModel: () => null,
            },
          })
        }}
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
    editorTarget: { root: agentRoot(SESSION), initialPath: PATH },
    editorTabs: {
      [rootKey(agentRoot(SESSION))]: {
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
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
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
    expect(editorSetTabDirtyMock).not.toHaveBeenCalledWith(agentRoot(SESSION),
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
    writeMock.mockResolvedValue({
      modified: "2026-01-01T00:00:00+00:00",
      size: TYPED.length,
    })
    await mountWithDirtyTab()

    fireEvent.click(screen.getByRole("button", { name: /save/i }))
    await waitFor(() =>
      expect(editorSetTabDirtyMock).toHaveBeenCalledWith(agentRoot(SESSION),
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
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
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
        editorTarget: { root: agentRoot(SESSION), initialPath: PATH },
        editorTabs: {
          [rootKey(agentRoot(SESSION))]: {
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
    editorTarget: { root: agentRoot(SESSION), initialPath: path },
    editorTabs: {
      [rootKey(agentRoot(SESSION))]: {
        tabs: [{ id: TAB_ID, path, dirty: false, preview: false, mode }],
        activeId: TAB_ID,
      },
    },
  } as DuxState
  return render(<EditorOverlay />)
}

// A TERMINAL-rooted editor, for the affordances that must not be there.
const TERMINAL_ROOT = {
  kind: "terminal" as const,
  terminalId: "t1",
  owner: { kind: "standalone" as const },
}

async function mountTerminalRootedTab(path = "notes.md") {
  // `clearAllMocks` clears calls and KEEPS implementations, and earlier
  // describes leave `readMock` and `infoMock` answering their own scenarios.
  // Set both explicitly so this describe is order-independent (the same trap
  // the language-picker describe documents).
  readMock.mockResolvedValue({
    path,
    content: ON_DISK,
    binary: false,
    read_only: false,
  })
  infoMock.mockResolvedValue({
    path,
    kind: "file" as const,
    size: ON_DISK.length,
    modified: "2026-01-01T00:00:00+00:00",
    mode: "644",
    permissions: "rw-r--r--",
    symlink_target: null,
    git: { state: "clean" as const },
  })
  const { EditorOverlay } = await import("@/components/EditorOverlay")
  const { getSnapshot } = await import("@/lib/store")
  mockState = {
    ...getSnapshot(),
    editorTarget: { root: TERMINAL_ROOT, initialPath: path },
    editorTabs: {
      [rootKey(TERMINAL_ROOT)]: {
        tabs: [{ id: TAB_ID, path, dirty: false, preview: false, mode: "file" }],
        activeId: TAB_ID,
      },
    },
  } as DuxState
  return render(<EditorOverlay />)
}

describe("a terminal-rooted editor has no diff view at all", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    installBootStubs()
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it("offers no File/Diff switch, on either the desktop bar or the phone fold", async () => {
    // Absent, not disabled: there is no HEAD behind a terminal's directory and
    // no diff route registered for it, so a disabled control would be
    // promising something that does not exist.
    await mountTerminalRootedTab()
    await screen.findByTestId("code-editor")
    expect(screen.queryByRole("group", { name: "View mode" })).toBeNull()
    fireEvent.click(screen.getByLabelText("More editor actions"))
    await screen.findByRole("menu")
    expect(screen.queryByText("Diff view")).toBeNull()
    expect(screen.queryByText("File view")).toBeNull()
  })

  it("still keeps the switch for an agent root, so the absence above is the root's", async () => {
    await mountWithTab("src/a.ts")
    await screen.findByTestId("code-editor")
    expect(screen.queryByRole("group", { name: "View mode" })).not.toBeNull()
  })
})

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
        [rootKey(agentRoot(SESSION))]: {
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
        [rootKey(agentRoot(SESSION))]: {
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
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
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
        [rootKey(agentRoot(SESSION))]: {
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
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
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

  it("does not duplicate an unresolved diff request on an unrelated rerender", async () => {
    const request = pending<MockDiff>()
    diffMock.mockReturnValue(request.promise)
    const { rerender } = await mountWithTab("src/pending.txt", "diff")
    await waitFor(() => expect(diffMock).toHaveBeenCalledTimes(1))

    const { EditorOverlay } = await import("@/components/EditorOverlay")
    mockState = { ...mockState, offline: !mockState.offline }
    rerender(<EditorOverlay />)
    expect(diffMock).toHaveBeenCalledTimes(1)

    request.resolve({
      path: "src/pending.txt",
      original: "pending at HEAD\n",
      modified: "pending on disk\n",
      binary: false,
    })
    await screen.findByTestId("diff-viewer")
  })

  it("ignores a late diff result after the tab moves to another path", async () => {
    const first = pending<MockDiff>()
    const second = pending<MockDiff>()
    diffMock
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const { rerender } = await mountWithTab("src/a.txt", "diff")
    await waitFor(() => expect(diffMock).toHaveBeenCalledTimes(1))

    mockState = {
      ...mockState,
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: {
          tabs: [
            {
              id: TAB_ID,
              path: "src/b.txt",
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
    await waitFor(() => expect(diffMock).toHaveBeenCalledTimes(2))

    second.resolve({
      path: "src/b.txt",
      original: "b at HEAD\n",
      modified: "b on disk\n",
      binary: false,
    })
    await waitFor(() =>
      expect(screen.getByTestId("diff-viewer").getAttribute("data-original")).toBe(
        "b at HEAD\n",
      ),
    )

    first.resolve({
      path: "src/a.txt",
      original: "late a at HEAD\n",
      modified: "late a on disk\n",
      binary: false,
    })
    await waitFor(() =>
      expect(screen.getByTestId("diff-viewer").getAttribute("data-original")).toBe(
        "b at HEAD\n",
      ),
    )
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
        [rootKey(agentRoot(SESSION))]: {
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

  it("offers the same local editors in the desktop and mobile menus", async () => {
    await mountWithTab(PATH)

    fireEvent.click(screen.getByText("Open local editor").closest("button")!)
    for (const editor of OPEN_IN_EDITORS) {
      expect(await screen.findByRole("menuitem", { name: editor.label })).toBeTruthy()
    }

    fireEvent.keyDown(document, { key: "Escape" })
    fireEvent.click(screen.getByRole("button", { name: "More editor actions" }))
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "Open local editor" }),
    )
    for (const editor of OPEN_IN_EDITORS) {
      expect(await screen.findByRole("menuitem", { name: editor.label })).toBeTruthy()
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
      editorTarget: { root: agentRoot(SESSION), initialPath: null },
      editorTabs: { [rootKey(agentRoot(SESSION))]: { tabs: [], activeId: null } },
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

// A file mutation confirms itself only when its outcome is not already on
// screen. Delete is the sharp one: its dialog closes the moment it is
// confirmed rather than when the request settles, so a silent success would
// leave no trace at all. A create and an in-place rename are the opposite: the
// tree row and the open tab carry the result, so they say nothing.
describe("file mutations confirm themselves when nothing else does", () => {
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

  it("creates without a word, because the new entry is in the tree", async () => {
    await mountWithTab(PATH)
    fireEvent.click(screen.getByRole("button", { name: /new file/i }))
    fireEvent.change(await screen.findByPlaceholderText("example.ts"), {
      target: { value: "new.ts" },
    })
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }))
    await waitFor(() => expect(createFileMock).toHaveBeenCalled())
    expect(toastSuccess).not.toHaveBeenCalled()
    expect(toastError).not.toHaveBeenCalled()
  })

  it("renames without a word, because the row and the tab carry the new name", async () => {
    await rowMenu(/^Rename…$/)
    const field = await screen.findByDisplayValue("notes.md")
    fireEvent.change(field, { target: { value: "notes.txt" } })
    fireEvent.click(screen.getByRole("button", { name: /^rename$/i }))
    await waitFor(() => expect(renameMock).toHaveBeenCalled())
    expect(toastSuccess).not.toHaveBeenCalled()
    expect(toastError).not.toHaveBeenCalled()
  })

  it("still reports a refused create, which the tree cannot show", async () => {
    createFileMock.mockRejectedValueOnce(new Error("permission denied"))
    await mountWithTab(PATH)
    fireEvent.click(screen.getByRole("button", { name: /new file/i }))
    fireEvent.change(await screen.findByPlaceholderText("example.ts"), {
      target: { value: "new.ts" },
    })
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }))
    await waitFor(() => expect(toastError).toHaveBeenCalled())
    expect(String(toastError.mock.calls[0][0])).toContain("permission denied")
    expect(toastSuccess).not.toHaveBeenCalled()
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
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
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

  // The stub above cannot see what the real transition does to a Monaco model
  // (Monaco does not load under vitest at all), and the wrapper's own effect
  // SKIPS an undefined language, which is the bug. The decision that fixes it
  // is `autoRevertLanguageId`, tested un-stubbed in lib/editorLanguage.test.ts;
  // this asserts only that the picker clears the prop, which is its half.
  it("follows a rename, so a correction is not silently reverted", async () => {
    treeMock.mockImplementation(async () => ({
      dir: "",
      entries: [
        {
          name: "big.txt",
          path: PATH,
          is_dir: false,
          is_symlink: false,
          expandable: false,
        },
      ],
    }))
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    const view = await mountWithTab(PATH)
    await screen.findByTestId("code-editor")
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^TOML$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe("toml"),
    )

    // The name appears twice, on the tab pill and on the tree row; the tree is
    // the one with the context menu, so scope the query to the drop surface.
    const tree = await screen.findByTestId("file-tree-drop-surface")
    fireEvent.contextMenu(await within(tree).findByText("big.txt"))
    fireEvent.click(await screen.findByText(/^Rename…$/))
    fireEvent.change(await screen.findByDisplayValue("big.txt"), {
      target: { value: "big.toml" },
    })
    fireEvent.click(screen.getByRole("button", { name: /^rename$/i }))
    await waitFor(() => expect(renameMock).toHaveBeenCalled())

    // The store retargets the TAB; `useDux` is stubbed in this file, so the
    // new path is mirrored in by hand. The override has to have followed, or
    // the file the user just corrected reverts under them.
    mockState = {
      ...mockState,
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: {
          tabs: [
            {
              id: TAB_ID,
              path: "src/big.toml",
              dirty: false,
              preview: false,
              mode: "file" as const,
            },
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    view.rerender(<EditorOverlay />)
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe("toml"),
    )
    treeMock.mockImplementation(async () => ({ dir: "", entries: [] }))
  })

  // The override is documented to last until the file is CLOSED. It was keyed
  // by path and never pruned, so closing the tab and opening the same path
  // again silently re-applied it, and a different file that later took that
  // path inherited it.
  it("dies with the tab, so reopening the path re-infers", async () => {
    const { EditorOverlay } = await import("@/components/EditorOverlay")
    const other = {
      id: "tab-2",
      path: "src/other.txt",
      dirty: false,
      preview: false,
      mode: "file" as const,
    }
    const view = await mountWithTab(PATH)
    // A sibling tab stays open throughout, so the body is never unmounted and
    // the assertion is about PRUNING rather than about losing the whole map.
    mockState = {
      ...mockState,
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: {
          tabs: [
            {
              id: TAB_ID,
              path: PATH,
              dirty: false,
              preview: false,
              mode: "file" as const,
            },
            other,
          ],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    view.rerender(<EditorOverlay />)
    await screen.findByTestId("code-editor")
    fireEvent.click(await picker())
    fireEvent.click(await screen.findByRole("menuitem", { name: /^TOML$/ }))
    await waitFor(() =>
      expect(
        screen.getByTestId("code-editor").getAttribute("data-language"),
      ).toBe("toml"),
    )

    // Close the corrected tab.
    mockState = {
      ...mockState,
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: { tabs: [other], activeId: other.id },
      },
    } as DuxState
    view.rerender(<EditorOverlay />)
    // Reopen the same path as a NEW tab.
    mockState = {
      ...mockState,
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: {
          tabs: [
            other,
            {
              id: "tab-3",
              path: PATH,
              dirty: false,
              preview: false,
              mode: "file" as const,
            },
          ],
          activeId: "tab-3",
        },
      },
    } as DuxState
    view.rerender(<EditorOverlay />)
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

// --- The file changes on disk underneath the editor -------------------------
//
// The bug, reported and reproduced: an agent edits a file the web editor has
// open, and the editor keeps showing the old text forever. Three mechanisms
// stacked to guarantee it (the load effect never re-reads a loaded path, the
// draft cache restores the stale buffer across remounts, and Monaco reattaches
// the retained model), and the save on top of that was unconditional, so
// saving the stale buffer destroyed the agent's work.
//
// These drive the whole journey through the real component with a mock disk
// behind /read and /info, because the interesting part is not any one helper:
// it is which of the three triggers fires, what it does to a clean versus a
// dirty buffer, and what the user is offered when the editor cannot decide.
describe("when the file changes on disk underneath the editor", () => {
  const OTHER = "src/other.txt"
  const AGENT_TEXT = "the agent rewrote this\n"
  const TAB_B = "tab-b"

  // The mock disk: one record per path, mutated mid-test to stand in for the
  // agent's edit. Both /read and /info answer from it, which is what makes the
  // metadata check meaningful (a check that could not disagree with the read
  // would prove nothing).
  let disk: Map<string, { content: string; modified: string; size: number }>

  function put(path: string, content: string, modified: string) {
    disk.set(path, { content, modified, size: content.length })
  }

  function slice(additions: number, deletions: number) {
    return {
      sessionId: SESSION,
      phase: "loaded" as const,
      rev: 1,
      staged: [],
      unstaged: [
        { path: PATH, status: "M", additions, deletions, staged: false },
      ],
      error: null,
    }
  }

  function tab(id: string, path: string, dirty = false) {
    return { id, path, dirty, preview: false, mode: "file" as const }
  }

  function setTabs(
    tabs: ReturnType<typeof tab>[],
    activeId: string,
    changes?: unknown,
  ) {
    mockState = {
      ...mockState,
      ...(changes === undefined ? {} : { changes }),
      editorTabs: { [rootKey(agentRoot(SESSION))]: { tabs, activeId } },
    } as DuxState
  }

  // The component under test, imported once per test so `rerender` has
  // something to hand back in (the suite mocks modules, so it cannot be a
  // top-level import).
  let Overlay: () => React.ReactElement

  async function mountOne(dirty = false) {
    const { getSnapshot } = await import("@/lib/store")
    mockState = {
      ...getSnapshot(),
      editorTarget: { root: agentRoot(SESSION), initialPath: PATH },
      editorTabs: {
        [rootKey(agentRoot(SESSION))]: { tabs: [tab(TAB_ID, PATH, dirty)], activeId: TAB_ID },
      },
    } as DuxState
    const view = render(<Overlay />)
    const box = (await screen.findByTestId("code-editor")) as HTMLTextAreaElement
    await waitFor(() => expect(box.value).toBe(ON_DISK))
    return view
  }

  function editor() {
    return screen.getByTestId("code-editor") as HTMLTextAreaElement
  }

  beforeEach(async () => {
    vi.clearAllMocks()
    installBootStubs()
    const { clearRootDrafts } = await import("@/lib/editorDrafts")
    clearRootDrafts(rootKey(agentRoot(SESSION)))
    Overlay = (await import("@/components/EditorOverlay")).EditorOverlay
    selectionActive = false
    disk = new Map()
    put(PATH, ON_DISK, "2026-01-01T00:00:00+00:00")
    put(OTHER, "other file\n", "2026-01-01T00:00:00+00:00")
    readMock.mockImplementation(async (_sessionId?: unknown, path?: unknown) => {
      const entry = disk.get(String(path))
      if (!entry) throw new Error("no such file")
      return {
        path: String(path),
        content: entry.content,
        binary: false,
        read_only: false,
        modified: entry.modified,
        size: entry.size,
      }
    })
    infoMock.mockImplementation(async (_sessionId?: unknown, path?: unknown) => {
      const entry = disk.get(String(path))
      if (!entry) {
        const { FileApiError } = await import("@/lib/fileApi")
        throw new FileApiError(404, "no such entry in the worktree")
      }
      return {
        path: String(path),
        kind: "file" as const,
        size: entry.size,
        modified: entry.modified,
        mode: "644",
        permissions: "rw-r--r--",
        symlink_target: null,
        git: { state: "clean" as const },
      }
    })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  // The reported journey, end to end.
  it("shows the new content after switching away and back", async () => {
    const view = await mountOne()
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH), tab(TAB_B, OTHER)], TAB_B, slice(9, 1))
    view.rerender(<Overlay />)
    await waitFor(() => expect(editor().value).toBe("other file\n"))

    setTabs([tab(TAB_ID, PATH), tab(TAB_B, OTHER)], TAB_ID)
    view.rerender(<Overlay />)
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
  })

  it("reloads a clean buffer in place as soon as the change signal moves", async () => {
    const view = await mountOne()
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
    // Silently: a buffer with nothing to lose is exactly what the user asked
    // to be kept current, so there is no banner to dismiss.
    expect(screen.queryByRole("status")).toBeNull()
  })

  // The no-op movers. The user's own save moves the signal too, and so does
  // the changes slice merely refetching; neither may cost anything.
  it("checks but does not re-read when the signal moved and the file did not", async () => {
    const view = await mountOne()
    setTabs([tab(TAB_ID, PATH)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await waitFor(() => expect(infoMock).toHaveBeenCalled())
    expect(readMock).toHaveBeenCalledTimes(1)
    expect(editor().value).toBe(ON_DISK)
  })

  it("does not check at all while the changes slice is still loading", async () => {
    const view = await mountOne()
    setTabs([tab(TAB_ID, PATH)], TAB_ID, {
      sessionId: SESSION,
      phase: "loading",
      rev: 1,
      staged: [],
      unstaged: [],
      error: null,
    })
    view.rerender(<Overlay />)
    await waitFor(() => expect(editor().value).toBe(ON_DISK))
    expect(infoMock).not.toHaveBeenCalled()
  })

  // Focus catches what the signal is blind to: a git-ignored file, or an edit
  // that happens to keep the same +/- counts.
  it("catches a change on window focus even when the signal never moves", async () => {
    await mountOne()
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
  })

  it("deduplicates freshness triggers while one check is in flight", async () => {
    const view = await mountOne()
    let releaseInfo: (() => void) | null = null
    const held = new Promise<void>((resolve) => {
      releaseInfo = resolve
    })
    infoMock.mockImplementationOnce(async () => {
      await held
      const entry = disk.get(PATH)!
      return {
        path: PATH,
        kind: "file" as const,
        size: entry.size,
        modified: entry.modified,
        mode: "644",
        permissions: "rw-r--r--",
        symlink_target: null,
        git: { state: "clean" as const },
      }
    })

    fireEvent(window, new Event("focus"))
    fireEvent(window, new Event("focus"))
    setTabs([tab(TAB_ID, PATH)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await waitFor(() => expect(infoMock).toHaveBeenCalledTimes(1))

    releaseInfo!()
    await waitFor(() => expect(editor().value).toBe(ON_DISK))
  })

  it("ignores a late freshness result after the tab moves to another path", async () => {
    const view = await mountOne()
    let releaseInfo: (() => void) | null = null
    const held = new Promise<void>((resolve) => {
      releaseInfo = resolve
    })
    infoMock.mockImplementationOnce(async () => {
      await held
      return {
        path: PATH,
        kind: "file" as const,
        size: AGENT_TEXT.length,
        modified: "2026-02-02T00:00:00+00:00",
        mode: "644",
        permissions: "rw-r--r--",
        symlink_target: null,
        git: { state: "clean" as const },
      }
    })

    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(infoMock).toHaveBeenCalledTimes(1))
    setTabs([tab(TAB_ID, OTHER)], TAB_ID)
    view.rerender(<Overlay />)
    await waitFor(() => expect(editor().value).toBe("other file\n"))

    releaseInfo!()
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())
    expect(editor().value).toBe("other file\n")
    expect(readMock).toHaveBeenCalledTimes(2)
  })

  // The terminal-root case, measured rather than assumed: it gets no
  // changed-files broadcast at all, so if the other two triggers depended on
  // the slice, a terminal editor would never notice a file moving under it.
  it("still catches a change on a TERMINAL root, which has no broadcast", async () => {
    const { getSnapshot } = await import("@/lib/store")
    const terminalRoot = {
      kind: "terminal" as const,
      terminalId: "t1",
      owner: { kind: "standalone" as const },
    }
    mockState = {
      ...getSnapshot(),
      editorTarget: { root: terminalRoot, initialPath: PATH },
      editorTabs: {
        [rootKey(terminalRoot)]: {
          tabs: [tab(TAB_ID, PATH)],
          activeId: TAB_ID,
        },
      },
    } as DuxState
    render(<Overlay />)
    await waitFor(() => expect(editor().value).toBe(ON_DISK))

    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
  })

  it("never silently replaces a buffer with unsaved edits", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)

    const banner = await screen.findByRole("status")
    expect(banner.textContent).toContain("changed on disk")
    expect(editor().value).toBe(TYPED)
    expect(readMock).toHaveBeenCalledTimes(1)
  })

  it("reloads a dirty buffer only after the destructive confirm", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")

    fireEvent.click(screen.getByRole("button", { name: /reload from disk/i }))
    // Cancel first: the escape hatch must actually leave the text alone.
    fireEvent.click(await screen.findByRole("button", { name: /keep my edits/i }))
    expect(editor().value).toBe(TYPED)

    fireEvent.click(screen.getByRole("button", { name: /reload from disk/i }))
    fireEvent.click(await screen.findByRole("button", { name: /discard & reload/i }))
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
  })

  it("lets the user keep their version and dismiss the banner", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")

    fireEvent.click(screen.getByRole("button", { name: /keep mine/i }))
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())
    expect(editor().value).toBe(TYPED)
  })

  // "Keep mine" has to MEAN something. Every window focus runs another check,
  // so without remembering what was dismissed the banner would come straight
  // back and the button would be decoration.
  it("does not raise the same change again once it is dismissed", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")
    fireEvent.click(screen.getByRole("button", { name: /keep mine/i }))
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())

    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(infoMock.mock.calls.length).toBeGreaterThan(1))
    expect(screen.queryByRole("status")).toBeNull()
    expect(editor().value).toBe(TYPED)
  })

  it("raises it again when the file changes a SECOND time", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")
    fireEvent.click(screen.getByRole("button", { name: /keep mine/i }))
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())

    put(PATH, AGENT_TEXT + "and again\n", "2026-03-03T00:00:00+00:00")
    fireEvent(window, new Event("focus"))
    expect(await screen.findByRole("status")).toBeInstanceOf(HTMLElement)
  })

  it("says so when the file was deleted, and offers to close the tab", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    disk.delete(PATH)
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)

    const banner = await screen.findByRole("status")
    expect(banner.textContent).toContain("deleted on disk")
    expect(editor().value).toBe(TYPED)
    expect(
      screen.getByRole("button", { name: /close tab/i }),
    ).toBeInstanceOf(HTMLElement)
  })

  // The data-loss half.
  it("sends the freshness token with a save and re-baselines on the answer", async () => {
    writeMock.mockResolvedValue({
      modified: "2026-03-03T00:00:00+00:00",
      size: TYPED.length,
    })
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    fireEvent.click(screen.getByRole("button", { name: /save/i }))
    await waitFor(() => expect(writeMock).toHaveBeenCalled())
    expect(writeMock.mock.calls[0][3]).toEqual({
      modified: "2026-01-01T00:00:00+00:00",
      size: ON_DISK.length,
    })

    // The save moved the changed-files signal, as the user's own save always
    // does. That must not send the editor chasing its own work: the buffer
    // re-baselined on the write's answer, so the check that follows finds
    // nothing and nothing is re-read.
    put(PATH, TYPED, "2026-03-03T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, false)], TAB_ID, slice(12, 1))
    view.rerender(<Overlay />)
    await waitFor(() => expect(infoMock).toHaveBeenCalled())
    expect(readMock).toHaveBeenCalledTimes(1)
    expect(editor().value).toBe(TYPED)
  })

  it("routes a refused save to the conflict dialog, not to an error toast", async () => {
    const { FileConflictError } = await import("@/lib/fileApi")
    writeMock.mockRejectedValueOnce(
      new FileConflictError({
        modified: "2026-02-02T00:00:00+00:00",
        size: AGENT_TEXT.length,
        deleted: false,
      }),
    )
    await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    fireEvent.click(screen.getByRole("button", { name: /save/i }))

    expect(
      await screen.findByText(/changed after you opened it/i),
    ).toBeInstanceOf(HTMLElement)
    expect(toastError).not.toHaveBeenCalled()
    // The draft is untouched by a refused save, exactly as for every other
    // refusal.
    expect(editor().value).toBe(TYPED)

    // Overwrite re-sends the same body with NO token, which is the only way
    // to mean "yes, I know, do it anyway".
    writeMock.mockResolvedValueOnce({
      modified: "2026-04-04T00:00:00+00:00",
      size: TYPED.length,
    })
    fireEvent.click(screen.getByRole("button", { name: /^overwrite$/i }))
    await waitFor(() => expect(writeMock).toHaveBeenCalledTimes(2))
    expect(writeMock.mock.calls[1][2]).toBe(TYPED)
    expect(writeMock.mock.calls[1][3]).toBeUndefined()
  })

  // The gap between deciding "this buffer is clean, reload it" and the bytes
  // arriving is a whole network round trip, and the user is typing through it.
  // Deciding once, at request time, throws those keystrokes away and leaves
  // the tab reading clean, so the loss is invisible. The decision has to be
  // taken again when the answer lands.
  it("keeps text typed while an in-place reload was in flight", async () => {
    await mountOne()
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")

    // Hold the reload's read open so the keystrokes can land inside it.
    let releaseRead: (() => void) | null = null
    const held = new Promise<void>((resolve) => {
      releaseRead = resolve
    })
    const answer = {
      path: PATH,
      content: AGENT_TEXT,
      binary: false,
      read_only: false,
      modified: "2026-02-02T00:00:00+00:00",
      size: AGENT_TEXT.length,
    }
    readMock.mockImplementationOnce(async () => {
      await held
      return answer
    })

    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(readMock).toHaveBeenCalledTimes(2))

    fireEvent.change(editor(), { target: { value: TYPED } })
    releaseRead!()

    const banner = await screen.findByRole("status")
    expect(banner.textContent).toContain("changed on disk")
    expect(editor().value).toBe(TYPED)
  })

  // The file is GONE, so the buffer is the last copy of the text in existence.
  // Closing the tab from the banner therefore has to ask exactly the way the
  // tab strip's own close does.
  it("asks before closing a deleted file's tab that still holds edits", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    disk.delete(PATH)
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")

    fireEvent.click(screen.getByRole("button", { name: /close tab/i }))
    expect(openEditorCloseTabMock).toHaveBeenCalledWith(agentRoot(SESSION), TAB_ID)
    expect(editorCloseTabMock).not.toHaveBeenCalled()
  })

  it("closes a deleted file's clean tab straight away", async () => {
    const view = await mountOne()
    disk.delete(PATH)
    setTabs([tab(TAB_ID, PATH)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")

    fireEvent.click(screen.getByRole("button", { name: /close tab/i }))
    expect(editorCloseTabMock).toHaveBeenCalledWith(agentRoot(SESSION), TAB_ID)
    expect(openEditorCloseTabMock).not.toHaveBeenCalled()
  })

  // A clean buffer with a live selection gets the banner too, but for a
  // completely different reason, and telling the user they have unsaved edits
  // when they have none is the kind of wrong that makes people distrust the
  // next warning.
  it("says the reload is paused, not that there are unsaved edits, when a selection is active", async () => {
    const view = await mountOne()
    selectionActive = true
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)

    const banner = await screen.findByRole("status")
    expect(banner.textContent).toContain("changed on disk")
    expect(banner.textContent).toContain("selection")
    expect(banner.textContent).not.toContain("unsaved edits")
    expect(editor().value).toBe(ON_DISK)

    // And the offer still works: nothing is dirty, so it reloads with no
    // destructive confirm in the way.
    fireEvent.click(screen.getByRole("button", { name: /reload from disk/i }))
    await waitFor(() => expect(editor().value).toBe(AGENT_TEXT))
  })

  // The interleaving: a check goes out against the pre-save stamp, the save
  // lands and re-baselines, and the check's answer arrives describing the
  // file the buffer now holds. Adopting the signal without clearing the state
  // pins a banner about a change that is the user's own save.
  it("clears a banner when a later check finds the buffer and the disk agree", async () => {
    await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })

    // A check goes out against the pre-save baseline and is held open.
    let releaseInfo: (() => void) | null = null
    const held = new Promise<void>((resolve) => {
      releaseInfo = resolve
    })
    const preSave = {
      path: PATH,
      kind: "file" as const,
      size: ON_DISK.length,
      modified: "2026-01-01T00:00:00+00:00",
      mode: "644",
      permissions: "rw-r--r--",
      symlink_target: null,
      git: { state: "clean" as const },
    }
    infoMock.mockImplementationOnce(async () => {
      await held
      return preSave
    })
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(infoMock).toHaveBeenCalledTimes(1))

    // The save lands underneath it and re-baselines the buffer on the bytes
    // now on disk.
    writeMock.mockResolvedValueOnce({
      modified: "2026-03-03T00:00:00+00:00",
      size: TYPED.length,
    })
    put(PATH, TYPED, "2026-03-03T00:00:00+00:00")
    fireEvent.click(screen.getByRole("button", { name: /save/i }))
    // Wait for the save to have LANDED in the buffer, not merely to have been
    // sent: the re-baseline is what the stale check then disagrees with.
    await waitFor(() =>
      expect(editorSetTabDirtyMock).toHaveBeenCalledWith(agentRoot(SESSION), TAB_ID, false),
    )

    // Now the stale check answers, describing the file as it was BEFORE the
    // save. It disagrees with the new baseline, so a banner goes up about a
    // change that is the user's own save.
    releaseInfo!()
    await screen.findByRole("status")

    // The next check compares the buffer against a disk that matches it
    // exactly. That is the moment the banner has to come down.
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())
  })

  // A symlinked file. The read followed the link and stamped the file it
  // actually read; the info route stats the LINK, on purpose, because the info
  // panel describes the link. Comparing the buffer's stamp against the link's
  // finds a difference every single time, which made every symlinked open file
  // read as permanently stale.
  it("compares a symlinked file against its target, not against the link", async () => {
    await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    const entry = disk.get(PATH)!
    infoMock.mockImplementation(async () => ({
      path: PATH,
      kind: "symlink" as const,
      // The link's own stat: the length of the stored target path, and the
      // moment the link was made. Nothing to do with the bytes on screen.
      size: 20,
      modified: "1999-09-09T00:00:00+00:00",
      mode: "777",
      permissions: "rwxrwxrwx",
      symlink_target: "../elsewhere/big.txt",
      target_modified: entry.modified,
      target_size: entry.size,
      git: { state: "clean" as const },
    }))

    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(infoMock).toHaveBeenCalled())
    // Nothing moved, so nothing is offered and nothing is re-read.
    await waitFor(() => expect(editor().value).toBe(TYPED))
    expect(screen.queryByRole("status")).toBeNull()
    expect(readMock).toHaveBeenCalledTimes(1)
  })

  // The draft cache hands a buffer back across a remount with its disk state
  // still on it. That is deliberate (see `loadRootDrafts`), and it only
  // stays honest because the mount trigger re-checks: a file put back the way
  // the buffer has it must lose the banner.
  it("re-checks a buffer restored from the draft cache on mount", async () => {
    const view = await mountOne(true)
    fireEvent.change(editor(), { target: { value: TYPED } })
    put(PATH, AGENT_TEXT, "2026-02-02T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    view.rerender(<Overlay />)
    await screen.findByRole("status")
    cleanup()

    // Remount onto the same session: the cached buffer comes back with its
    // banner, and the mount check is the only thing that can question it.
    put(PATH, ON_DISK, "2026-01-01T00:00:00+00:00")
    setTabs([tab(TAB_ID, PATH, true)], TAB_ID, slice(9, 1))
    render(<Overlay />)
    await screen.findByTestId("code-editor")
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull())
    expect(editor().value).toBe(TYPED)
  })
})
