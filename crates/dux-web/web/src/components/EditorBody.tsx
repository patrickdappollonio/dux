// The editor BODY: the header row, the tab strip, the explorer panel, and
// the Monaco/diff/preview panes, extracted from EditorOverlay so two shells
// can compose it: the desktop overlay Dialog (EditorOverlay.tsx) and the
// standalone whole-tab surface (StandaloneEditor.tsx). Exactly one of the
// two mounts it at a time (the overlay stands down while the tab is the
// standalone surface), so there is never a second Monaco model set or a
// second buffer map over the same files.
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react"
import {
  Check,
  ChevronDown,
  CircleAlert,
  Ellipsis,
  ExternalLink,
  Eye,
  FileCode2,
  FilePlus,
  FileText,
  GitCompare,
  Loader2,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  RotateCw,
  Save,
  Search,
  X,
} from "lucide-react"
import { useDefaultLayout } from "react-resizable-panels"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { notify, notifyBusy, notifyError, notifySuccess, notifyWarning } from "@/lib/notify"
import { fileApi } from "@/lib/fileApi"
import { OPEN_IN_EDITORS } from "@/lib/editors"
import {
  emptyBuffer,
  fileLoadSeedBuffer,
  isBufferStale,
  pruneByIds,
  pruneSetByIds,
  shouldSkipFileLoad,
  unionRevalidateBatch,
} from "@/lib/editorBuffers"
import type { TabBuffer } from "@/lib/editorBuffers"
import { isAllDeleteDiff } from "@/lib/diffPresentation"
import {
  loadSessionDrafts,
  storeSessionDrafts,
} from "@/lib/editorDrafts"
import {
  hasDirtyUnderPath,
  saveResolutionOutcome,
  shouldPromoteOnEdit,
} from "@/lib/editorTabs"
import {
  joinName,
  parentDir,
  renameTarget as computeRenameTarget,
} from "@/lib/fileTreeOps"
import { performMove } from "@/lib/moveEntry"
import { isLocalAccessHost } from "@/lib/localAccess"
import {
  EDITOR_CONTENT_MIN_SIZE_PROP,
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  editorMountLayout,
  EXPLORER_DEFAULT_SIZE_PROP,
  EXPLORER_MIN_SIZE_PROP,
  EXPLORER_PANEL_ID,
  explorerExpandTarget,
  isExplorerCollapsed,
  lastExpandedExplorerSize,
  sanitizeEditorLayout,
} from "@/lib/editorLayout"
import { isImagePreviewPath, previewKind } from "@/lib/editorPreview"
import { performTreeDrop } from "@/lib/editorDrop"
import type { DroppedItems } from "@/lib/editorDrop"
import { nextFileDropToastId } from "@/lib/fileDrop"
import { uploadDroppedFile } from "@/lib/fileDropApi"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { useObjectUrl } from "@/hooks/use-object-url"
import type { MonacoInstance } from "@/components/CodeEditor"
import { DeleteEntryDialog } from "@/components/DeleteEntryDialog"
import type { DeleteEntryTarget } from "@/components/DeleteEntryDialog"
import { EditorIcon } from "@/components/EditorIcon"
import { EditorTabsStrip } from "@/components/EditorTabsStrip"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { Button } from "@/components/ui/button"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { FileTree } from "@/components/FileTree"
import { FileInfoDialog } from "@/components/FileInfoDialog"
import type { FileInfoTarget } from "@/components/FileInfoDialog"
import { MoveEntryDialog } from "@/components/MoveEntryDialog"
import type { MoveEntryTarget } from "@/components/MoveEntryDialog"
import { NewEntryDialog } from "@/components/NewEntryDialog"
import type { NewEntryTarget } from "@/components/NewEntryDialog"
import { RenameEntryDialog } from "@/components/RenameEntryDialog"
import type { RenameEntryTarget } from "@/components/RenameEntryDialog"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  closeEditor,
  editorCloseTabsUnderPath,
  editorOpenFile,
  editorPinTab,
  editorRenameTabPaths,
  editorSetTabDirty,
  editorSetTabMode,
  editorSyncActiveTab,
  standaloneEditorHash,
  useDux,
} from "@/lib/store"

// Monaco is multiple MB; keep both surfaces off the main bundle by loading them
// only when the editor actually opens. They share the self-host bootstrap
// (lib/monacoSetup), so the heavy monaco chunk is loaded once for both.
const CodeEditor = lazy(() => import("./CodeEditor"))
const DiffViewer = lazy(() => import("./DiffViewer"))
// react-markdown is only needed when previewing a markdown file — lazy-load it
// into its own chunk so it never weighs on the main bundle or the editor open.
const MarkdownPreview = lazy(() => import("./MarkdownPreview"))

// Cap how many results the search list renders so a 1-char query in a huge repo
// can't mount thousands of rows.
const MAX_SEARCH_RESULTS = 300

interface EditorBodyProps {
  sessionId: string
  // True when composed by StandaloneEditorShell: the body IS the tab, so the
  // header drops its Close button (there is nothing to close into; the
  // shell's open-in-dux link is the way out) and its open-in-new-tab anchor
  // (this already is that tab), and on phones the explorer starts collapsed.
  standalone?: boolean
}

export function EditorBody({ sessionId, standalone = false }: EditorBodyProps) {
  const { bootstrap, changes, editorTarget, editorTabs } = useDux()
  const tabsState = editorTabs[sessionId]
  const tabs = useMemo(() => tabsState?.tabs ?? [], [tabsState])
  const activeTab = tabs.find((t) => t.id === tabsState?.activeId) ?? null

  // Per-tab Monaco buffers (file content + diff cache), keyed by tab id, kept
  // OUT of the global store deliberately (see lib/editorTabs.ts header
  // comment): putting file contents in zustand-style global state would fire
  // a store-wide update on every keystroke. Seeded from the module-level
  // draft cache (lib/editorDrafts.ts) and written back below, which is what
  // lets an unsaved draft survive the editor being closed and reopened.
  const [buffers, setBuffers] = useState<Map<string, TabBuffer>>(() =>
    loadSessionDrafts(sessionId),
  )
  const activeBuffer = activeTab ? buffers.get(activeTab.id) : undefined

  // Mirror every buffer change into the draft cache. The cache outlives this
  // component; the store prunes it when tabs close and clears it on session
  // delete, so this side is write-only.
  useEffect(() => {
    storeSessionDrafts(sessionId, buffers)
  }, [sessionId, buffers])

  // Report the active tab up as the editor's live URL position: the address
  // bar names the file (and mode) actually on screen, and in-editor switches
  // REPLACE the history entry (see the store's `editorSyncActiveTab`). No
  // active tab reports a pathless open editor.
  const activeTabMode = activeTab?.mode ?? "file"
  const activeTabPathForUrl = activeTab?.path ?? null
  useEffect(() => {
    editorSyncActiveTab(sessionId, activeTabMode, activeTabPathForUrl)
  }, [sessionId, activeTabMode, activeTabPathForUrl])

  // The `monaco` instance, captured once CodeEditor mounts (see CodeEditor's
  // `onReady`). EditorBody, not CodeEditor, owns disposal because it owns
  // tab lifecycle; see the model-lifecycle effect below.
  const monacoRef = useRef<MonacoInstance | null>(null)
  // The set of open PATHS as of the last run of the disposal effect. Disposal
  // diffs by PATH (not tab id): paths are unique across tabs (openFile rule 1
  // activates rather than duplicates an already-open path), so a path leaving
  // this set unambiguously means no tab holds it open anymore, including the
  // OLD path of a tab that just preview-replaced onto a new one.
  const prevOpenPathsRef = useRef<Set<string>>(new Set())

  // The latest `tabs` array, kept current every render (see the closeReqRef
  // effect below for the identical pattern). `save()`'s `.then()` needs the
  // LIVE tab list at RESOLVE time, not the `tabs` closed over when `save()`
  // was called: a delete confirmed while the write was in flight closes the
  // tab, and the save must notice that at resolve time to avoid a false
  // "Saved" toast (see `saveResolutionOutcome` / finding 3).
  const tabsRef = useRef<typeof tabs>(tabs)
  useEffect(() => {
    tabsRef.current = tabs
  })

  const [savingTabId, setSavingTabId] = useState<string | null>(null)
  // Paths with a save currently in flight, independent of `savingTabId`
  // (which is keyed by tab id and only tracks the one tab the Save button UI
  // reflects). Used to gate the Delete confirm dialog: deleting a path whose
  // save hasn't resolved yet would let the in-flight write silently recreate
  // the file right after the delete lands.
  const [savingPaths, setSavingPaths] = useState<Set<string>>(() => new Set())
  // Markdown preview toggle: render the buffer instead of the Monaco editor.
  // Kept per TAB ID (a Set of tabs currently showing the preview) rather than
  // one shared boolean reset on tab-change via an effect, a Set read is just
  // as simple, needs no reset effect (an effect that reset a plain boolean on
  // every tab switch would call `setState` synchronously in its body, which
  // the react-hooks/set-state-in-effect lint flags), and arguably reads better
  // (each tab remembers its own preview/edit choice across switches).
  const [previewOpenTabIds, setPreviewOpenTabIds] = useState<Set<string>>(
    () => new Set(),
  )
  function togglePreview(): void {
    if (!activeTab) return
    const tabId = activeTab.id
    setPreviewOpenTabIds((prev) => {
      const next = new Set(prev)
      if (next.has(tabId)) next.delete(tabId)
      else next.add(tabId)
      return next
    })
  }

  // The path currently revealed on first mount (deep link / opened-from-
  // elsewhere), frozen once, since FileTree only consumes it in a mount-only
  // effect. Later opens reveal themselves via FileTree's own openPath-change
  // effect, so this doesn't need to track subsequent opens. A lazy `useState`
  // initializer (not a ref) so it's safe to read during render.
  const [initialPath] = useState(() => editorTarget?.initialPath ?? null)

  // New File…/New Folder…/Rename…/Delete… dialog targets. Local EditorBody
  // state, not store targets: the file tree is a client-owned lazy cache, not
  // a server-broadcast ViewModel slice, so there is nothing for
  // useVanishedTargetGuard to key against (matches the New-file dialog's
  // existing precedent, see the plan's stop-condition note).
  const [newEntryTarget, setNewEntryTarget] = useState<NewEntryTarget | null>(
    null,
  )
  const [renameEntryTarget, setRenameEntryTarget] =
    useState<RenameEntryTarget | null>(null)
  const [deleteEntryTarget, setDeleteEntryTarget] =
    useState<DeleteEntryTarget | null>(null)
  const [moveEntryTarget, setMoveEntryTarget] =
    useState<MoveEntryTarget | null>(null)
  // The info panel is the one editor dialog with a live server-side truth to
  // key on, so unlike the four above it DOES close itself when its entry
  // vanishes; the guard lives inside the dialog, next to the fetch that
  // learns about it.
  const [fileInfoTarget, setFileInfoTarget] = useState<FileInfoTarget | null>(
    null,
  )
  // Bumped by `revalidateDirs` after a create/rename/delete lands, so
  // `FileTree` force-refetches the affected dir(s) past its lazy-load cache.
  const [treeRevalidate, setTreeRevalidate] = useState<{
    dirs: string[]
    nonce: number
  } | null>(null)
  const revalidateNonceRef = useRef(0)
  function revalidateDirs(dirs: string[]): void {
    revalidateNonceRef.current += 1
    const nonce = revalidateNonceRef.current
    // Functional update: unions `dirs` into whatever batch is already
    // pending rather than overwriting it, so two mutations that each call
    // `revalidateDirs` before React flushes a render between them (e.g. a
    // rename's source + destination parent dirs) both survive. See
    // `unionRevalidateBatch`'s doc comment for why a plain assignment drops
    // the earlier batch under React's same-tick setState batching.
    setTreeRevalidate((prev) => unionRevalidateBatch(prev, dirs, nonce))
  }
  // The flat file list backing the "Search files…" box (fetched from the
  // editor's session directly, independent of the changed-files watch). The
  // TREE does not consume this — it browses lazily via fileApi.tree.
  const [searchIndex, setSearchIndex] = useState<string[]>([])
  const [searchLoading, setSearchLoading] = useState(true)
  // True when the server capped the search index before sending all paths.
  const [searchTruncated, setSearchTruncated] = useState(false)
  const [search, setSearch] = useState("")
  // "Open editor" request in flight.
  const [openingEditor, setOpeningEditor] = useState(false)

  // Explorer panel layout: persisted by the panel library's OWN
  // useDefaultLayout (per tenet: reuse before invent; hand-rolled persistence
  // is exactly what this hook exists to replace). One shared localStorage
  // layout keyed by EDITOR_LAYOUT_ID. `onLayoutChanged` is the current
  // callback; `onLayoutChange` is deprecated and deliberately unused.
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({
    id: EDITOR_LAYOUT_ID,
    panelIds: [EXPLORER_PANEL_ID, EDITOR_CONTENT_PANEL_ID],
    storage: localStorage,
  })
  // Drop a stored layout carrying the sliver-explorer artifact (persisted
  // while defaultSize was a bare number the library read as pixels); see
  // sanitizeEditorLayout. Everything below reads the sanitized layout so the
  // seeds and the mount agree.
  const storedLayout = sanitizeEditorLayout(defaultLayout)
  // On a phone the STANDALONE editor starts with the explorer collapsed
  // (settled decision: phone standalone is best-effort, and the viewport
  // barely fits Monaco alone). Read at mount by editorMountLayout below;
  // a later resize or rotation must not re-collapse an explorer the user
  // has since expanded, and it cannot, because a defaultLayout only applies
  // at mount.
  const isMobile = useIsMobile()
  const startExplorerCollapsed = standalone && isMobile
  // What the group actually mounts with: the stored layout, or a true
  // 0%/100% collapse for the phone standalone case — overriding whatever a
  // desktop visit persisted under the same localStorage key. Structural on
  // purpose: mounting collapsed (rather than relying only on the
  // belt-and-braces collapse() effect below) leaves no frame where the
  // explorer renders expanded and no race with the library's initial layout.
  const mountLayout = editorMountLayout(storedLayout, startExplorerCollapsed)
  // The header toggle's icon/label state. Seeded from the mount layout (so a
  // collapsed explorer stays collapsed across opens, and the phone
  // standalone's forced-collapsed mount shows "Show" from the first frame;
  // with nothing stored the desktop overlay starts expanded) and kept
  // current by onLayoutChanged, which fires for drag-collapse and toggle
  // alike.
  const [explorerCollapsed, setExplorerCollapsed] = useState(() =>
    isExplorerCollapsed(mountLayout),
  )
  const explorerPanelRef = useRef<PanelImperativeHandle | null>(null)
  // The last width the explorer had while expanded, seeded from the persisted
  // layout and folded forward on every layout report. Toggling open resizes
  // to THIS rather than calling `panel.expand()`: expand() falls back to
  // minSize when it has no in-memory expand size (a fresh page load after
  // collapsing), which would reopen the explorer at a 12% sliver.
  const lastExpandedExplorerSizeRef = useRef<number | null>(
    lastExpandedExplorerSize(storedLayout, null),
  )
  function toggleExplorer(): void {
    const panel = explorerPanelRef.current
    if (!panel) return
    if (panel.isCollapsed()) {
      // On a phone the expand target ignores the remembered width (usually a
      // desktop-sized 22%, an ~86px sliver at 390px) and opens to the widest
      // width the content pane's minimum permits. See explorerExpandTarget.
      panel.resize(
        explorerExpandTarget(lastExpandedExplorerSizeRef.current, isMobile),
      )
    } else {
      panel.collapse()
    }
  }

  // Belt-and-braces for the phone standalone's collapsed start: the mount
  // layout above already mounts the explorer at a true 0%, so this is a
  // no-op when that landed (collapse() bails when already at collapsedSize).
  // It stays for the one gap defaultLayout has — the library ignores a
  // defaultLayout when its keys don't cover every panel — so the explorer
  // can never mount expanded on a phone through that route. Mount-only on
  // purpose: a later resize or rotation must not re-collapse an explorer the
  // user has since expanded.
  useEffect(() => {
    if (startExplorerCollapsed) explorerPanelRef.current?.collapse()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const dirty = activeTab?.dirty ?? false
  // Raster images never fetch /read (the 5 MiB /read cap refuses them before
  // the binary flag exists, and fileReady would park the tab on a spinner);
  // they render a read-only pane from /raw instead, and the buffer-derived
  // toolbar controls (Save, the preview toggle) do not render for them. SVG
  // is deliberately NOT an image tab: it opens in Monaco as editable text.
  const isImageTab = activeTab !== null && isImagePreviewPath(activeTab.path)
  // Which draft-accurate preview this tab's TEXT can offer: "markdown"
  // (react-markdown) or "svg" (a Blob URL over the current draft), null when
  // the Preview toggle should not render at all.
  const activePreviewKind =
    activeTab !== null ? previewKind(activeTab.path) : null
  const fileReady =
    activeTab !== null &&
    activeBuffer !== undefined &&
    !isBufferStale(activeBuffer, activeTab.path) &&
    activeBuffer.loadedPath === activeTab.path
  const binary = fileReady ? (activeBuffer?.binary ?? false) : false
  const readOnly = fileReady ? (activeBuffer?.readOnly ?? false) : false
  // File-mode readiness for the preview toggle: a loaded, non-binary buffer.
  // The toggle itself is available in BOTH modes; its diff-mode half needs
  // `diffReady`, declared further down, so `canPreview` lives beside it (one
  // source of truth for both the toggle button and the render).
  const filePreviewReady = fileReady && !binary
  // "Open editor" spawns a GUI editor on the SERVER, so it only helps when the
  // server is the user's own machine. Enable for local-access URLs; for remote
  // URLs keep the control but disable it with an explanatory tooltip.
  const localAccess = isLocalAccessHost(window.location.hostname)

  // The changed-files slice, trusted only when it belongs to THIS editor's
  // session (the editor always operates on the selected session, but a fast
  // switch could momentarily leave the slice pointed elsewhere).
  const slice = changes.sessionId === sessionId ? changes : null

  // Mark the tree's changed files from the slice. Stores the raw git status code
  // per path; FileStatusIcon maps it to an icon + label.
  const changedMap = useMemo(() => {
    const map = new Map<string, string>()
    if (!slice) return map
    for (const f of [...slice.unstaged, ...slice.staged]) {
      if (!map.has(f.path)) map.set(f.path, f.status)
    }
    return map
  }, [slice])

  // A per-file change-signal for the open file from the same slice: status +
  // line counts move when the file's content changes (best-effort — an edit that
  // keeps identical +/- counts won't move it). Used ONLY to flag a stale diff, not
  // to key the cache, so it never drives a refetch. Scanning unstaged then staged
  // avoids allocating a combined array on every tick.
  const activeTabPath = activeTab?.path ?? null
  // Not memoized: two small array scans over the changes slice is cheap, and
  // wrapping it in `useMemo` here fought `eslint-plugin-react-hooks`'
  // compiler-derived lint rules (it flagged the manual dependency array as
  // stale relative to its analysis). Note the build does NOT run
  // babel-plugin-react-compiler, so there is no runtime auto-memoization
  // here. This expression genuinely re-evaluates on every render; the two
  // scans are just cheap enough that that's fine.
  const openFileSignal = (() => {
    if (activeTabPath === null || !slice) return ""
    const f =
      slice.unstaged.find((x) => x.path === activeTabPath) ??
      slice.staged.find((x) => x.path === activeTabPath)
    return f ? `${f.status}:${f.additions}:${f.deletions}` : ""
  })()
  const openFileSignalRef = useRef("")

  // The diff is cached per tab+path; ready when the loaded diff is for the
  // active tab's CURRENT path. While ready, a change-signal differing from the
  // one captured at load means the file changed underneath, surface a reload
  // button (diffStale).
  const diffReady =
    activeTab !== null &&
    activeBuffer !== undefined &&
    !isBufferStale(activeBuffer, activeTab.path) &&
    activeBuffer.diffLoadedPath === activeTab.path
  const diffStale =
    diffReady && openFileSignal !== (activeBuffer?.diffLoadedSignal ?? "")
  // Diff-mode readiness for the preview: the diff is loaded for the tab's
  // current path and neither side is binary (the `fileReady && !binary`
  // equivalent for a tab whose only content is the diff cache).
  const diffPreviewReady = diffReady && !(activeBuffer?.diff?.binary ?? false)
  const canPreview =
    activePreviewKind !== null &&
    (activeTab?.mode === "diff" ? diffPreviewReady : filePreviewReady)
  const showPreview =
    activeTab !== null && previewOpenTabIds.has(activeTab.id) && canPreview
  // What the preview renders: always the END STATE of the file. In file mode
  // that is the draft. In diff mode the tab may have no file buffer at all,
  // so the unsaved draft wins only when one actually exists (buffer loaded
  // AND the tab is dirty); otherwise the diff's MODIFIED side (the file as on
  // disk) is exactly what file-mode preview would show.
  const previewContent =
    activeTab?.mode === "diff" && !(fileReady && dirty)
      ? (activeBuffer?.diff?.modified ?? "")
      : (activeBuffer?.draft ?? "")
  // This tab's save is in flight.
  const isSaving = savingTabId !== null && savingTabId === activeTab?.id

  // The one preview pane both content arms share (file mode and diff mode),
  // rendering `previewContent` (the file's end state, see its derivation
  // above). A plain function, not a component: extracting it as a component
  // would remount the pane (and re-mint the SVG blob URL) on every render.
  function renderPreviewPane(path: string) {
    return activePreviewKind === "svg" ? (
      // Rendered SVG of the end state (unsaved edits included when a draft
      // exists): a Blob object URL, and an <img>-embedded SVG executes no
      // scripts by spec. Not lazy: no heavy chunk to defer.
      <SvgPreviewPane draft={previewContent} path={path} />
    ) : (
      // Rendered markdown of the end state. Lazy like Monaco, so the same
      // ChunkBoundary + Suspense applies.
      <ChunkBoundary>
        <Suspense
          fallback={
            <div className="flex h-full items-center justify-center text-muted-foreground">
              <Loader2 className="size-5 motion-safe:animate-spin" />
            </div>
          }
        >
          <MarkdownPreview
            content={previewContent}
            sessionId={sessionId}
            path={path}
          />
        </Suspense>
      </ChunkBoundary>
    )
  }

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase()
    if (!needle) return []
    return searchIndex
      .filter((f) => f.toLowerCase().includes(needle))
      .slice(0, MAX_SEARCH_RESULTS)
  }, [search, searchIndex])

  // Refetch the search index (a capped flat walk of the worktree). Called on
  // mount AND after every create/rename/delete mutation so newly-created or
  // renamed paths become findable via "Search files…" without waiting for the
  // next overlay open. The TREE never uses this: it browses lazily per
  // directory via fileApi.tree, revalidated separately (see `revalidateDirs`).
  function refreshSearchIndex(): Promise<void> {
    return fileApi
      .list(sessionId)
      .then((result) => {
        setSearchIndex(result.files)
        setSearchTruncated(result.truncated ?? false)
      })
      .catch(() => {
        notifyError("could not index worktree files for search")
      })
  }

  // Mount-only: the body is keyed by session, so a new session remounts.
  useEffect(() => {
    refreshSearchIndex().finally(() => setSearchLoading(false))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Per-tab request tokens (mirrors FileTree.tsx's requestTokenRef): a resolve
  // whose token no longer matches the latest issued for that tab id is stale
  // (superseded by a later preview-replace or a rapid tab switch) and must not
  // write state.
  const fileRequestTokenRef = useRef<Map<string, number>>(new Map())
  const diffRequestTokenRef = useRef<Map<string, number>>(new Map())

  // Fetch and store a tab's file buffer for `path`. Extracted into its own
  // function (rather than inlined in the effect below) so the effect body
  // never calls `setBuffers` directly, mirrors FileTree.tsx's `fetchDir`. A
  // plain function (not `useCallback`): `eslint-plugin-react-hooks`'
  // compiler-derived manual-memoization lint disagreed with a `[sessionId]`
  // dependency array here even though this mirrors `fetchDir`'s (which DOES
  // pass) shape exactly; the effect below still gates on tab/path identity,
  // so a fresh function
  // each render costs nothing beyond one extra allocation.
  function loadFileBuffer(tabId: string, path: string): void {
    const token = (fileRequestTokenRef.current.get(tabId) ?? 0) + 1
    fileRequestTokenRef.current.set(tabId, token)
    // Seed/replace this tab's buffer for the new path, marking it `loading`
    // since a read is now in flight. This is the re-key step the preview-
    // replace fix depends on: without it, a stale buffer entry for the tab's
    // OLD path could still render while the new fetch is pending.
    setBuffers((prev) => {
      const next = new Map(prev)
      next.set(tabId, fileLoadSeedBuffer(path))
      return next
    })
    fileApi
      .read(sessionId, path)
      .then((f) => {
        if (fileRequestTokenRef.current.get(tabId) !== token) return
        setBuffers((prev) => {
          const cur = prev.get(tabId)
          // Superseded by a later preview-replace on this same tab id while
          // the read was in flight, don't resurrect a buffer for a path
          // this tab no longer holds.
          if (!cur || cur.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...cur,
            loadedPath: path,
            loading: false,
            loaded: f.content,
            draft: f.content,
            binary: f.binary,
            readOnly: f.read_only ?? false,
            fileError: null,
          })
          return next
        })
      })
      .catch((e) => {
        if (fileRequestTokenRef.current.get(tabId) !== token) return
        setBuffers((prev) => {
          const cur = prev.get(tabId)
          if (!cur || cur.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...cur,
            loading: false,
            // Settle this path as errored so the load effect's
            // `shouldSkipFileLoad` guard stops re-firing `fileApi.read` for
            // it on every render (see the `errorPath` field doc comment).
            errorPath: path,
            fileError: e instanceof Error ? e.message : "could not open file",
          })
          return next
        })
      })
  }

  // Load the active tab's file buffer lazily: only in file mode, only when the
  // cached buffer doesn't already hold CURRENT content for this tab (absent,
  // stale per `isBufferStale`, e.g. a preview-replace swapped this tab's
  // path), isn't already mid-fetch for this exact path (`loading`), AND
  // hasn't already settled with an error for this exact path (`errorPath`) --
  // that last check is what stops a failed read (a delete/rename race, a
  // plain 404) from retry-looping `fileApi.read` on every render forever; see
  // `shouldSkipFileLoad`'s doc comment. Skipped entirely in diff mode. Unlike
  // the diff, the buffer is NOT auto-refreshed when the file changes on disk
  // under us: re-reading could silently clobber unsaved edits.
  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    // Image tabs never load a buffer: the pane renders from /raw, and a /read
    // here would be refused over 5 MiB (before the binary flag exists) or
    // read megabytes only to discard them. A preview-replace onto an image
    // can leave the tab's PREVIOUS file's buffer behind, so drop it: without
    // this, replacing back to that file would render the stale buffer with
    // no refetch. Same-reference return when there is nothing to drop, so
    // this is not a setState loop (matches the prune effect's pattern).
    if (isImagePreviewPath(activeTab.path)) {
      const tabId = activeTab.id
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setBuffers((prev) => {
        if (!prev.has(tabId)) return prev
        const next = new Map(prev)
        next.delete(tabId)
        return next
      })
      return
    }
    if (shouldSkipFileLoad(activeBuffer, activeTab.path)) return
    // `loadFileBuffer` synchronously seeds a placeholder buffer (so a stale
    // buffer from a preview-replace never renders even for one frame) before
    // its async fetch resolves, a legitimate, deliberate synchronous
    // `setState` at the top of a data-loading effect, matching FileTree.tsx's
    // `fetchDir` (which seeds `{ status: "loading" }` before fetching the
    // same way). The lint's static call-graph tracing flags it here even
    // though the equivalent pattern in FileTree.tsx doesn't trip it. That
    // synchronous seed changes `activeBuffer?.loadedPath` on the very next
    // render (undefined -> null), which is exactly why the `loading` check
    // above exists: without it, this effect would fire a second, redundant
    // `fileApi.read` for the same tab+path before the first one even settles.
    // (The lint reports this effect once, on the image branch's setBuffers
    // above, so its disable covers this call too.)
    loadFileBuffer(activeTab.id, activeTab.path)
    // `loadFileBuffer` is a plain function (fresh identity every render, see
    // its own comment above), intentionally omitted so this effect is keyed
    // purely on tab/path/buffer identity, not on that fresh identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    activeTab?.mode,
    activeTab?.id,
    activeTab?.path,
    activeBuffer?.path,
    activeBuffer?.loadedPath,
    activeBuffer?.loading,
    activeBuffer?.errorPath,
  ])

  // Fetch and store a tab's diff cache for `path`. Extracted for the same
  // reason as `loadFileBuffer` above; also a plain function for the same
  // compiler-derived-lint-rule reason.
  //
  // The settle-time base: the tab's cached buffer may still carry a PREVIOUS
  // file's path (a preview-replace reuses the tab id and swaps the path; diff
  // mode has no synchronous seed the way `loadFileBuffer` does). A result for
  // `path` must then land on a FRESH buffer for `path`, not be dropped on the
  // stale one: dropping it left `diffLoadedPath` forever behind the tab's
  // path, nothing re-triggered the load effect, and the pane sat on the
  // spinner permanently (the Changes-pane "click a second file" bug). The
  // stale buffer's file fields are deliberately NOT carried over — they
  // describe the old file. When the buffer already belongs to `path` it is
  // kept, so a draft loaded in file mode survives a diff load.
  //
  // `tabsRef` guards the one remaining race: a valid-token result whose
  // `path` the tab has ALREADY moved off (the path changed after this fetch
  // started, before the load effect fired the replacement request and bumped
  // the token). Installing a buffer for the abandoned path would strand the
  // tab again, so that result is dropped; the new path's own load effect is
  // what recovers.
  function diffResultBase(
    prev: Map<string, TabBuffer>,
    tabId: string,
    path: string,
  ): TabBuffer | null {
    const cur = prev.get(tabId) ?? emptyBuffer(path)
    if (cur.path === path) return cur
    const tabPathNow = tabsRef.current.find((t) => t.id === tabId)?.path
    return tabPathNow === path ? emptyBuffer(path) : null
  }

  function loadDiffBuffer(tabId: string, path: string): void {
    const token = (diffRequestTokenRef.current.get(tabId) ?? 0) + 1
    diffRequestTokenRef.current.set(tabId, token)
    fileApi
      .diff(sessionId, path)
      .then((d) => {
        if (diffRequestTokenRef.current.get(tabId) !== token) return
        setBuffers((prev) => {
          const base = diffResultBase(prev, tabId, path)
          if (base === null) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...base,
            diff: d,
            diffLoadedPath: path,
            diffLoadedSignal: openFileSignalRef.current,
            diffError: null,
          })
          return next
        })
      })
      .catch((e) => {
        if (diffRequestTokenRef.current.get(tabId) !== token) return
        setBuffers((prev) => {
          const base = diffResultBase(prev, tabId, path)
          if (base === null) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...base,
            diffError: e instanceof Error ? e.message : "could not load diff",
          })
          return next
        })
      })
  }

  // Load the active tab's diff lazily: only in diff mode, only when the cache
  // doesn't already hold the tab's current path. Refetches on a tab switch and
  // on manual reload (which clears diffLoadedPath); does NOT refetch on a
  // change-signal tick, that lights the reload button instead.
  useEffect(() => {
    if (!activeTab || activeTab.mode !== "diff") return
    if (
      activeBuffer &&
      !isBufferStale(activeBuffer, activeTab.path) &&
      activeBuffer.diffLoadedPath === activeTab.path
    )
      return
    loadDiffBuffer(activeTab.id, activeTab.path)
    // See the loadFileBuffer effect above for why `loadDiffBuffer` is omitted.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    activeTab?.mode,
    activeTab?.id,
    activeTab?.path,
    activeBuffer?.path,
    activeBuffer?.diffLoadedPath,
  ])

  // Monaco model disposal: dispose a path's model once no tab holds it open
  // anymore. Diffs by the SET OF OPEN PATHS (not tab ids), see the
  // `prevOpenPathsRef` comment above for why that's required for a
  // preview-replace's OLD path to get disposed too.
  //
  // This same tick also prunes every tab-id-keyed cache down to the live tab
  // set: the `buffers` Map, the file/diff request-token maps, and the
  // markdown-preview-open Set otherwise keep a closed tab's entry forever
  // (unbounded for a long-lived overlay session that opens/closes many
  // files). `pruneByIds` returns the same reference when there's nothing to
  // drop, so this is a no-op setState on every tab-list change that isn't a
  // close.
  useEffect(() => {
    const currentPaths = new Set(tabs.map((t) => t.path))
    const mon = monacoRef.current
    if (mon) {
      for (const p of prevOpenPathsRef.current) {
        if (!currentPaths.has(p)) {
          mon.editor.getModel(mon.Uri.parse(p))?.dispose()
        }
      }
    }
    prevOpenPathsRef.current = currentPaths

    const liveIds = new Set(tabs.map((t) => t.id))
    // Synchronous setState in this effect body is deliberate, matching
    // `loadFileBuffer`'s seed above: this is a synchronize-with-`tabs`
    // cleanup step (pruning caches keyed by ids that just left `tabs`), not
    // state derived from a render. `pruneByIds`/`pruneSetByIds` return the
    // SAME reference when nothing needs dropping, so React bails out of the
    // re-render on every tick that isn't an actual tab close.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setBuffers((prev) => pruneByIds(prev, liveIds))
    setPreviewOpenTabIds((prev) => pruneSetByIds(prev, liveIds))
    fileRequestTokenRef.current = pruneByIds(fileRequestTokenRef.current, liveIds)
    diffRequestTokenRef.current = pruneByIds(diffRequestTokenRef.current, liveIds)
  }, [tabs])

  // Dispose every retained model when the overlay body unmounts (overlay
  // closed). @monaco-editor/react only disposes the CURRENT model on
  // unmount, so we own the rest, i.e. every OTHER path's model this tab strip
  // accumulated. One thing we deliberately do NOT chase down: the library
  // also keeps its own module-level view-state cache (cursor/scroll position
  // per path, in a Map that is never pruned), separate from the models we
  // dispose here. We accept that small, unbounded cache since its per-entry
  // cost is just cursor/scroll coordinates, and passing
  // `saveViewState={false}` to avoid it would mean losing view-state restore
  // (cursor/scroll position) when reopening a file.
  useEffect(() => {
    return () => {
      const mon = monacoRef.current
      if (!mon) return
      for (const p of prevOpenPathsRef.current) {
        mon.editor.getModel(mon.Uri.parse(p))?.dispose()
      }
    }
  }, [])

  // Preview open (tree single-click, search click). Double-click / new-file
  // pass { pin: true }. Single source of truth for every open entry point,
  // see lib/editorTabs.ts `openFile` for the promotion rules. Deliberately
  // carries no `mode` intent: a tree/search click is a plain activation, not
  // an explicit Edit/Diff action, so re-clicking an already-open path must
  // preserve whatever mode that tab is currently showing (openFile's rule 1)
  // rather than silently flipping an open diff tab back to file view.
  function requestOpen(path: string, opts?: { pin?: boolean }): void {
    editorOpenFile(sessionId, path, opts)
  }

  // Kept current here (an effect, not render) so the diff fetch's late callback
  // stamps the diff with the signal as of load-resolve time.
  useEffect(() => {
    openFileSignalRef.current = openFileSignal
  })

  // Report the buffer's dirty state up to the store (so the strip's dot and
  // the close-confirm gating read from one place), and promote a preview tab
  // to permanent on its first edit, so an in-progress edit is never silently
  // discarded by a later preview-replace.
  function handleDraftChange(value: string): void {
    if (!activeTab) return
    const tabId = activeTab.id
    setBuffers((prev) => {
      const cur = prev.get(tabId)
      if (!cur) return prev
      const next = new Map(prev)
      next.set(tabId, { ...cur, draft: value })
      return next
    })
    const cur = buffers.get(tabId)
    if (cur && !cur.binary && !cur.readOnly) {
      const newDirty = value !== cur.loaded
      // Only dispatch when the dirty flag actually flips: `useDux()` is an
      // unselective `useSyncExternalStore`, so a dispatch on every keystroke
      // fans out a store-wide re-render to every consumer, not just this
      // overlay. `editorSetTabDirty`/`setTabDirty` also short-circuit on an
      // unchanged value (belt-and-braces), but skipping the call entirely
      // here avoids even constructing the new-tabs-array allocation the
      // reducer would otherwise do on every keystroke.
      if (newDirty !== activeTab.dirty) {
        editorSetTabDirty(sessionId, tabId, newDirty)
      }
      if (shouldPromoteOnEdit(activeTab, newDirty)) editorPinTab(sessionId, tabId)
    }
  }

  function save(): void {
    if (!activeTab || binary || isSaving || !dirty) return
    const tabId = activeTab.id
    const path = activeTab.path
    const body = activeBuffer?.draft ?? ""
    setSavingTabId(tabId)
    setSavingPaths((prev) => {
      const next = new Set(prev)
      next.add(path)
      return next
    })
    fileApi
      .write(sessionId, path, body)
      .then(() => {
        // Re-check the LIVE tabs list at RESOLVE time (via `tabsRef`, kept
        // current every render), not the `tabs` this closure captured when
        // `save()` was called: a delete confirmed while the write was in
        // flight already closed the tab, and the write already reached the
        // server and succeeded there (it cannot be un-sent). Reporting
        // "Saved" in that case would lie; `saveResolutionOutcome` picks the
        // honest outcome instead.
        const tabStillOpen = tabsRef.current.some((t) => t.id === tabId)
        const outcome = saveResolutionOutcome(path, tabStillOpen)
        if (tabStillOpen) {
          // Stale-save guard: if this tab's buffer no longer belongs to `path`
          // (a preview-replace reused the tab id for a different file while
          // the save was in flight), don't resurrect a buffer for a file no
          // longer open in this tab.
          setBuffers((prev) => {
            const cur = prev.get(tabId)
            if (!cur || isBufferStale(cur, path)) return prev
            const next = new Map(prev)
            next.set(tabId, { ...cur, loaded: body, diffLoadedPath: null })
            return next
          })
          editorSetTabDirty(sessionId, tabId, false)
        }
        if (outcome.tone === "warning") notifyWarning(outcome.message)
        else notifySuccess(outcome.message)
      })
      .catch((e) => {
        // The DRAFT IS KEPT, deliberately, and this is the whole answer to a
        // refused save. Nothing here touches `setBuffers` or clears the dirty
        // flag, so the text stays exactly as typed and the tab stays dirty:
        // the user can shorten the file, or copy the buffer out, and save
        // again. It matters most for the size refusal, which is the one
        // failure a user can reach by editing rather than by something going
        // wrong (the read cap is 5 MiB and the write cap roughly 10 MiB, so it
        // takes more than doubling a file's escaped size in one sitting). A
        // pre-flight size check is deliberately NOT built for that: it would
        // need a size on every tree entry and an escaped-length estimate on
        // the client, to guard a case that costs nothing when it happens
        // because of this line.
        notifyError(e instanceof Error ? e.message : "could not save file")
      })
      .finally(() => {
        setSavingTabId((id) => (id === tabId ? null : id))
        setSavingPaths((prev) => {
          if (!prev.has(path)) return prev
          const next = new Set(prev)
          next.delete(path)
          return next
        })
      })
  }

  // Reload the diff for the active tab: the "file changed underneath you"
  // reload button. Dropping diffLoadedPath makes the diff-load effect refetch.
  function refreshDiff(): void {
    if (!activeTab) return
    const tabId = activeTab.id
    setBuffers((prev) => {
      const cur = prev.get(tabId)
      if (!cur) return prev
      const next = new Map(prev)
      next.set(tabId, { ...cur, diffLoadedPath: null })
      return next
    })
  }

  // Open the current file in a locally-installed GUI editor (server-side spawn).
  // `editorKey` is the one the user picked from the menu; the server launches it
  // or reports it isn't installed. Only reachable when `localAccess` is true.
  function openInEditorAction(editorKey: string): void {
    if (!activeTab || openingEditor) return
    setOpeningEditor(true)
    fileApi
      .openInEditor(sessionId, activeTab.path, editorKey)
      // "Opening" not "Opened": we spawned the editor but can't confirm a window
      // actually appeared (e.g. a headless server would launch-then-exit).
      .then((editor) => notifySuccess(`Opening in ${editor}…`))
      .catch((e) =>
        notifyError(e instanceof Error ? e.message : "could not open in editor"),
      )
      .finally(() => setOpeningEditor(false))
  }

  // New File… / New Folder…, unified: `newEntryTarget.kind` picks the server
  // call. On success the dialog closes; a NEW file is also opened (pinned,
  // forced to file mode (it can't have a diff since it was just created),
  // matching the header FilePlus button's "New File at root" behavior. On
  // error the dialog stays open (target untouched) so the user can fix the
  // name and retry: the promise resolves either way so NewEntryDialog's
  // submitting flag always clears.
  function handleNewEntrySubmit(name: string): Promise<void> {
    if (!newEntryTarget) return Promise.resolve()
    const { kind, dir } = newEntryTarget
    const path = joinName(dir, name)
    const create =
      kind === "file"
        ? fileApi.createFile(sessionId, path)
        : fileApi.createDir(sessionId, path)
    return create
      .then(() => {
        setNewEntryTarget(null)
        revalidateDirs([dir])
        if (kind === "file") {
          editorOpenFile(sessionId, path, { mode: "file", pin: true })
        }
        return refreshSearchIndex()
      })
      .catch((e) => {
        notifyError(
          e instanceof Error
            ? e.message
            : `could not create ${kind === "file" ? "file" : "folder"}`,
        )
      })
  }

  // Rename…: retargets the open tab's path in place (see editorTabs.ts
  // `renameTabPaths` for the tab-collision close and the accepted Monaco
  // undo-history/view-state loss) and revalidates BOTH the source and
  // destination parent dirs (a same-dir rename only needs one refetch, but a
  // move across dirs needs both to reflect the entry leaving one and
  // appearing in the other). On error the dialog stays open for a retry.
  function handleRenameSubmit(newName: string): Promise<void> {
    if (!renameEntryTarget) return Promise.resolve()
    const from = renameEntryTarget.path
    const to = computeRenameTarget(from, newName)
    return fileApi
      .rename(sessionId, from, to)
      .then(() => {
        setRenameEntryTarget(null)
        editorRenameTabPaths(sessionId, from, to)
        revalidateDirs([parentDir(from), parentDir(to)])
        return refreshSearchIndex()
      })
      .catch((e) => {
        notifyError(e instanceof Error ? e.message : "could not rename")
      })
  }

  // Move…: a move IS a rename on disk, so it goes to the same server route
  // with a destination in a different folder, and reuses the rename's tab
  // retargeting and two-directory revalidation. Overwriting is REFUSED rather
  // than confirmed (see the dialog and CLAUDE.md): the server rejects an
  // occupied destination and the toast says so. The composition itself lives
  // in `lib/moveEntry`, so its ordering is testable without mounting the
  // editor; this is only the wiring.
  function handleMoveSubmit(destDir: string): Promise<void> {
    if (!moveEntryTarget) return Promise.resolve()
    return performMove(moveEntryTarget.path, destDir, {
      rename: (from, to) => fileApi.rename(sessionId, from, to),
      clearTarget: () => setMoveEntryTarget(null),
      retargetTabs: (from, to) => editorRenameTabPaths(sessionId, from, to),
      revalidateDirs,
      refreshSearchIndex,
      reportError: (message) => notifyError(message),
    })
  }

  // Delete…: fire-and-forget once confirmed, mirroring
  // ConfirmDiscardFileDialog's handleConfirm (closes immediately rather than
  // waiting on the request; the destructive confirm dialog already gated the
  // action, and a failure (e.g. another client already deleted it) just
  // toasts, matching the plan's "another client raced us" acceptance).
  //
  // Defensively re-checks `savingPaths` even though the Delete button in
  // `DeleteEntryDialog` is already disabled via `blockedBySave` while a save
  // for this path is in flight: the dialog computes that from the render at
  // which it was opened, so this is a belt-and-braces guard against a race
  // between a save starting and the click handler firing.
  function handleDeleteConfirm(): void {
    const target = deleteEntryTarget
    if (!target || savingPaths.has(target.path)) return
    setDeleteEntryTarget(null)
    fileApi
      .remove(sessionId, target.path)
      .then(() => {
        editorCloseTabsUnderPath(sessionId, target.path)
        revalidateDirs([parentDir(target.path)])
        return refreshSearchIndex()
      })
      .catch((e) => {
        notifyError(e instanceof Error ? e.message : "could not delete")
        // A failed delete (e.g. a permission error mid-`remove_dir_all`, or
        // another client racing the same path) can still leave the tree
        // cache stale relative to whatever is actually left on disk after a
        // PARTIAL removal. Revalidate the same dir(s) the success path
        // would, so `FileTree` re-reads the real state instead of showing a
        // listing from before the attempt.
        revalidateDirs(
          target.isDir
            ? [parentDir(target.path), target.path]
            : [parentDir(target.path)],
        )
      })
  }

  // Files dragged from the DESKTOP onto the tree. This is dux's durable drop
  // intent, "add this file to my project": the file lands where the user
  // pointed, as an ordinary visible file, and NOTHING is pasted into any
  // terminal (that is the pane drop, and it is a different intent entirely).
  //
  // The refresh afterwards is the move path's, for the same reason: the tree's
  // cached listing of that directory and the flat search index both went stale.
  function handleFilesDropped(dir: string, dropped: DroppedItems): void {
    const toastId = nextFileDropToastId()
    void performTreeDrop(dir, dropped, {
      upload: (file, into) =>
        // `conn` is null on purpose. It names the TERMINAL SOCKET, and the
        // check it feeds exists only so a viewer who cannot paste is told
        // before a file is written. A tree drop pastes nothing, so there is
        // nothing to check and no socket to name.
        uploadDroppedFile(file, { pty: sessionId, conn: null, dir: into }),
      revalidateDirs,
      refreshSearchIndex,
      reportBusy: (message) => notifyBusy(message, { id: toastId }),
      // `sticky` is forwarded rather than dropped, so the decision stays in the
      // one place that makes it (`editorDropToast`, where every rung says
      // false, and says why). Hardcoding it here would put a second opinion
      // next to the first; omitting it, as this line used to, silently made a
      // tree drop unpinnable whatever the report asked for.
      reportFinal: (t) => notify(t.tone, t.message, { id: toastId, sticky: t.sticky }),
    })
  }

  const openPath = activeTab?.path ?? null

  return (
    <>
      {/* Header: open file path, view toggle, dirty indicator, actions.
          min-h-12.75 (51px) floors the row at its tallest control, the
          File/Diff segmented group, an h-7 (28px) button inside p-0.5 (4px)
          + border (2px) = 34px, plus the row's py-2 (16px) and its own
          border-b (1px; min-h is border-box), so the bar keeps one height
          as controls come and go instead of jumping 6px whenever a file
          opens or closes (measured 51px with a file open, 45px without,
          before the floor). The mobile toggle (max-md:size-10, 40px)
          exceeds the floor on phones, where the toggle always renders, so
          the row is constant there too. */}
      <div className="flex min-h-12.75 items-center gap-2 border-b px-3 py-2">
        {/* Explorer collapse/expand toggle: lives in the header, OUTSIDE the
            panel it hides, so it stays reachable while collapsed. */}
        <SimpleTooltip
          content={
            explorerCollapsed ? "Show the file explorer" : "Hide the file explorer"
          }
        >
          <Button
            size="icon-sm"
            variant="ghost"
            className="shrink-0 max-md:size-10"
            aria-label={
              explorerCollapsed
                ? "Show the file explorer"
                : "Hide the file explorer"
            }
            // No aria-pressed: the control's state is carried by its CHANGING
            // accessible name (Hide/Show), and a pressed state on top of a
            // changing name reads as contradictory to assistive tech.
            onClick={toggleExplorer}
          >
            {explorerCollapsed ? <PanelLeftOpen /> : <PanelLeftClose />}
          </Button>
        </SimpleTooltip>
        <FileCode2 className="size-4 shrink-0 text-muted-foreground" />
        {/* <bdi> LTR isolate: keeps a leading bidi-neutral char in a dotfile
            path (".github/...") from being reordered to the end by direction:rtl. */}
        <span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl]">
          <bdi dir="ltr">{openPath ?? "Select a file"}</bdi>
        </span>
        {/* Dirty dot kept OUTSIDE the truncating span so it can't be clipped on
            a long path; sr-only text announces the state to screen readers. */}
        {dirty && (
          <>
            <SimpleTooltip content="Unsaved changes">
              <span className="shrink-0 text-primary" aria-hidden="true">
                ●
              </span>
            </SimpleTooltip>
            <span className="sr-only">unsaved changes</span>
          </>
        )}
        {/* Read-only badge — shown when the server flagged the file as
            read-only (external symlink or .git/ path). */}
        {readOnly && activeTab?.mode === "file" && (
          <SimpleTooltip content="This file is read-only — it is a symlink to an external file or a .git path">
            <span className="shrink-0 text-xs text-muted-foreground">
              read-only
            </span>
          </SimpleTooltip>
        )}
        {/* File / Diff view toggle — a segmented control. Hidden until a file
            is open (nothing to view otherwise), and hidden for image tabs
            (no text to diff; the store coerces image opens to file mode, so
            offering the switch would only reach the binary-diff dead end).
            Sets the ACTIVE TAB's mode. */}
        {activeTab && !isImageTab && (
          // max-md:hidden: on phones (only the standalone surface — the
          // overlay is desktop-only) the header folds every secondary
          // control into the one ⋯ menu at the row's end, per the
          // row-actions tenet; only the explorer toggle and Save stay
          // inline. Desktop keeps the inline layout exactly.
          <div
            className="flex shrink-0 items-center gap-0.5 rounded-md border p-0.5 max-md:hidden"
            role="group"
            aria-label="View mode"
          >
            <Button
              size="sm"
              variant={activeTab.mode === "file" ? "default" : "ghost"}
              aria-pressed={activeTab.mode === "file"}
              onClick={() => editorSetTabMode(sessionId, activeTab.id, "file")}
            >
              <FileText />
              File
            </Button>
            <Button
              size="sm"
              variant={activeTab.mode === "diff" ? "default" : "ghost"}
              aria-pressed={activeTab.mode === "diff"}
              onClick={() => editorSetTabMode(sessionId, activeTab.id, "diff")}
            >
              <GitCompare />
              Diff
            </Button>
          </div>
        )}
        {/* "File changed underneath you" reload — shown in diff mode when the
            changed-files broadcast indicates the open file moved since the diff was
            loaded. We don't auto-refetch (avoids churn); the user reloads on click. */}
        {activeTab?.mode === "diff" && diffStale && (
          <SimpleTooltip content="This file changed on disk — reload the diff">
            <Button
              size="sm"
              variant="ghost"
              className="text-amber-500 max-md:hidden"
              aria-label="Reload diff — the file changed on disk"
              onClick={refreshDiff}
            >
              <CircleAlert />
              Reload
            </Button>
          </SimpleTooltip>
        )}
        {/* Markdown/SVG preview toggle — both modes. In file mode the label
            swaps to "Edit" while previewing (the toggle returns to the
            editor); in diff mode toggling off returns to the READ-ONLY diff,
            so "Edit" would lie — the label stays "Preview" and aria-pressed
            plus the variant carry the state. */}
        {canPreview && (
          <Button
            size="sm"
            variant={showPreview ? "default" : "ghost"}
            className="max-md:hidden"
            aria-pressed={showPreview}
            onClick={togglePreview}
          >
            {showPreview && activeTab?.mode === "file" ? <Pencil /> : <Eye />}
            {showPreview && activeTab?.mode === "file" ? "Edit" : "Preview"}
          </Button>
        )}
        {/* Open in a local GUI editor — a menu of supported editors. A disabled
            trigger swallows hover events (pointer-events:none), so the tooltip
            lives on a wrapping span that always receives them. */}
        {activeTab && (
          <SimpleTooltip
            content={
              localAccess
                ? undefined
                : "Only available when dux is opened locally — not over a remote URL."
            }
          >
            <span className="inline-flex max-md:hidden">
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={!localAccess || openingEditor}
                      aria-busy={openingEditor}
                    />
                  }
                >
                  {openingEditor ? (
                    <Loader2 className="motion-safe:animate-spin" />
                  ) : (
                    <ExternalLink />
                  )}
                  Open local editor
                  <ChevronDown />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {OPEN_IN_EDITORS.map((editor) => (
                    <DropdownMenuItem
                      key={editor.key}
                      onClick={() => openInEditorAction(editor.key)}
                    >
                      <EditorIcon editorKey={editor.key} />
                      {editor.label}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </span>
          </SimpleTooltip>
        )}
        {/* Save — file mode only (diff is read-only), and never for an image
            tab (no buffer exists for it to act on). */}
        {activeTab?.mode === "file" && !isImageTab && (
          <Button
            size="sm"
            className="max-md:min-h-10"
            disabled={!dirty || isSaving || readOnly}
            aria-busy={isSaving}
            onClick={save}
          >
            {isSaving ? <Loader2 className="motion-safe:animate-spin" /> : <Save />}
            Save
          </Button>
        )}
        {/* The phone fold (md:hidden): every secondary header control in ONE
            ⋯ menu, per the row-actions tenet — the mode switch, the stale-
            diff reload, the preview toggle, and Open local editor. Items
            keep their leading lucide icons; none carries a trailing "…"
            because none opens a dialog. The active view mode is marked with
            a trailing check AND aria-current so it reads to assistive tech.
            Only the explorer toggle and Save stay inline on a phone. */}
        {activeTab && (
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  size="icon-sm"
                  variant="ghost"
                  className="shrink-0 md:hidden max-md:size-10"
                  aria-label="More editor actions"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {!isImageTab && (
                <>
                  <DropdownMenuItem
                    aria-current={activeTab.mode === "file" ? "true" : undefined}
                    onClick={() =>
                      editorSetTabMode(sessionId, activeTab.id, "file")
                    }
                  >
                    <FileText />
                    File view
                    {activeTab.mode === "file" && <Check className="ml-auto" />}
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    aria-current={activeTab.mode === "diff" ? "true" : undefined}
                    onClick={() =>
                      editorSetTabMode(sessionId, activeTab.id, "diff")
                    }
                  >
                    <GitCompare />
                    Diff view
                    {activeTab.mode === "diff" && <Check className="ml-auto" />}
                  </DropdownMenuItem>
                </>
              )}
              {activeTab.mode === "diff" && diffStale && (
                <DropdownMenuItem onClick={refreshDiff}>
                  <CircleAlert />
                  Reload diff
                </DropdownMenuItem>
              )}
              {canPreview && (
                <DropdownMenuItem onClick={togglePreview}>
                  {showPreview && activeTab.mode === "file" ? (
                    <Pencil />
                  ) : (
                    <Eye />
                  )}
                  {showPreview ? "Hide preview" : "Show preview"}
                </DropdownMenuItem>
              )}
              {/* Present for image tabs too, matching the inline control. */}
              <DropdownMenuSub>
                <DropdownMenuSubTrigger disabled={!localAccess || openingEditor}>
                  <ExternalLink />
                  Open local editor
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {OPEN_IN_EDITORS.map((editor) => (
                    <DropdownMenuItem
                      key={editor.key}
                      onClick={() => openInEditorAction(editor.key)}
                    >
                      <EditorIcon editorKey={editor.key} />
                      {editor.label}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        {/* Open this editor as its own browser tab (the standalone surface).
            A real anchor, not a click handler, so middle-click and
            ctrl/cmd-click keep their native new-tab semantics; the href
            carries the active file so the new tab opens on it. Absent on the
            standalone surface itself, which already is that tab. */}
        {!standalone && (
          <Button
            size="sm"
            variant="ghost"
            className="shrink-0 max-md:min-h-10"
            render={
              <a
                href={standaloneEditorHash(
                  sessionId,
                  activeTab
                    ? { mode: activeTab.mode, path: activeTab.path }
                    : null,
                )}
                target="_blank"
                rel="noopener"
              />
            }
          >
            <ExternalLink />
            {/* A visible label, not an icon-only button: an unlabeled
                external-link glyph reads as "some link", and this header's
                idiom is icon + text (Save, Close). The text also makes the
                tooltip redundant, so there is none. */}
            Open in new tab
          </Button>
        )}
        {/* Closes immediately, dirty tabs included: nothing is lost (tabs
            live in the store, drafts in the module cache), so there is no
            dialog to ask with. Standalone has no overlay to close; its way
            out is the shell's open-in-dux link. */}
        {!standalone && (
          <Button size="sm" variant="ghost" onClick={() => closeEditor()}>
            <X />
            Close
          </Button>
        )}
      </div>

      {/* Tab strip, only renders once the session has at least one tab. */}
      <EditorTabsStrip sessionId={sessionId} />

      {/* Body: worktree file tree (left, a collapsible resizable panel) +
          Monaco editor/diff (right). The outer div mirrors DesktopShell's
          panel-group mount (min-h-0 flex-1 wrapper, size-full group). */}
      <div className="min-h-0 flex-1">
        <ResizablePanelGroup
          orientation="horizontal"
          id={EDITOR_LAYOUT_ID}
          defaultLayout={mountLayout}
          onLayoutChanged={(layout) => {
            onLayoutChanged(layout)
            setExplorerCollapsed(isExplorerCollapsed(layout))
            lastExpandedExplorerSizeRef.current = lastExpandedExplorerSize(
              layout,
              lastExpandedExplorerSizeRef.current,
            )
          }}
          className="size-full"
        >
          {/* Size props are STRING percentages (see editorLayout.ts): the
              panel library reads a bare number as PIXELS, which is how the
              explorer once mounted ~22px wide. The inline overflow:hidden
              overrides the library wrapper's own overflow:auto (a className
              cannot beat an inline style) so each pane owns its scrolling:
              the tree/search ScrollAreas here, Monaco and the preview panes
              in the content panel. Without it the wrapper sprouts its own
              scrollbars around Monaco's (the nested-scrollbar bug) and
              jitters during divider drags (see TerminalArea's identical
              clip). */}
          <ResizablePanel
            id={EXPLORER_PANEL_ID}
            panelRef={explorerPanelRef}
            defaultSize={EXPLORER_DEFAULT_SIZE_PROP}
            minSize={EXPLORER_MIN_SIZE_PROP}
            style={{ overflow: "hidden" }}
            collapsible
          >
            {/* min-w-0 so path truncation keeps working at narrow widths.
                Deliberately NO border-r: the ResizableHandle beside this
                panel already draws the 1px pane separator, and the app-wide
                idiom (the terminal/changes split, the Changes pane's
                border-0 Card) is panes bleeding to the shell edge with the
                handle as the only divider. A border here doubled the left
                edge of the editor content while its bottom/right had none,
                which read as an unfinished frame. */}
            <div className="flex h-full min-w-0 flex-col">
              <div className="flex items-center gap-1 border-b p-2">
                <div className="relative flex-1">
                  <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search files…"
                    className="h-8 pl-7 text-sm"
                  />
                </div>
                <SimpleTooltip content="New file">
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label="New file"
                    onClick={() => setNewEntryTarget({ kind: "file", dir: "" })}
                  >
                    <FilePlus />
                  </Button>
                </SimpleTooltip>
              </div>
              {/* The tree owns its own ScrollArea (it virtualizes against its
                  viewport, so it must be the element that scrolls); this outer one
                  wraps only the flat search results. */}
              {search.trim() ? (
                <ScrollArea className="min-h-0 flex-1">
                  <div className="p-1">
                    {searchLoading ? (
                      <div className="flex items-center justify-center py-4 text-muted-foreground">
                        <Loader2 className="size-4 motion-safe:animate-spin" />
                      </div>
                    ) : filtered.length === 0 ? (
                      <p className="px-1 py-2 text-sm text-muted-foreground">
                        No files match.
                      </p>
                    ) : (
                      <>
                        {filtered.map((p) => (
                          <button
                            key={p}
                            type="button"
                            onClick={() => requestOpen(p)}
                            className={cn(
                              "flex w-full items-center gap-1.5 rounded px-1 py-1 hover:bg-muted",
                              p === openPath && "bg-muted",
                            )}
                          >
                            {changedMap.has(p) && (
                              <FileStatusIcon status={changedMap.get(p)!} />
                            )}
                            {/* Full path → start-ellipsize so the filename stays visible.
                                <bdi> LTR isolate keeps a leading "." (dotfile path) from
                                being reordered to the end by direction:rtl. */}
                            <span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl]">
                              <bdi dir="ltr">{p}</bdi>
                            </span>
                          </button>
                        ))}
                        {searchTruncated && (
                          <p className="px-1 py-2 text-xs text-muted-foreground">
                            The search index was capped — results may be
                            incomplete.
                          </p>
                        )}
                      </>
                    )}
                  </div>
                </ScrollArea>
              ) : (
                <FileTree
                  sessionId={sessionId}
                  openPath={openPath}
                  changed={changedMap}
                  initialPath={initialPath}
                  onOpen={requestOpen}
                  onNewFile={(dir) => setNewEntryTarget({ kind: "file", dir })}
                  onNewFolder={(dir) => setNewEntryTarget({ kind: "folder", dir })}
                  onRename={(path, isDir) =>
                    setRenameEntryTarget({ path, isDir })
                  }
                  onMove={(path, isDir) => setMoveEntryTarget({ path, isDir })}
                  onInfo={(path) => setFileInfoTarget({ path })}
                  onDelete={(path, isDir) =>
                    setDeleteEntryTarget({ path, isDir })
                  }
                  revalidate={treeRevalidate}
                  // NOT YET KNOWN is NOT ENABLED, exactly as on the pane: the
                  // bootstrap document and the workspace load in parallel, and
                  // an older server never sends the field, so a drag arriving
                  // in that window must not offer a feature dux cannot yet say
                  // it has. The window closes in one fetch.
                  fileDropEnabled={(bootstrap?.file_drop_max_bytes ?? 0) > 0}
                  onFilesDropped={handleFilesDropped}
                />
              )}
            </div>
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel
            id={EDITOR_CONTENT_PANEL_ID}
            minSize={EDITOR_CONTENT_MIN_SIZE_PROP}
            style={{ overflow: "hidden" }}
          >
            <div className="relative h-full min-w-0">
              {activeTab === null ? (
                <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
                  Select a file from the tree to view or edit it.
                </div>
              ) : isImageTab ? (
                // Sits ABOVE the diff arm AND the buffer-gated chain on
                // purpose. Above the buffer chain: an image tab has no
                // buffer, so `fileReady` never turns true and those arms
                // would park it on the spinner forever. Above the diff arm:
                // the store coerces image opens to file mode, but if a
                // diff-mode image tab reaches here anyway the picture must
                // win over the binary-diff refusal (defense in depth). Keyed
                // by path so a failed load resets when the tab
                // preview-replaces onto another file.
                <ImagePreviewPane
                  key={activeTab.path}
                  src={fileApi.rawUrl(sessionId, activeTab.path)}
                  path={activeTab.path}
                />
              ) : activeTab.mode === "diff" ? (
                // Read-only Monaco diff (HEAD vs working copy).
                activeBuffer?.diffError && !isBufferStale(activeBuffer, activeTab.path) ? (
                  <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-sm text-destructive">
                    {activeBuffer.diffError}
                    {/* Manual retry, mirroring the file-mode error arm below: a
                        settled diff error is never auto-retried (the load
                        effect's deps don't move once diffError is set), so
                        without this the only ways back are switching tabs or
                        reopening the editor. */}
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => loadDiffBuffer(activeTab.id, activeTab.path)}
                    >
                      <RotateCw />
                      Retry
                    </Button>
                  </div>
                ) : !diffReady ? (
                  <div className="flex h-full items-center justify-center text-muted-foreground">
                    <Loader2 className="size-5 motion-safe:animate-spin" />
                  </div>
                ) : activeBuffer?.diff?.binary ? (
                  <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
                    This file is binary and can&rsquo;t be diffed here.
                  </div>
                ) : showPreview ? (
                  // Same preview as file mode, rendering the file's END
                  // STATE (the draft when one exists, else the diff's
                  // modified side). Toggling off returns here, to the diff:
                  // previewing never changes the tab's mode.
                  renderPreviewPane(activeTab.path)
                ) : (
                  // An ALL-DELETE diff (a deleted or truncated-to-empty file)
                  // gets the `dux-diff-all-delete` marker class, which scopes
                  // the three-layer suppression of Monaco's phantom trailing
                  // "1 +" inserted line (an empty modified model still has one
                  // line, and monaco 0.55.1's diff computer reports a real
                  // insertion for it — original [1,N) → modified [1,2) — for
                  // every original shape). Layer one is CSS (index.css under
                  // this marker): the insertion DECORATIONS are plain DOM, so
                  // scoped rules blank them — but Monaco ships its own
                  // `.monaco-editor .insert-sign { display: flex !important }`,
                  // so the sign rule must match that specificity to win the
                  // !important tie. Layer two is options (allDeleteDiffOptions
                  // via the `allDelete` prop): the current-line highlight
                  // borders the now-blank row, and the overview ruler is a
                  // CANVAS whose green speck no CSS can reach. Layer three is
                  // the line-number rule, scoped to `.editor.modified`: the
                  // deleted rows' numbers are ordinary .line-numbers in the
                  // sibling original editor's margin and must survive, while
                  // the modified editor's sole line number in an all-delete
                  // diff is the phantom row's. The original side's text is
                  // never touched.
                  <div
                    className={
                      activeBuffer?.diff && isAllDeleteDiff(activeBuffer.diff)
                        ? "dux-diff-all-delete h-full"
                        : "h-full"
                    }
                  >
                    <ChunkBoundary>
                      <Suspense
                        fallback={
                          <div className="flex h-full items-center justify-center text-muted-foreground">
                            <Loader2 className="size-5 motion-safe:animate-spin" />
                          </div>
                        }
                      >
                        <DiffViewer
                          path={activeTab.path}
                          original={activeBuffer?.diff?.original ?? ""}
                          modified={activeBuffer?.diff?.modified ?? ""}
                          allDelete={
                            activeBuffer?.diff !== null &&
                            activeBuffer?.diff !== undefined &&
                            isAllDeleteDiff(activeBuffer.diff)
                          }
                        />
                      </Suspense>
                    </ChunkBoundary>
                  </div>
                )
              ) : activeBuffer?.fileError && !isBufferStale(activeBuffer, activeTab.path) ? (
                <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-sm text-destructive">
                  {activeBuffer.fileError}
                  {/* Manual retry: a settled error is never auto-retried (see
                      `errorPath`/`shouldSkipFileLoad`), so this is the only way
                      back without switching tabs. Mirrors the diff pane's reload
                      button above. */}
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => loadFileBuffer(activeTab.id, activeTab.path)}
                  >
                    <RotateCw />
                    Retry
                  </Button>
                </div>
              ) : !fileReady ? (
                <div className="flex h-full items-center justify-center text-muted-foreground">
                  <Loader2 className="size-5 motion-safe:animate-spin" />
                </div>
              ) : binary ? (
                <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
                  This file is binary and can&rsquo;t be edited here.
                </div>
              ) : showPreview ? (
                renderPreviewPane(activeTab.path)
              ) : (
                // ChunkBoundary (outside Suspense) catches a failed lazy import after
                // a redeploy — a 404 on the hashed Monaco chunk — and offers reload,
                // instead of unmounting the whole app to a white screen.
                <ChunkBoundary>
                  <Suspense
                    fallback={
                      <div className="flex h-full items-center justify-center text-muted-foreground">
                        <Loader2 className="size-5 motion-safe:animate-spin" />
                      </div>
                    }
                  >
                    <CodeEditor
                      path={activeTab.path}
                      value={activeBuffer?.draft ?? ""}
                      onChange={handleDraftChange}
                      onSave={save}
                      onReady={(mon) => {
                        monacoRef.current = mon
                      }}
                    />
                  </Suspense>
                </ChunkBoundary>
              )}
            </div>
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>

      {/* New File… / New Folder…, Rename…, Delete…: driven by the file
          tree's right-click context menu (and the header FilePlus button,
          which targets the root). Local EditorBody state, not store targets
          (see the newEntryTarget/renameEntryTarget/deleteEntryTarget
          declarations above for why useVanishedTargetGuard doesn't apply
          here). */}
      <NewEntryDialog
        target={newEntryTarget}
        onClose={() => setNewEntryTarget(null)}
        onSubmit={handleNewEntrySubmit}
      />
      <RenameEntryDialog
        target={renameEntryTarget}
        isDirty={
          renameEntryTarget !== null &&
          hasDirtyUnderPath(
            tabsState ?? { tabs: [], activeId: null },
            renameEntryTarget.path,
          )
        }
        onClose={() => setRenameEntryTarget(null)}
        onSubmit={handleRenameSubmit}
      />
      <MoveEntryDialog
        sessionId={sessionId}
        target={moveEntryTarget}
        isDirty={
          moveEntryTarget !== null &&
          hasDirtyUnderPath(
            tabsState ?? { tabs: [], activeId: null },
            moveEntryTarget.path,
          )
        }
        onClose={() => setMoveEntryTarget(null)}
        onSubmit={handleMoveSubmit}
      />
      <FileInfoDialog
        sessionId={sessionId}
        target={fileInfoTarget}
        onClose={() => setFileInfoTarget(null)}
      />
      <DeleteEntryDialog
        target={deleteEntryTarget}
        blockedBySave={
          deleteEntryTarget !== null && savingPaths.has(deleteEntryTarget.path)
        }
        onClose={() => setDeleteEntryTarget(null)}
        onConfirm={handleDeleteConfirm}
      />
    </>
  )
}

// Read-only preview for a raster image tab: renders straight from /raw (no
// /read, no buffer; see the isImageTab derivation in EditorBody). The /raw
// 25 MiB cap governs; when the request is refused (or the bytes are not a
// decodable image) the <img> errors and the pane swaps to the error text
// with a Retry. Retry bumps `attempt`, which keys the <img>, so a FRESH
// element re-fires the request at the SAME URL (no cache-busting param;
// /raw already sends Cache-Control: no-cache). The caller keys the whole
// pane by path so all of this state resets on a preview-replace.
//
// The caption shows path + pixel dimensions once loaded. The plan asked for
// path + byte size, but the pane deliberately never fetches the file, so the
// byte size is unknowable here; naturalWidth/naturalHeight are what the
// render itself knows.
function ImagePreviewPane({ src, path }: { src: string; path: string }) {
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null)
  if (failed) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-sm text-destructive">
        <span>
          This image could not be loaded. One possibility is that it exceeds
          the server&rsquo;s raw-file size cap.
        </span>
        {/* Mirrors the file-error arm's manual Retry. */}
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            setFailed(false)
            setDims(null)
            setAttempt((a) => a + 1)
          }}
        >
          <RotateCw />
          Retry
        </Button>
      </div>
    )
  }
  return (
    <PreviewImageFrame
      caption={dims === null ? path : `${path} · ${dims.w} × ${dims.h}`}
    >
      <img
        key={attempt}
        src={src}
        alt={path}
        className="max-h-full max-w-full object-contain"
        onLoad={(e) =>
          setDims({
            w: e.currentTarget.naturalWidth,
            h: e.currentTarget.naturalHeight,
          })
        }
        onError={() => setFailed(true)}
      />
    </PreviewImageFrame>
  )
}

// Draft-accurate SVG preview: the Blob URL is rebuilt on every draft change
// and revoked by useObjectUrl, so unsaved edits render exactly like the
// markdown preview's. A draft that is not (yet) valid SVG shows the browser's
// broken-image state until the next keystroke fixes it; that is honest for a
// live preview of text mid-edit.
function SvgPreviewPane({ draft, path }: { draft: string; path: string }) {
  const url = useObjectUrl(draft, "image/svg+xml")
  return (
    <PreviewImageFrame caption={path}>
      {url !== null && (
        <img src={url} alt={path} className="max-h-full max-w-full object-contain" />
      )}
    </PreviewImageFrame>
  )
}

// Shared frame for the two image panes: the image lives in a min-h-0 flex-1
// box and the caption is a fixed (shrink-0) sibling, so a portrait image can
// never push the caption out of the pane or force a surprise scrollbar.
function PreviewImageFrame({
  caption,
  children,
}: {
  caption: string
  children: React.ReactNode
}) {
  return (
    <div className="flex h-full min-h-0 flex-col items-center gap-2 bg-muted/30 p-4">
      <div className="flex min-h-0 w-full flex-1 items-center justify-center">
        {children}
      </div>
      <span className="max-w-full shrink-0 truncate font-mono text-xs text-muted-foreground">
        {caption}
      </span>
    </div>
  )
}
