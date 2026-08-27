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
  Code2,
  Eye,
  FileCode2,
  FilePlus,
  FileText,
  GitCompare,
  Laptop,
  Loader2,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  RotateCw,
  Save,
  Search,
  Wand2,
  X,
} from "lucide-react"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { notify, notifyBusy, notifyError, notifySuccess } from "@/lib/notify"
import { fileApi } from "@/lib/fileApi"
import { OPEN_IN_EDITORS } from "@/lib/editors"
import {
  changeSignalFor,
  isBufferStale,
  pruneByIds,
  pruneSetByIds,
  unionRevalidateBatch,
} from "@/lib/editorBuffers"
import type { ChangesSliceView, TabBuffer } from "@/lib/editorBuffers"
import { isAllDeleteDiff } from "@/lib/diffPresentation"
import { loadRootDrafts, storeRootDrafts } from "@/lib/editorDrafts"
import {
  rootHasDiff,
  rootKey,
  rootPtyId,
  rootSessionId,
  type EditorRoot,
} from "@/lib/editorRoot"
import { hasDirtyUnderPath, shouldPromoteOnEdit } from "@/lib/editorTabs"
import type { EditorTab } from "@/lib/editorTabs"
import {
  joinName,
  parentDir,
  renameTarget as computeRenameTarget,
} from "@/lib/fileTreeOps"
import { performMove } from "@/lib/moveEntry"
import {
  createdMessage,
  deletedMessage,
  renamedMessage,
} from "@/lib/editorMutations"
import {
  effectiveLanguageLabel,
  languageOverrideFor,
  languagePickerEntries,
  pruneLanguageOverrides,
  retargetLanguageOverrides,
  withLanguageOverride,
} from "@/lib/editorLanguage"
import type { RegisteredLanguage } from "@/lib/editorLanguage"
import { isLocalAccessHost } from "@/lib/localAccess"
import {
  EDITOR_CONTENT_MIN_SIZE_PROP,
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  editorMountLayout,
  EXPLORER_LAYOUT_KEY,
  EXPLORER_MIN_SIZE_PROP,
  EXPLORER_PANEL_ID,
  explorerExpandTarget,
  explorerMountSize,
  isExplorerCollapsed,
  nextExpandedExplorerPx,
  parseExplorerLayout,
  serializeExplorerLayout,
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
import { ConfirmReloadFileDialog } from "@/components/ConfirmReloadFileDialog"
import type { ReloadFileTarget } from "@/components/ConfirmReloadFileDialog"
import { EditorIcon } from "@/components/EditorIcon"
import { FileDiskBanner } from "@/components/FileDiskBanner"
import { SaveConflictDialog } from "@/components/SaveConflictDialog"
import { useEditorDiffReads } from "@/components/useEditorDiffReads"
import { useEditorFileReads } from "@/components/useEditorFileReads"
import {
  createEditorDiskBannerActions,
  useEditorDiskFreshness,
} from "@/components/useEditorDiskFreshness"
import { useEditorSave } from "@/components/useEditorSave"
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
  DropdownMenuSeparator,
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
  editorCloseTab,
  editorCloseTabsUnderPath,
  editorOpenFile,
  editorPinTab,
  editorRenameTabPaths,
  editorSetTabDirty,
  editorSetTabMode,
  editorSyncActiveTab,
  openEditorCloseTab,
  standaloneEditorHash,
  useDux,
  type ChangesSlice,
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

// The two lines of I/O around the explorer's persisted width. The decisions
// (what the value means, what an unrecognised one does) are all in the pure
// helpers in lib/editorLayout.ts; these only touch storage.
//
// Both swallow: `localStorage` THROWS on access in a browser with site data
// disabled, and an explorer that fails to remember its width is not a reason
// to fail to open the editor.
function readExplorerLayoutRaw(): string | null {
  try {
    return localStorage.getItem(EXPLORER_LAYOUT_KEY)
  } catch {
    return null
  }
}

function writeExplorerLayout(px: number | null, collapsed: boolean): void {
  // Nothing worth remembering yet: the panel has never reported a usable
  // width (a first open that was collapsed before it was ever dragged), and
  // writing a placeholder would just be discarded on the way back in.
  if (px === null) return
  try {
    localStorage.setItem(
      EXPLORER_LAYOUT_KEY,
      serializeExplorerLayout({ px, collapsed }),
    )
  } catch {
    // Storage refused; the width lasts for this session only.
  }
}

interface EditorBodyProps {
  // What this editor is rooted at: an agent's worktree, or the directory a
  // terminal was spawned in. Every file call, tab-list lookup and draft-cache
  // entry keys off it rather than off a bare id.
  root: EditorRoot
  // True when composed by StandaloneEditorShell: the body IS the tab, so the
  // header drops its Close button (there is nothing to close into; the
  // shell's open-in-dux link is the way out) and its open-in-new-tab anchor
  // (this already is that tab), and on phones the explorer starts collapsed.
  standalone?: boolean
}

export function EditorBody({ root, standalone = false }: EditorBodyProps) {
  const { bootstrap, changes, editorTarget, editorTabs } = useDux()
  // The namespaced key for every per-root map, and the stable dependency for
  // every effect below: the root itself is a fresh object on each render.
  const key = rootKey(root)
  // Whether this editor has a diff view at all. A terminal root does not: it is
  // a plain directory with no HEAD behind it and no diff route on the server,
  // so every diff affordance below is ABSENT rather than disabled, and the
  // store refuses the mode even when an address asks for it.
  const hasDiff = rootHasDiff(root)
  const tabsState = editorTabs[key]
  const tabs = useMemo(() => tabsState?.tabs ?? [], [tabsState])
  const activeTab = tabs.find((t) => t.id === tabsState?.activeId) ?? null
  const {
    mode: activeTabMode,
    path: activeTabPath,
    dirty,
  } = activeTabFacts(activeTab)

  // Per-tab Monaco buffers (file content + diff cache), keyed by tab id, kept
  // OUT of the global store deliberately (see lib/editorTabs.ts header
  // comment): putting file contents in zustand-style global state would fire
  // a store-wide update on every keystroke. Seeded from the module-level
  // draft cache (lib/editorDrafts.ts) and written back below, which is what
  // lets an unsaved draft survive the editor being closed and reopened.
  const [buffers, setBuffers] = useState<Map<string, TabBuffer>>(() =>
    loadRootDrafts(key),
  )
  const activeBuffer = bufferForTab(buffers, activeTab)

  // Mirror every buffer change into the draft cache. The cache outlives this
  // component; the store prunes it when tabs close and clears it on session
  // delete, so this side is write-only.
  useEffect(() => {
    storeRootDrafts(key, buffers)
  }, [key, buffers])

  // Report the active tab up as the editor's live URL position: the address
  // bar names the file (and mode) actually on screen, and in-editor switches
  // REPLACE the history entry (see the store's `editorSyncActiveTab`). No
  // active tab reports a pathless open editor.
  useEffect(() => {
    editorSyncActiveTab(root, activeTabMode, activeTabPath)
    // `root` is a fresh object every render, so the KEY is the dependency; a
    // root that keys the same is the same root.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, activeTabMode, activeTabPath])

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
  // "Saved" toast (see `saveResolutionOutcome`).
  const tabsRef = useRef<typeof tabs>(tabs)
  useEffect(() => {
    tabsRef.current = tabs
  })

  // The live buffers and changed-files slice, for the same reason `tabsRef`
  // exists: the freshness check and the save both resolve LATER, and they must
  // decide against the buffer and the slice as they are at that moment, not as
  // they were when the request went out.
  const buffersRef = useRef(buffers)
  useEffect(() => {
    buffersRef.current = buffers
  })

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
  // The two disk-freshness dialogs. Both are local like the four above, but
  // unlike them they DO have a live truth to close on, so each is given one:
  // the reload confirm has a `present` predicate recomputed from the buffer
  // (see below), and the save conflict closes itself when its tab goes away.
  const [reloadTarget, setReloadTarget] = useState<ReloadFileTarget | null>(
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

  // The language picker's two pieces of state.
  //
  // The REGISTRY, read off Monaco once the editor surface is actually up. It
  // is fetched through a DYNAMIC import so this module never pulls the
  // multi-MB Monaco bundle into the main chunk; the import resolves to the
  // same chunk CodeEditor and DiffViewer already lazy-load, so by the time a
  // text tab is on screen it costs nothing extra. It is read in an effect
  // rather than from CodeEditor's `onReady` because the picker must also work
  // in DIFF mode, where CodeEditor never mounts.
  const [registeredLanguages, setRegisteredLanguages] = useState<
    RegisteredLanguage[]
  >([])
  // The OVERRIDES, keyed by tab PATH, one entry per file the user has
  // corrected. Session-lived and deliberately NOT persisted: a language
  // override is a reaction to what is on screen right now, and a file
  // reopened later re-infers, which is the behavior a user gets from every
  // other editor. It also means there is no stored shape to migrate when
  // Monaco's language ids move. Closing the editor clears it with the
  // component.
  const [languageOverrides, setLanguageOverrides] = useState<
    Map<string, string>
  >(() => new Map())

  // Explorer panel layout, persisted by dux rather than by the panel
  // library's `useDefaultLayout`. That hook is the reuse-before-invent
  // answer and it was the previous one, but what it persists is a `Layout`,
  // which is percentages by definition, and a percentage is precisely what
  // makes the explorer two different widths in the two shells. See
  // lib/editorLayout.ts's header. Read ONCE, at mount, into state: the panel
  // props below are only consulted at mount too, so a value that changed
  // mid-session would be a lie either way.
  const [storedExplorer] = useState(() =>
    parseExplorerLayout(readExplorerLayoutRaw()),
  )
  // On a phone the STANDALONE editor starts with the explorer collapsed
  // (settled decision: phone standalone is best-effort, and the viewport
  // barely fits Monaco alone). Read at mount by editorMountLayout below;
  // a later resize or rotation must not re-collapse an explorer the user
  // has since expanded, and it cannot, because a defaultLayout only applies
  // at mount.
  const isMobile = useIsMobile()
  const startExplorerCollapsed = shouldStartExplorerCollapsed(
    standalone,
    isMobile,
    storedExplorer?.collapsed,
  )
  // What the group actually mounts with: undefined in the ordinary case, so
  // the explorer panel's own PIXEL defaultSize decides its width, or a true
  // 0%/100% collapse when this open starts collapsed. Structural on purpose:
  // mounting collapsed (rather than relying only on the belt-and-braces
  // collapse() effect below) leaves no frame where the explorer renders
  // expanded and no race with the library's initial layout.
  const mountLayout = editorMountLayout(startExplorerCollapsed)
  // The header toggle's icon/label state. Seeded from the same flag the mount
  // layout is (so a collapsed explorer stays collapsed across opens, and the
  // phone standalone's forced-collapsed mount shows "Show" from the first
  // frame) and kept current by onLayoutChanged, which fires for drag-collapse
  // and toggle alike.
  const [explorerCollapsed, setExplorerCollapsed] = useState(
    startExplorerCollapsed,
  )
  const explorerPanelRef = useRef<PanelImperativeHandle | null>(null)
  // The last width the explorer had while expanded, IN PIXELS, seeded from
  // storage and folded forward on every layout report. Toggling open resizes
  // to THIS rather than calling `panel.expand()`: expand() falls back to
  // minSize when it has no in-memory expand size (a fresh page load after
  // collapsing), which would reopen the explorer at the minimum.
  const lastExpandedExplorerPxRef = useRef<number | null>(
    storedExplorer?.px ?? null,
  )
  function toggleExplorer(): void {
    const panel = explorerPanelRef.current
    if (!panel) return
    if (panel.isCollapsed()) {
      // On a phone the expand target ignores the remembered pixel width and
      // opens to the widest width the content pane's minimum permits, as a
      // percentage. See explorerExpandTarget.
      panel.resize(
        explorerExpandTarget(lastExpandedExplorerPxRef.current, isMobile),
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

  // Raster images never fetch /read (the 5 MiB /read cap refuses them before
  // the binary flag exists, and fileReady would park the tab on a spinner);
  // they render a read-only pane from /raw instead, and the buffer-derived
  // toolbar controls (Save, the preview toggle) do not render for them. SVG
  // is deliberately NOT an image tab: it opens in Monaco as editable text.
  const isImageTab = tabIsImage(activeTab)
  // Which draft-accurate preview this tab's TEXT can offer: "markdown"
  // (react-markdown) or "svg" (a Blob URL over the current draft), null when
  // the Preview toggle should not render at all.
  const activePreviewKind = tabPreviewKind(activeTab)
  const fileReady = bufferIsFileReady(activeTab, activeBuffer)
  const binary = readyBufferFlag(fileReady, activeBuffer?.binary)
  const readOnly = readyBufferFlag(fileReady, activeBuffer?.readOnly)
  // File-mode readiness for the preview toggle: a loaded, non-binary buffer.
  // The toggle itself is available in BOTH modes; its diff-mode half needs
  // `diffReady`, declared further down, so `canPreview` lives beside it (one
  // source of truth for both the toggle button and the render).
  const filePreviewReady = previewableFileIsReady(fileReady, binary)
  // "Open editor" spawns a GUI editor on the SERVER, so it only helps when the
  // server is the user's own machine. Enable for local-access URLs; for remote
  // URLs keep the control but disable it with an explanatory tooltip.
  const localAccess = isLocalAccessHost(window.location.hostname)

  // The language picker, derived. It renders only for a TEXT tab (an image
  // has no language to pick) and only once the registry has been read: an
  // empty list would give a control whose menu is empty and whose trigger
  // always reads "Plain text".
  const languageTabPath = languagePath(activeTab, isImageTab)
  useEffect(() => {
    if (languageTabPath === null) return
    let cancelled = false
    // The registry does not change after the grammars register at import
    // time, so this runs once and then short-circuits on the state check.
    void import("@/lib/monacoSetup")
      .then((m) => {
        if (!cancelled) setRegisteredLanguages(m.monaco.languages.getLanguages())
      })
      // Monaco failing to load is already fatal for the pane behind us, which
      // reports it; the picker simply does not appear.
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [languageTabPath])
  const languageChoices = useMemo(
    () => languagePickerEntries(registeredLanguages),
    [registeredLanguages],
  )
  const activeLanguageOverride = languageOverrideFor(
    languageOverrides,
    languageTabPath,
  )
  const activeLanguageLabel = effectiveLanguageLabel(
    languageOverrides,
    languageTabPath,
    registeredLanguages,
  )
  const showLanguagePicker = hasLanguageChoices(languageTabPath, languageChoices)
  // Follow a renamed or moved file, so a correction the user just made is not
  // silently reverted, and so the old key is not left behind for whatever file
  // lands on that path next. Mirrors `editorTabs.renameTabPaths`, directory
  // prefixes included.
  function retargetOverrides(from: string, to: string): void {
    setLanguageOverrides((prev) => retargetLanguageOverrides(prev, from, to))
  }
  function pickLanguage(id: string | null): void {
    if (languageTabPath === null) return
    setLanguageOverrides((prev) =>
      withLanguageOverride(prev, languageTabPath, id),
    )
  }

  // The changed-files slice, trusted only when it belongs to THIS editor's
  // session (an agent editor always operates on the selected session, but a
  // fast switch could momentarily leave the slice pointed elsewhere). A
  // TERMINAL root has no session, so it never gets a slice: no diff mode, no
  // changed-file decorations from the broadcast, and freshness rides window
  // focus and tab activation instead.
  const slice = changesForRoot(changes, root)

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
  // Not memoized: two small array scans over the changes slice is cheap, and
  // wrapping it in `useMemo` here fought `eslint-plugin-react-hooks`'
  // compiler-derived lint rules (it flagged the manual dependency array as
  // stale relative to its analysis). Note the build does NOT run
  // babel-plugin-react-compiler, so there is no runtime auto-memoization
  // here. This expression genuinely re-evaluates on every render; the two
  // scans are just cheap enough that that's fine.
  const openFileSignal = changeSignalFor(slice, activeTabPath)
  const openFileSignalRef = useRef("")
  // The slice as of the last render, for the callbacks that resolve later and
  // need the signal for a path that may not be the active tab's.
  const sliceRef = useRef<ChangesSliceView | null>(slice)
  useEffect(() => {
    sliceRef.current = slice
  })

  const { raiseDiskBanner, dismissDiskBanner } =
    createEditorDiskBannerActions(setBuffers)

  const {
    savingTabId,
    savingPaths,
    saveConflict,
    setSaveConflict,
    writeBuffer,
  } = useEditorSave({
    root,
    tabsRef,
    sliceRef,
    setBuffers,
    raiseDiskBanner,
  })

  // The diff is cached per tab+path; ready when the loaded diff is for the
  // active tab's CURRENT path. While ready, a change-signal differing from the
  // one captured at load means the file changed underneath, surface a reload
  // button (diffStale).
  const diffReady = bufferIsDiffReady(activeTab, activeBuffer)
  const diffStale = diffBufferIsStale(diffReady, openFileSignal, activeBuffer)
  // Diff-mode readiness for the preview: the diff is loaded for the tab's
  // current path and neither side is binary (the `fileReady && !binary`
  // equivalent for a tab whose only content is the diff cache).
  const diffPreviewReady = previewableDiffIsReady(diffReady, activeBuffer)
  const canPreview = previewIsAvailable(
    activePreviewKind,
    activeTab,
    diffPreviewReady,
    filePreviewReady,
  )
  const showPreview = previewIsOpen(activeTab, previewOpenTabIds, canPreview)
  // What the preview renders: always the END STATE of the file. In file mode
  // that is the draft. In diff mode the tab may have no file buffer at all,
  // so the unsaved draft wins only when one actually exists (buffer loaded
  // AND the tab is dirty); otherwise the diff's MODIFIED side (the file as on
  // disk) is exactly what file-mode preview would show.
  const previewContent = contentForPreview(
    activeTab,
    activeBuffer,
    fileReady,
    dirty,
  )
  // This tab's save is in flight.
  const isSaving = tabIsSaving(savingTabId, activeTab)

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
            root={root}
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
      .list(root)
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

  const { loadFileBuffer, reloadFileInPlace } = useEditorFileReads({
    root,
    tabs,
    activeTab,
    activeBuffer,
    tabsRef,
    buffersRef,
    sliceRef,
    setBuffers,
    raiseDiskBanner,
  })

  useEditorDiskFreshness({
    root,
    activeTab,
    activeBuffer,
    openFileSignal,
    slice,
    tabsRef,
    buffersRef,
    sliceRef,
    monacoRef,
    setBuffers,
    raiseDiskBanner,
    reloadFileInPlace,
  })

  const { loadDiffBuffer, refreshDiff } = useEditorDiffReads({
    root,
    tabs,
    activeTab,
    activeBuffer,
    tabsRef,
    loadedSignalRef: openFileSignalRef,
    setBuffers,
  })

  // Dispose and prune by the set of open paths so preview replacement and every
  // tab-scoped cache observe the same snapshot.
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
    // The language overrides are keyed by PATH, so they prune against the same
    // open-path set the model disposal above uses, and for the same reason: an
    // override is documented to last until the file is closed, and one left
    // behind is re-applied when that path is reopened (or inherited by a
    // different file that later takes it). This covers every way a tab leaves,
    // the close action and the vanished-tab paths alike, because all of them
    // end in `tabs` losing the entry. `pruneLanguageOverrides` returns the same
    // map when nothing is stale, so this is a no-op setState otherwise.
    setLanguageOverrides((prev) => pruneLanguageOverrides(prev, currentPaths))
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
    editorOpenFile(root, path, opts)
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
        editorSetTabDirty(root, tabId, newDirty)
      }
      if (shouldPromoteOnEdit(activeTab, newDirty)) editorPinTab(root, tabId)
    }
  }

  // Save the active tab. `expected` is the freshness token, present for an
  // ordinary save and deliberately ABSENT for the conflict dialog's Overwrite:
  // dropping the token is exactly what "yes, I know, do it anyway" means on
  // this route.
  function save(): void {
    if (!activeTab || binary || isSaving || !dirty) return
    writeBuffer(
      activeTab.id,
      activeTab.path,
      activeBuffer?.draft ?? "",
      activeBuffer?.stamp,
    )
  }

  // The banner's "Reload from disk". Confirmed only when there is something to
  // lose: with no unsaved edits the reload is exactly what the auto-reload
  // would have done unasked, and a confirm for "replace text you did not
  // write" is a dialog that teaches people to click through dialogs.
  function requestReload(tabId: string, path: string): void {
    const isDirty = tabsRef.current.find((t) => t.id === tabId)?.dirty ?? false
    if (!isDirty) {
      reloadFileInPlace(tabId, path, true)
      return
    }
    setReloadTarget({ tabId, path })
  }

  // Open the current file in a locally-installed GUI editor (server-side spawn).
  // `editorKey` is the one the user picked from the menu; the server launches it
  // or reports it isn't installed. Only reachable when `localAccess` is true.
  function openInEditorAction(editorKey: string): void {
    if (!activeTab || openingEditor) return
    setOpeningEditor(true)
    fileApi
      .openInEditor(root, activeTab.path, editorKey)
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
        ? fileApi.createFile(root, path)
        : fileApi.createDir(root, path)
    return create
      .then(() => {
        setNewEntryTarget(null)
        notifySuccess(createdMessage(kind, path))
        revalidateDirs([dir])
        if (kind === "file") {
          editorOpenFile(root, path, { mode: "file", pin: true })
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
      .rename(root, from, to)
      .then(() => {
        setRenameEntryTarget(null)
        notifySuccess(renamedMessage(from, to))
        editorRenameTabPaths(root, from, to)
        retargetOverrides(from, to)
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
      rename: (from, to) => fileApi.rename(root, from, to),
      clearTarget: () => setMoveEntryTarget(null),
      retargetTabs: (from, to) => {
        editorRenameTabPaths(root, from, to)
        retargetOverrides(from, to)
      },
      revalidateDirs,
      refreshSearchIndex,
      reportSuccess: (message) => notifySuccess(message),
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
      .remove(root, target.path)
      .then(() => {
        // The one confirmation that is load-bearing rather than merely
        // polite: the dialog closed the moment it was confirmed (see above),
        // so without this a delete leaves no trace on screen at all and the
        // user is left reading the tree to work out whether it happened.
        notifySuccess(deletedMessage(target.path, target.isDir))
        editorCloseTabsUnderPath(root, target.path)
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
        uploadDroppedFile(file, { pty: rootPtyId(root), conn: null, dir: into }),
      revalidateDirs,
      refreshSearchIndex,
      reportBusy: (message) => notifyBusy(message, { id: toastId }),
      // `sticky` is forwarded rather than dropped, so the decision stays in the
      // one place that makes it (`editorDropToast`, where every rung says
      // false, and says why). Hardcoding it here would put a second opinion
      // next to the first; omitting it silently makes a tree drop unpinnable
      // whatever the report asked for.
      reportFinal: (t) => notify(t.tone, t.message, { id: toastId, sticky: t.sticky }),
    })
  }

  const openPath = activeTabPath

  // The language menu's rows, shared by the desktop trigger and the phone
  // fold's submenu so the two cannot drift. "Auto" first, above a separator,
  // because it is the state every file starts in and the one a user goes
  // looking for to undo a pick; it CLEARS the override rather than selecting
  // a language, so it is checked exactly when no override is set.
  function languagePickerItems(): React.ReactNode {
    return (
      <>
        <DropdownMenuItem
          aria-current={activeLanguageOverride === undefined ? "true" : undefined}
          onClick={() => pickLanguage(null)}
        >
          <Wand2 />
          Auto
          {activeLanguageOverride === undefined && <Check className="ml-auto" />}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {languageChoices.map((choice) => (
          <DropdownMenuItem
            key={choice.id}
            aria-current={
              activeLanguageOverride === choice.id ? "true" : undefined
            }
            onClick={() => pickLanguage(choice.id)}
          >
            <Code2 />
            {choice.label}
            {activeLanguageOverride === choice.id && (
              <Check className="ml-auto" />
            )}
          </DropdownMenuItem>
        ))}
      </>
    )
  }

  function localEditorMenuItems(): React.ReactNode {
    return OPEN_IN_EDITORS.map((editor) => (
      <DropdownMenuItem
        key={editor.key}
        onClick={() => openInEditorAction(editor.key)}
      >
        <EditorIcon editorKey={editor.key} />
        {editor.label}
      </DropdownMenuItem>
    ))
  }

  function renderStatusIndicators(): React.ReactNode {
    return (
      <>
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
        {readOnly && activeTab?.mode === "file" && (
          <SimpleTooltip content="This file is read-only — it is a symlink to an external file or a .git path">
            <span className="shrink-0 text-xs text-muted-foreground">
              read-only
            </span>
          </SimpleTooltip>
        )}
      </>
    )
  }

  function renderDesktopViewActions(): React.ReactNode {
    return (
      <>
        {activeTab && !isImageTab && hasDiff && (
          <div
            className="flex shrink-0 items-center gap-0.5 rounded-md border p-0.5 max-md:hidden"
            role="group"
            aria-label="View mode"
          >
            <Button
              size="sm"
              variant={activeTab.mode === "file" ? "default" : "ghost"}
              aria-pressed={activeTab.mode === "file"}
              onClick={() => editorSetTabMode(root, activeTab.id, "file")}
            >
              <FileText />
              File
            </Button>
            <Button
              size="sm"
              variant={activeTab.mode === "diff" ? "default" : "ghost"}
              aria-pressed={activeTab.mode === "diff"}
              onClick={() => editorSetTabMode(root, activeTab.id, "diff")}
            >
              <GitCompare />
              Diff
            </Button>
          </div>
        )}
        {hasDiff && activeTab?.mode === "diff" && diffStale && (
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
        {showLanguagePicker && (
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  size="sm"
                  variant="ghost"
                  className="max-md:hidden"
                  aria-label={`Syntax language: ${activeLanguageLabel}`}
                />
              }
            >
              <Code2 />
              {activeLanguageLabel}
              <ChevronDown />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {languagePickerItems()}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </>
    )
  }

  function renderLocalEditorAction(): React.ReactNode {
    if (activeTab === null) return null
    return (
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
                <Laptop />
              )}
              Open local editor
              <ChevronDown />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {localEditorMenuItems()}
            </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </SimpleTooltip>
    )
  }

  function renderSaveAction(): React.ReactNode {
    if (activeTab?.mode !== "file" || isImageTab) return null
    return (
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
    )
  }

  function renderMobileActions(): React.ReactNode {
    if (activeTab === null) return null
    return (
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
          {!isImageTab && hasDiff && (
            <>
              <DropdownMenuItem
                aria-current={activeTab.mode === "file" ? "true" : undefined}
                onClick={() => editorSetTabMode(root, activeTab.id, "file")}
              >
                <FileText />
                File view
                {activeTab.mode === "file" && <Check className="ml-auto" />}
              </DropdownMenuItem>
              <DropdownMenuItem
                aria-current={activeTab.mode === "diff" ? "true" : undefined}
                onClick={() => editorSetTabMode(root, activeTab.id, "diff")}
              >
                <GitCompare />
                Diff view
                {activeTab.mode === "diff" && <Check className="ml-auto" />}
              </DropdownMenuItem>
            </>
          )}
          {hasDiff && activeTab.mode === "diff" && diffStale && (
            <DropdownMenuItem onClick={refreshDiff}>
              <CircleAlert />
              Reload diff
            </DropdownMenuItem>
          )}
          {canPreview && (
            <DropdownMenuItem onClick={togglePreview}>
              {showPreview && activeTab.mode === "file" ? <Pencil /> : <Eye />}
              {showPreview ? "Hide preview" : "Show preview"}
            </DropdownMenuItem>
          )}
          {showLanguagePicker && (
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <Code2 />
                {activeLanguageLabel}
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                {languagePickerItems()}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          )}
          <DropdownMenuSub>
            <DropdownMenuSubTrigger disabled={!localAccess || openingEditor}>
              <Laptop />
              Open local editor
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {localEditorMenuItems()}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  function renderStandaloneActions(): React.ReactNode {
    if (standalone) return null
    return (
      <>
        <Button
          size="sm"
          variant="ghost"
          className="shrink-0 max-md:min-h-10"
          render={
            <a
              href={standaloneEditorHash(
                root,
                activeTab ? { mode: activeTab.mode, path: activeTab.path } : null,
              )}
              target="_blank"
              rel="noopener"
            />
          }
        >
          <ExternalLink />
          Open in new tab
        </Button>
        <Button size="sm" variant="ghost" onClick={() => closeEditor()}>
          <X />
          Close
        </Button>
      </>
    )
  }

  function renderHeader(): React.ReactNode {
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
        {renderStatusIndicators()}
        {renderDesktopViewActions()}
        {renderLocalEditorAction()}
        {renderSaveAction()}
        {renderMobileActions()}
        {renderStandaloneActions()}
      </div>
      </>
    )
  }

  function renderWorkspace(): React.ReactNode {
    return (
      <>
        {/* Tab strip, only renders once the session has at least one tab. */}
        <EditorTabsStrip root={root} />

      {/* Body: worktree file tree (left, a collapsible resizable panel) +
          Monaco editor/diff (right). The outer div mirrors DesktopShell's
          panel-group mount (min-h-0 flex-1 wrapper, size-full group). */}
      <div className="min-h-0 flex-1">
        <ResizablePanelGroup
          orientation="horizontal"
          id={EDITOR_LAYOUT_ID}
          defaultLayout={mountLayout}
          // The collapse half of the report comes from the layout the library
          // hands us (a collapsed panel is zero in every unit); the WIDTH half
          // is read off the panel in PIXELS, because that is the unit dux
          // stores and the layout has no way to express it.
          onLayoutChanged={(layout) => {
            const collapsed = isExplorerCollapsed(layout)
            setExplorerCollapsed(collapsed)
            lastExpandedExplorerPxRef.current = nextExpandedExplorerPx(
              explorerPanelRef.current?.getSize().inPixels,
              lastExpandedExplorerPxRef.current,
            )
            writeExplorerLayout(lastExpandedExplorerPxRef.current, collapsed)
          }}
          className="size-full"
        >
          {/* Size props carry EXPLICIT units (see editorLayout.ts): the
              explorer in px so both shells render the same tree, the content
              pane in % as the relative half of the pair. The inline overflow:hidden
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
            defaultSize={explorerMountSize(storedExplorer)}
            minSize={EXPLORER_MIN_SIZE_PROP}
            // Keep the explorer's PIXEL width when the group resizes (a
            // window drag, the phone rotating, the modal's cap kicking in),
            // rather than rescaling it proportionally: a width that moved
            // with the container would be back to meaning different things
            // in the two shells. The content pane keeps the library's default
            // relative behavior, which is also the "at least one relative
            // panel" the group requires.
            groupResizeBehavior="preserve-pixel-size"
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
                  root={root}
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
            <div className="relative flex h-full min-w-0 flex-col">
              {/* The disk-freshness notice, above the content and inside the
                  pane: it is a fact about THIS tab, so it travels with the tab
                  rather than floating over the app as a toast. It renders
                  nothing in the ordinary case. */}
              {/* File mode only: diff mode has its own staleness affordance
                  (the header's Reload), and a diff the user is reading must
                  not be reflowed or crowded by a second one. */}
              {activeTab !== null &&
                activeTab.mode === "file" &&
                !isBufferStale(activeBuffer, activeTab.path) && (
                  <FileDiskBanner
                    state={activeBuffer?.diskState ?? "fresh"}
                    path={activeTab.path}
                    dirty={dirty}
                    onReload={() => requestReload(activeTab.id, activeTab.path)}
                    onDismiss={() =>
                      dismissDiskBanner(activeTab.id, activeTab.path)
                    }
                    // Same route the tab strip's own close takes, and here it
                    // matters more: the file is GONE, so a dirty buffer is
                    // the last copy of that text anywhere. Closing it without
                    // the confirm would be the one destructive action in the
                    // editor that never asks.
                    onCloseTab={() => {
                      if (dirty) openEditorCloseTab(root, activeTab.id)
                      else editorCloseTab(root, activeTab.id)
                    }}
                  />
                )}
              <div className="relative min-h-0 flex-1">
                <EditorContentPane
                  root={root}
                  tab={activeTab}
                  buffer={activeBuffer}
                  image={isImageTab}
                  fileReady={fileReady}
                  binary={binary}
                  diffReady={diffReady}
                  preview={showPreview}
                  language={activeLanguageOverride}
                  renderPreview={renderPreviewPane}
                  onRetryFile={loadFileBuffer}
                  onRetryDiff={loadDiffBuffer}
                  onDraftChange={handleDraftChange}
                  onSave={save}
                  onMonacoReady={(monaco) => {
                    monacoRef.current = monaco
                  }}
                />
              </div>
            </div>
          </ResizablePanel>
        </ResizablePanelGroup>
        </div>
      </>
    )
  }

  function renderDialogs(): React.ReactNode {
    return (
      <>
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
        root={root}
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
        root={root}
        target={fileInfoTarget}
        onClose={() => setFileInfoTarget(null)}
      />
      {/* Discarding your own edits for what is on disk. The vanished-target
          predicate is "still worth asking": the tab still exists, is still
          dirty, and the file is still reported changed. Saving, reverting, or
          reloading from another surface while this is open all retire it. */}
      <ConfirmReloadFileDialog
        target={reloadTarget}
        present={
          reloadTarget !== null &&
          (tabs.find((t) => t.id === reloadTarget.tabId)?.dirty ?? false) &&
          buffers.get(reloadTarget.tabId)?.diskState === "changed"
        }
        onClose={() => setReloadTarget(null)}
        onConfirm={() => {
          reloadFileInPlace(reloadTarget!.tabId, reloadTarget!.path, true)
          setReloadTarget(null)
        }}
      />
      {/* The 409. Its own target vanishes with the tab (nothing else can
          resolve a conflict that is waiting on an answer), so the guard is a
          plain tab lookup rather than the shared hook's two-value form. */}
      <SaveConflictDialog
        target={
          saveConflict !== null &&
          tabs.some((t) => t.id === saveConflict.tabId)
            ? saveConflict
            : null
        }
        onClose={() => setSaveConflict(null)}
        onOverwrite={() => {
          const target = saveConflict!
          setSaveConflict(null)
          // No token: the whole meaning of Overwrite is to save without the
          // guard, and the body is the one the refused save carried.
          writeBuffer(target.tabId, target.path, target.body)
        }}
        onReload={() => {
          const target = saveConflict!
          setSaveConflict(null)
          // Straight into the destructive confirm: taking the disk version
          // throws away everything the user typed, and that is the same act
          // the banner's Reload asks about.
          requestReload(target.tabId, target.path)
        }}
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

  return (
    <>
      {renderHeader()}
      {renderWorkspace()}
      {renderDialogs()}
    </>
  )
}

interface ActiveTabFacts {
  mode: EditorTab["mode"]
  path: string | null
  dirty: boolean
}

function activeTabFacts(tab: EditorTab | null): ActiveTabFacts {
  return {
    mode: tab?.mode ?? "file",
    path: tab?.path ?? null,
    dirty: tab?.dirty ?? false,
  }
}

function bufferForTab(
  buffers: ReadonlyMap<string, TabBuffer>,
  tab: EditorTab | null,
): TabBuffer | undefined {
  return tab === null ? undefined : buffers.get(tab.id)
}

function shouldStartExplorerCollapsed(
  standalone: boolean,
  mobile: boolean,
  stored: boolean | undefined,
): boolean {
  return (standalone && mobile) || (stored ?? false)
}

function tabIsImage(tab: EditorTab | null): boolean {
  return tab !== null && isImagePreviewPath(tab.path)
}

function tabPreviewKind(tab: EditorTab | null): ReturnType<typeof previewKind> {
  return tab === null ? null : previewKind(tab.path)
}

function bufferIsFileReady(
  tab: EditorTab | null,
  buffer: TabBuffer | undefined,
): boolean {
  return (
    tab !== null &&
    buffer !== undefined &&
    !isBufferStale(buffer, tab.path) &&
    buffer.loadedPath === tab.path
  )
}

function readyBufferFlag(ready: boolean, flag: boolean | undefined): boolean {
  return ready ? (flag ?? false) : false
}

function previewableFileIsReady(ready: boolean, binary: boolean): boolean {
  return ready && !binary
}

function languagePath(tab: EditorTab | null, image: boolean): string | null {
  return tab !== null && !image ? tab.path : null
}

function hasLanguageChoices(
  path: string | null,
  choices: readonly unknown[],
): boolean {
  return path !== null && choices.length > 0
}

function changesForRoot(
  changes: ChangesSlice,
  root: EditorRoot,
): ChangesSliceView | null {
  return changes.sessionId === rootSessionId(root) ? changes : null
}

function bufferIsDiffReady(
  tab: EditorTab | null,
  buffer: TabBuffer | undefined,
): boolean {
  return (
    tab !== null &&
    buffer !== undefined &&
    !isBufferStale(buffer, tab.path) &&
    buffer.diffLoadedPath === tab.path
  )
}

function diffBufferIsStale(
  ready: boolean,
  signal: string,
  buffer: TabBuffer | undefined,
): boolean {
  return ready && signal !== (buffer?.diffLoadedSignal ?? "")
}

function previewableDiffIsReady(
  ready: boolean,
  buffer: TabBuffer | undefined,
): boolean {
  return ready && !(buffer?.diff?.binary ?? false)
}

function previewIsAvailable(
  kind: ReturnType<typeof previewKind>,
  tab: EditorTab | null,
  diffReady: boolean,
  fileReady: boolean,
): boolean {
  return kind !== null && (tab?.mode === "diff" ? diffReady : fileReady)
}

function previewIsOpen(
  tab: EditorTab | null,
  openTabIds: ReadonlySet<string>,
  available: boolean,
): boolean {
  return tab !== null && openTabIds.has(tab.id) && available
}

function contentForPreview(
  tab: EditorTab | null,
  buffer: TabBuffer | undefined,
  fileReady: boolean,
  dirty: boolean,
): string {
  if (tab?.mode === "diff" && !(fileReady && dirty)) {
    return buffer?.diff?.modified ?? ""
  }
  return buffer?.draft ?? ""
}

function tabIsSaving(savingTabId: string | null, tab: EditorTab | null): boolean {
  return savingTabId !== null && savingTabId === tab?.id
}

interface EditorContentPaneProps {
  root: EditorRoot
  tab: EditorTab | null
  buffer: TabBuffer | undefined
  image: boolean
  fileReady: boolean
  binary: boolean
  diffReady: boolean
  preview: boolean
  language: string | undefined
  renderPreview: (path: string) => React.ReactNode
  onRetryFile: (tabId: string, path: string) => void
  onRetryDiff: (tabId: string, path: string) => void
  onDraftChange: (value: string) => void
  onSave: () => void
  onMonacoReady: (monaco: MonacoInstance) => void
}

function EditorContentPane(props: EditorContentPaneProps) {
  const { root, tab, image } = props
  if (tab === null) {
    return (
      <CenteredPane>Select a file from the tree to view or edit it.</CenteredPane>
    )
  }
  // Images have no text buffer and win even if stale state says "diff".
  if (image) {
    return (
      <ImagePreviewPane
        key={tab.path}
        src={fileApi.rawUrl(root, tab.path)}
        path={tab.path}
      />
    )
  }
  if (tab.mode === "diff") return <DiffContentPane {...props} tab={tab} />
  return <FileContentPane {...props} tab={tab} />
}

interface ActiveEditorContentProps extends EditorContentPaneProps {
  tab: EditorTab
}

function DiffContentPane({
  tab,
  buffer,
  diffReady,
  preview,
  language,
  renderPreview,
  onRetryDiff,
}: ActiveEditorContentProps) {
  if (buffer?.diffError && !isBufferStale(buffer, tab.path)) {
    return (
      <ErrorPane
        message={buffer.diffError}
        onRetry={() => onRetryDiff(tab.id, tab.path)}
      />
    )
  }
  if (!diffReady) return <LoadingPane />
  if (buffer?.diff?.binary) {
    return (
      <CenteredPane>
        This file is binary and can&rsquo;t be diffed here.
      </CenteredPane>
    )
  }
  if (preview) return <>{renderPreview(tab.path)}</>

  const diff = buffer?.diff
  const allDelete = diff !== null && diff !== undefined && isAllDeleteDiff(diff)
  return (
    <div className={allDelete ? "dux-diff-all-delete h-full" : "h-full"}>
      <ChunkBoundary>
        <Suspense fallback={<LoadingPane />}>
          <DiffViewer
            path={tab.path}
            language={language}
            original={diff?.original ?? ""}
            modified={diff?.modified ?? ""}
            allDelete={allDelete}
          />
        </Suspense>
      </ChunkBoundary>
    </div>
  )
}

function FileContentPane({
  tab,
  buffer,
  fileReady,
  binary,
  preview,
  language,
  renderPreview,
  onRetryFile,
  onDraftChange,
  onSave,
  onMonacoReady,
}: ActiveEditorContentProps) {
  if (buffer?.fileError && !isBufferStale(buffer, tab.path)) {
    return (
      <ErrorPane
        message={buffer.fileError}
        onRetry={() => onRetryFile(tab.id, tab.path)}
      />
    )
  }
  if (!fileReady) return <LoadingPane />
  if (binary) {
    return (
      <CenteredPane>
        This file is binary and can&rsquo;t be edited here.
      </CenteredPane>
    )
  }
  if (preview) return <>{renderPreview(tab.path)}</>
  return (
    <ChunkBoundary>
      <Suspense fallback={<LoadingPane />}>
        <CodeEditor
          path={tab.path}
          language={language}
          value={buffer?.draft ?? ""}
          onChange={onDraftChange}
          onSave={onSave}
          onReady={onMonacoReady}
        />
      </Suspense>
    </ChunkBoundary>
  )
}

function LoadingPane() {
  return (
    <div className="flex h-full items-center justify-center text-muted-foreground">
      <Loader2 className="size-5 motion-safe:animate-spin" />
    </div>
  )
}

function CenteredPane({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
      {children}
    </div>
  )
}

function ErrorPane({
  message,
  onRetry,
}: {
  message: string
  onRetry: () => void
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-sm text-destructive">
      {message}
      <Button size="sm" variant="outline" onClick={onRetry}>
        <RotateCw />
        Retry
      </Button>
    </div>
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
