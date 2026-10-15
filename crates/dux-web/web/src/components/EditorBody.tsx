// The editor BODY: the header row, the tab strip, the explorer panel, and
// the Monaco/diff/preview panes. Two shells compose it: the desktop overlay
// Dialog (EditorOverlay.tsx) and the standalone whole-tab surface
// (StandaloneEditor.tsx). Exactly one of the two mounts it at a time (the
// overlay stands down while the tab is the standalone surface), so there is
// never a second Monaco model set or a second buffer map over the same files.
import { changedStatusLookup } from "@/lib/changedFiles"
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react"
import {
  Check,
  ChevronDown,
  ChevronLeft,
  CircleAlert,
  Ellipsis,
  ExternalLink,
  Code2,
  Eye,
  FileCode2,
  FilePlus,
  FileText,
  FileType,
  GitCompare,
  Laptop,
  Loader2,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  RefreshCw,
  RotateCw,
  Save,
  SaveAll,
  Search,
  SquareTerminal,
  Wand2,
  X,
} from "lucide-react"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { notify, notifyBusy, notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"
import { fileApi } from "@/lib/fileApi"
import { OPEN_IN_EDITORS } from "@/lib/editors"
import {
  bufferReadEncoding,
  bufferSaveEncoding,
  isBufferDirty,
  changeSignalFor,
  isBufferStale,
  pruneByIds,
  pruneSetByIds,
  unionRevalidateBatch,
} from "@/lib/editorBuffers"
import type { ChangesSliceView, TabBuffer } from "@/lib/editorBuffers"
import { isAllDeleteDiff } from "@/lib/diffPresentation"
import { diffHeadBanner } from "@/lib/diffHeadBanner"
import { loadRootDrafts, storeRootDrafts } from "@/lib/editorDrafts"
import { laneKey, useLaneBusy } from "@/lib/editorLane"
import {
  REOPEN_CHOICES,
  SAVE_CHOICES,
  dismissNotice,
  encodingNotices,
  encodingLabel,
  noticeDismissed,
  sameSaveEncoding,
  type SaveEncoding,
} from "@/lib/editorEncoding"
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
import { deletedMessage } from "@/lib/editorMutations"
import {
  effectiveLanguageLabel,
  languageOverrideFor,
  languagePickerEntries,
  pruneLanguageOverrides,
  retargetLanguageOverrides,
  withLanguageOverride,
} from "@/lib/editorLanguage"
import type { RegisteredLanguage } from "@/lib/editorLanguage"
import { loadRegisteredLanguages } from "@/lib/monacoLanguages"
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
import { planStatusBar, type ImageDimensions } from "@/lib/editorStatus"
import {
  editorTitleProse,
  standaloneEditorName,
} from "@/lib/standaloneEditorName"
import { performTreeDrop } from "@/lib/editorDrop"
import type { DroppedItems } from "@/lib/editorDrop"
import { nextFileDropToastId } from "@/lib/fileDrop"
import { uploadDroppedFile } from "@/lib/fileDropApi"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { useObjectUrl } from "@/hooks/use-object-url"
import { useSearchPick } from "@/hooks/use-search-pick"
import type { MonacoInstance } from "@/components/CodeEditor"
import { DeleteEntryDialog } from "@/components/DeleteEntryDialog"
import type { DeleteEntryTarget } from "@/components/DeleteEntryDialog"
import { ConfirmReloadFileDialog } from "@/components/ConfirmReloadFileDialog"
import type { ReloadFileTarget } from "@/components/ConfirmReloadFileDialog"
import { EditorIcon } from "@/components/EditorIcon"
import { FileDiskBanner } from "@/components/FileDiskBanner"
import { EncodingNotice } from "@/components/EncodingNotice"
import {
  ConfirmReopenEncodingDialog,
  type ReopenEncodingTarget,
} from "@/components/ConfirmReopenEncodingDialog"
import { SaveConflictDialog } from "@/components/SaveConflictDialog"
import { useEditorDiffReads } from "@/components/useEditorDiffReads"
import { useEditorFileReads } from "@/components/useEditorFileReads"
import {
  createEditorDiskBannerActions,
  useEditorDiskFreshness,
} from "@/components/useEditorDiskFreshness"
import { useEditorSave } from "@/components/useEditorSave"
import { EditorTabsStrip } from "@/components/EditorTabsStrip"
import {
  EditorStatusBar,
  type StatusBarEditor,
} from "@/components/EditorStatusBar"
import { InlineCode } from "@/components/ui/inline-code"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { FileTree } from "@/components/FileTree"
import { useFileTreeFreshness } from "@/components/useFileTreeFreshness"
import { FileInfoDialog } from "@/components/FileInfoDialog"
import type { FileInfoTarget } from "@/components/FileInfoDialog"
import { MoveEntryDialog } from "@/components/MoveEntryDialog"
import type { MoveEntryTarget } from "@/components/MoveEntryDialog"
import { NewEntryDialog } from "@/components/NewEntryDialog"
import type { NewEntryTarget } from "@/components/NewEntryDialog"
import { RenameEntryDialog } from "@/components/RenameEntryDialog"
import type { RenameEntryTarget } from "@/components/RenameEntryDialog"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { StartTruncatedText } from "@/components/StartTruncatedText"
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
  leaveStandaloneEditor,
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
const DiffHeadViewer = lazy(() => import("./DiffHeadViewer"))
// react-markdown is only needed when previewing a markdown file, so lazy-load it
// into its own chunk so it never weighs on the main bundle or the editor open.
const MarkdownPreview = lazy(() => import("./MarkdownPreview"))

// Cap how many results the search list renders so a 1-char query in a huge repo
// can't mount thousands of rows.
const MAX_SEARCH_RESULTS = 300

// The header's one height token for its labelled controls, which all wear the
// outline variant: the icon-only ones take the matching `size="icon"` plus
// HEADER_ICON_CONTROL. A finger at any width gets the 40px floor
// (pointer-coarse), and a control painted on a phone adds its own
// (max-md:h-10); the rest are desktop-only and reached through the phone's fold.
const HEADER_CONTROL = "h-8 shrink-0 pointer-coarse:h-10"
const HEADER_ICON_CONTROL = "shrink-0 pointer-coarse:size-10"

// Below xl the header's labelled controls do not fit beside the path (at md
// they measured wider than the row), so each drops its words to a
// screen-reader-only label, keeping its accessible name, and the padding
// shrinks to make the lone icon a square: 32px, or 40px for a finger. The
// shared tooltip names each one, on hover and on keyboard focus.
const COLLAPSES_BELOW_XL = "max-xl:px-2 pointer-coarse:max-xl:px-3"

function HeaderLabel({ children }: { children: React.ReactNode }) {
  return <span className="max-xl:sr-only">{children}</span>
}

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
  // header drops its Close button (there is nothing to close into) and its
  // open-in-new-tab anchor (this already is that tab), and on a phone it leads
  // with the Back control every phone screen has and starts the explorer
  // collapsed.
  standalone?: boolean
}

export function EditorBody({ root, standalone = false }: EditorBodyProps) {
  const { bootstrap, changes, editorTarget, editorTabs, spine } = useDux()
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

  // Whether this editor is still mounted. Every asynchronous answer (a
  // save's, a read's) checks it before touching anything, the tab's unsaved
  // mark in the global store above all: an editor closed and opened again is
  // a new one, and the old one's answers belong to nobody.
  const aliveRef = useRef(true)
  useEffect(() => {
    aliveRef.current = true
    return () => {
      aliveRef.current = false
    }
  }, [])

  // Markdown preview toggle: render the buffer instead of the Monaco editor.
  // Kept per tab id rather than as one shared boolean, so each tab remembers
  // its own choice and no effect has to reset it on every tab switch.
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
  // Reopening a dirty buffer in another encoding discards the edits, so it
  // asks first, with the same live truth as the reload confirm: still dirty.
  const [reopenTarget, setReopenTarget] =
    useState<ReopenEncodingTarget | null>(null)
  // The fallback notice's dismissals live in a page-session set outside React
  // (lib/editorEncoding.ts); this only makes a dismissal re-render.
  const [, setNoticeDismissals] = useState(0)
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
    // Functional update: unions `dirs` into whatever batch is already pending
    // rather than overwriting it, so two mutations calling `revalidateDirs`
    // before React flushes a render between them both survive.
    setTreeRevalidate((prev) => unionRevalidateBatch(prev, dirs, nonce))
  }
  // The tree's loaded directories, as it last reported them. A ref, not state:
  // only the freshness triggers read it, and nothing on screen depends on it.
  const loadedTreeDirsRef = useRef<readonly string[]>([])
  // The flat file list backing the "Search files…" box (fetched from the
  // editor's session directly, independent of the changed-files watch). The
  // TREE does not consume this: it browses lazily via fileApi.tree.
  const [searchIndex, setSearchIndex] = useState<string[]>([])
  const [searchLoading, setSearchLoading] = useState(true)
  // True when the server capped the search index before sending all paths.
  const [searchTruncated, setSearchTruncated] = useState(false)
  const [search, setSearch] = useState("")
  // "Open editor" request in flight.
  const [openingEditor, setOpeningEditor] = useState(false)

  // The language picker's two pieces of state.
  //
  // The registry, read off Monaco through a dynamic import so this module never
  // pulls the Monaco bundle into the main chunk (see `loadRegisteredLanguages`).
  // Read in an effect rather than
  // from CodeEditor's `onReady`, because the picker must also work in diff
  // mode, where CodeEditor never mounts.
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

  // The live Monaco editor while the file pane is showing it, for the status
  // bar to follow. State rather than a ref so the bar re-subscribes when the
  // editor comes and goes; the bar's own cursor reads stay inside the bar.
  const [codeEditor, setCodeEditor] = useState<StatusBarEditor | null>(null)
  // The pixel size of the last image tab that finished loading, for the
  // status bar. Tagged with its path, so a picture still loading never shows
  // the previous one's.
  const [imageDims, setImageDims] = useState<ImageDimensions | null>(null)

  // Explorer panel layout, persisted by dux rather than by the panel library's
  // `useDefaultLayout`, which persists a percentage and so makes the explorer
  // two different widths in the two shells (see lib/editorLayout.ts). Read once
  // at mount, like the panel props below.
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

  // Covers the one gap the mount layout has: the library ignores a
  // defaultLayout whose keys do not cover every panel, so the explorer could
  // otherwise mount expanded on a phone. A no-op when the layout landed, since
  // collapse() bails at collapsedSize. Mount-only, so a later resize or
  // rotation cannot re-collapse an explorer the user has expanded.
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
  // Read-only only because the encoding cannot write the file back unchanged:
  // still text, and converting it to another encoding is the way out. Every
  // other read-only (a symlink, a .git path) has no save at all.
  const hardReadOnly = readyBufferFlag(fileReady, activeBuffer?.pathReadOnly)
  const encodingUnwritable =
    readOnly && !hardReadOnly && (activeBuffer?.unwritable ?? null) !== null
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
    void loadRegisteredLanguages()
      .then((languages) => {
        if (!cancelled) setRegisteredLanguages(languages)
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
    encodingChoicesRef.current = retargetLanguageOverrides(
      encodingChoicesRef.current,
      from,
      to,
    )
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

  // Mark the tree's changed files from the slice: the raw git status code per
  // path, a file inside a folded folder answering as that folder;
  // FileStatusIcon maps it to an icon + label.
  const changedMap = useMemo(
    () => changedStatusLookup(slice ? [slice.unstaged, slice.staged] : []),
    [slice],
  )

  // A per-file change-signal for the open file: status plus line counts, which
  // move when the content changes, though an edit keeping identical +/- counts
  // will not move it. Used only to flag a stale diff, never to key the cache,
  // so it drives no refetch. Deliberately not memoized: it re-evaluates every
  // render, and two small scans over the slice are cheap enough.
  const openFileSignal = changeSignalFor(slice, activeTabPath)
  const openFileSignalRef = useRef("")
  // The slice as of the last render, for the callbacks that resolve later and
  // need the signal for a path that may not be the active tab's.
  const sliceRef = useRef<ChangesSliceView | null>(slice)
  useEffect(() => {
    sliceRef.current = slice
  })

  // The tree's own freshness, on the same triggers the open buffers use: the
  // changed-files broadcast, the window regaining focus, the page becoming
  // visible, and switching editor tabs. Nothing polls.
  // ONE walk at a time: the triggers can arrive in a burst and a second walk
  // of the whole worktree would bring back nothing the first one is already
  // fetching. Dropped rather than queued, for the same reason.
  const searchWalkRef = useRef<Promise<void> | null>(null)
  function refreshSearchIndexOnce(): void {
    if (searchWalkRef.current) return
    const walk = refreshSearchIndex().finally(() => {
      if (searchWalkRef.current === walk) searchWalkRef.current = null
    })
    searchWalkRef.current = walk
  }
  const {
    refresh: treeRefresh,
    onRefreshSettled: treeRefreshSettled,
    refreshAllLoadedDirs,
  } = useFileTreeFreshness({
    slice,
    loadedDirsRef: loadedTreeDirsRef,
    activeTabKey: `${activeTab?.id ?? ""}:${activeTab?.path ?? ""}`,
    onRefreshRequested: refreshSearchIndexOnce,
  })

  const { raiseDiskBanner, dismissDiskBanner } =
    createEditorDiskBannerActions(setBuffers)

  // The save's way to read a file again when its answer no longer describes
  // the buffer; filled in once the reader below exists.
  const rereadRef = useRef<((tabId: string, path: string) => void) | null>(null)

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
    buffersRef,
    aliveRef,
    rereadRef,
    setBuffers,
    raiseDiskBanner,
  })
  // The operation out on the open file, if any (lib/editorLane.ts). While
  // one is, every control that would start another is unavailable: Save, the
  // encoding menu's rows, the banner's Reload and the conflict dialog's offers.
  const laneOp = useLaneBusy(
    activeTab === null ? null : laneKey(key, activeTab.path),
  )
  const fileBusy = laneOp !== undefined

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
  // What the status bar carries for what is on screen. The size is the disk's
  // as of the last read or save (the freshness stamp), not the unsaved
  // buffer's, so it never claims bytes that are not on disk yet.
  // The encoding of a large file's diff (the head of git's patch), while that
  // view is what is on screen and is not binary.
  const shownHead =
    activeTab?.mode === "diff" && diffReady ? activeBuffer?.diffHead : null
  const diffHeadEncoding =
    shownHead && !shownHead.binary
      ? encodingLabel(shownHead.encoding ?? "UTF-8", false)
      : null
  const statusPlan = planStatusBar({
    tab: activeTab === null ? null : { path: activeTab.path, mode: activeTab.mode },
    dirty,
    readOnly,
    image: isImageTab,
    imageDims,
    fileReady,
    binary,
    preview: showPreview,
    diffEditorShown: diffEditorIsShown(activeTab, diffReady, activeBuffer, showPreview),
    size: activeBuffer?.stamp.size ?? null,
    languageAvailable: showLanguagePicker,
    // Every text file names its encoding, plain UTF-8 included, the way VS
    // Code does, so any file can be reopened in any encoding from the bar.
    encodingLabel:
      activeBuffer !== undefined
        ? encodingLabel(
            activeBuffer.encoding,
            activeBuffer.bom,
          )
        : null,
    mixedEol: activeBuffer?.mixedEol ?? false,
    eol: activeBuffer?.eol ?? null,
    diffHeadEncodingLabel: diffHeadEncoding,
    readOnlyReason: encodingUnwritable
      ? `This file is read-only: ${encodingLabel(
          activeBuffer?.encoding ?? "UTF-8",
          activeBuffer?.bom ?? false,
        )} can't write it back without changing bytes you didn't touch. Reopen or convert it from the encoding menu.`
      : null,
  })

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
  const searchPick = useSearchPick({
    query: search,
    // While the index loads the list shows a spinner, not these rows.
    items: searchLoading ? [] : filtered,
    keyOf: (path) => path,
    onPick: (path) => requestOpen(path),
  })

  // Refetch the search index (a capped flat walk of the worktree). Called on
  // mount, after every create/rename/delete mutation, and on each of the
  // tree's own freshness triggers, so a file somebody else wrote is findable
  // in "Search files…" as soon as the tree shows it. The TREE never uses this:
  // it browses lazily per directory via fileApi.tree, revalidated separately
  // (see `revalidateDirs`).
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

  // The encoding the person chose for each open path, kept beside the buffers
  // for the one moment a buffer cannot carry it: a rename, which moves the tab
  // to a new path and so starts a fresh read. Same lifetime as a language
  // pick: it follows a rename or move and dies when the file is closed.
  const encodingChoicesRef = useRef<Map<string, string>>(new Map())
  useEffect(() => {
    for (const buffer of buffers.values()) {
      if (buffer.encodingRequested) {
        encodingChoicesRef.current.set(buffer.path, buffer.encodingRequested)
      }
    }
  }, [buffers])

  const { loadFileBuffer, reloadFileInPlace } = useEditorFileReads({
    aliveRef,
    root,
    tabs,
    activeTab,
    activeBuffer,
    tabsRef,
    buffersRef,
    sliceRef,
    encodingChoicesRef,
    setBuffers,
    raiseDiskBanner,
  })

  useEffect(() => {
    rereadRef.current = (tabId, path) =>
      reloadFileInPlace(tabId, path, false, { afterSave: true })
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

  const { loadDiffBuffer, refreshDiff, reloadDiffAs } = useEditorDiffReads({
    root,
    tabs,
    activeTab,
    activeBuffer,
    tabsRef,
    buffersRef,
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
    // The overrides are keyed by path, so they prune against the same open-path
    // set the model disposal above uses: an override lasts until the file is
    // closed, and one left behind is re-applied when that path is reopened, or
    // inherited by a different file that later takes it. Every way a tab leaves
    // ends in `tabs` losing the entry, so this covers all of them.
    setLanguageOverrides((prev) => pruneLanguageOverrides(prev, currentPaths))
    // The encoding choices die with their tab the same way.
    const choices = encodingChoicesRef.current
    for (const path of [...choices.keys()]) {
      if (!currentPaths.has(path)) choices.delete(path)
    }
  }, [tabs])

  // Dispose every retained model when the body unmounts: @monaco-editor/react
  // disposes only the current one, so every other path's model this tab strip
  // accumulated is dux's to reclaim. The library's own module-level view-state
  // cache is deliberately left alone: its per-entry cost is cursor and scroll
  // coordinates, and `saveViewState={false}` would lose view-state restore.
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
  // `path` is the file the change is the text of. It must be the file the
  // buffer holds, checked against the LIVE buffer: a preview tab reuses its id
  // for another file, and a change for one file must never land in another's
  // draft, whatever order the editor wrapper delivers it in.
  function handleDraftChange(value: string, path: string): void {
    if (!activeTab || activeTab.path !== path) return
    const tabId = activeTab.id
    setBuffers((prev) => {
      const cur = prev.get(tabId)
      if (!cur || cur.path !== path || cur.loadedPath !== path) return prev
      // Nobody can type into a read-only buffer (nor one an asked-for reopen
      // or reload is reading), so a change here is the editor's own echo and
      // never an edit; it must not move the text a conversion would write.
      if (cur.readOnly || cur.readingLock) return prev
      const next = new Map(prev)
      next.set(tabId, { ...cur, draft: value })
      return next
    })
    const cur = buffers.get(tabId)
    if (cur && cur.path === path && !cur.binary && !cur.readOnly) {
      const newDirty = isBufferDirty({ ...cur, draft: value })
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
    // A busy file is answered by `writeBuffer`, which says so.
    if (!activeTab || binary || !dirty) return
    writeBuffer(
      activeTab.id,
      activeTab.path,
      activeBuffer?.draft ?? "",
      bufferSaveEncoding(activeBuffer),
      activeBuffer?.stamp,
    )
  }

  // "Save with encoding": write the buffer now in `choice`, with the same
  // freshness token as any save. Offered whether or not there are edits, since
  // converting the file is itself the change; a character the encoding cannot
  // hold refuses the whole save and writes nothing.
  function saveWithEncoding(choice: SaveEncoding): void {
    if (!activeTab || binary || hardReadOnly || fileBusy || !fileReady) return
    if (!activeBuffer) return
    // What the file is now; that row is inert, so a save from here is always
    // a conversion.
    if (sameSaveEncoding(choice, bufferReadEncoding(activeBuffer))) return
    // The text goes exactly as the editor holds it: the row's mark choice
    // alone decides whether a mark is written, and nothing in the text is
    // taken out or put in for it.
    writeBuffer(
      activeTab.id,
      activeTab.path,
      activeBuffer.draft,
      choice,
      activeBuffer.stamp,
      true,
    )
  }

  // "Reopen with encoding": read the file again in exactly `encoding`. With
  // unsaved edits that throws them away, so it confirms first; a clean buffer
  // reopens at once. A file that does not decode in it is refused by the
  // server and the buffer stays as it was (see `reloadFileInPlace`).
  function requestReopen(encoding: string): void {
    // A save in flight would answer for whatever text this replaced it with.
    if (!activeTab || fileBusy) return
    const tabId = activeTab.id
    const path = activeTab.path
    const isDirty = tabsRef.current.find((t) => t.id === tabId)?.dirty ?? false
    if (isDirty) {
      setReopenTarget({
        tabId,
        path,
        encoding,
        label: encodingLabel(encoding, false),
      })
      return
    }
    reloadFileInPlace(tabId, path, true, { encoding })
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
      .then((editor) => notifySuccess(prose`Opening in ${chip(editor)}…`))
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
        // No toast: the entry appears in the tree, and a new FILE also opens in
        // a pinned tab. The failure arm below still speaks, because the dialog
        // stays open with nothing else to say why.
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
        // No toast: the tree row and the open tab both carry the new name. A
        // MOVE keeps its toast (see `handleMoveSubmit`), because the entry
        // leaves the folder the user was looking at.
        editorRenameTabPaths(root, from, to)
        retargetOverrides(from, to)
        revalidateDirs([parentDir(from), parentDir(to)])
        return refreshSearchIndex()
      })
      .catch((e) => {
        notifyError(e instanceof Error ? e.message : "could not rename")
      })
  }

  // A move is a rename on disk, so it takes the same route with a destination
  // in another folder and reuses the rename's tab retargeting and
  // two-directory revalidation. Overwriting is refused rather than confirmed:
  // the server rejects an occupied destination and the toast says so. The
  // ordering lives in `lib/moveEntry`; this is only the wiring.
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

  // Fire-and-forget once confirmed: the destructive dialog already gated the
  // action, so this closes immediately rather than waiting on the request and a
  // failure only toasts.
  //
  // `savingPaths` is re-checked even though `DeleteEntryDialog`'s button is
  // already disabled while a save for this path is in flight, because the
  // dialog computes that from the render at which it opened.
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
        // A failed delete can still have removed part of a directory, so the
        // tree cache is stale either way. Revalidate the same directories the
        // success path would, so `FileTree` re-reads what is really on disk.
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
      // Local, not wire: the upload is a fetch this tab awaits, so a
      // stranded spinner means a slow request rather than a quiet server.
      reportBusy: (message) => notifyBusy(message, { id: toastId, origin: "local" }),
      // `sticky` is forwarded rather than dropped, so the decision stays in
      // `editorDropToast`, the one place that makes it. Hardcoding it here
      // would be a second opinion; omitting it makes a tree drop unpinnable
      // whatever the report asked for.
      reportFinal: (t) => notify(t.tone, t.prose, { id: toastId, sticky: t.sticky }),
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

  // The encoding menu's rows, behind the status bar's encoding selector on
  // every screen. Two submenus, not dialogs, so no trailing ellipsis. Any file
  // can be reopened in any encoding: a garbled result is the person's call and
  // a reopen away from undone, while a decode that would lose bytes is refused
  // by the server. Save with is gone for a read-only file, which has no save.
  function encodingMenuItems(): React.ReactNode {
    // The large-file diff view rereads the patch and has no save; the file
    // view reopens the file and can save in another encoding.
    const inDiff = activeTab?.mode === "diff"
    const current = inDiff
      ? { encoding: activeBuffer?.diffHead?.encoding ?? "UTF-8", bom: false }
      : activeBuffer
        ? bufferReadEncoding(activeBuffer)
        : bufferSaveEncoding(undefined)
    const reopen = (encoding: string) => {
      if (inDiff && activeTab) reloadDiffAs(activeTab.id, activeTab.path, encoding)
      else requestReopen(encoding)
    }
    return (
      <>
        <DropdownMenuSub>
          {/* Not while this tab is saving: the save's answer is about the text
              it sent, and a reopen would put a different decoding under it. */}
          <DropdownMenuSubTrigger disabled={fileBusy}>
            <RotateCw />
            Reopen with encoding
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="max-h-80 overflow-y-auto">
            {REOPEN_CHOICES.map((choice) => (
              <DropdownMenuItem
                key={choice.encoding}
                aria-current={
                  choice.encoding === current.encoding ? "true" : undefined
                }
                onClick={() => reopen(choice.encoding)}
              >
                <FileType />
                {choice.label}
                {choice.encoding === current.encoding && (
                  <Check className="ml-auto" />
                )}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        {!inDiff && !hardReadOnly && (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger disabled={fileBusy}>
              <SaveAll />
              Save with encoding
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="max-h-80 overflow-y-auto">
              {SAVE_CHOICES.map((choice) => {
                const isCurrent = sameSaveEncoding(choice, current)
                return (
                  <DropdownMenuItem
                    key={`${choice.encoding}:${choice.bom}`}
                    aria-current={isCurrent ? "true" : undefined}
                    // A file its encoding cannot write back unchanged can be
                    // converted, never saved again in that same encoding.
                    // The current encoding and mark is never a row to act
                    // on: it is not a conversion, and not an edit either, so
                    // a save from it would only rewrite untouched bytes (the
                    // line endings, say) for nothing.
                    disabled={isCurrent}
                    onClick={() =>
                      saveWithEncoding({
                        encoding: choice.encoding,
                        bom: choice.bom,
                      })
                    }
                  >
                    <FileType />
                    {choice.label}
                    {isCurrent && <Check className="ml-auto" />}
                  </DropdownMenuItem>
                )
              })}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        )}
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

  function renderDesktopViewActions(): React.ReactNode {
    const previewLabel =
      showPreview && activeTab?.mode === "file" ? "Edit" : "Preview"
    return (
      <>
        {activeTab && !isImageTab && hasDiff && (
          // One switch, two outline buttons joined by the shared ButtonGroup;
          // the Button toggle look's primary fill marks the current mode.
          <ButtonGroup className="shrink-0 max-md:hidden" aria-label="View mode">
            <SimpleTooltip content="File">
              <Button
                variant="outline"
                className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL)}
                toggle
                aria-pressed={activeTab.mode === "file"}
                onClick={() => editorSetTabMode(root, activeTab.id, "file")}
              >
                <FileText />
                <HeaderLabel>File</HeaderLabel>
              </Button>
            </SimpleTooltip>
            <SimpleTooltip content="Diff">
              <Button
                variant="outline"
                className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL)}
                toggle
                aria-pressed={activeTab.mode === "diff"}
                onClick={() => editorSetTabMode(root, activeTab.id, "diff")}
              >
                <GitCompare />
                <HeaderLabel>Diff</HeaderLabel>
              </Button>
            </SimpleTooltip>
          </ButtonGroup>
        )}
        {hasDiff && activeTab?.mode === "diff" && diffStale && (
          <SimpleTooltip content="This file changed on disk: reload the diff">
            <Button
              variant="outline"
              className={cn(
                HEADER_CONTROL,
                COLLAPSES_BELOW_XL,
                "text-dux-warning max-md:hidden",
              )}
              aria-label="Reload the diff. The file changed on disk"
              onClick={refreshDiff}
            >
              <CircleAlert />
              <HeaderLabel>Reload</HeaderLabel>
            </Button>
          </SimpleTooltip>
        )}
        {canPreview && (
          <SimpleTooltip content={previewLabel}>
            <Button
              variant="outline"
              className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL, "max-md:hidden")}
              toggle
              aria-pressed={showPreview}
              onClick={togglePreview}
            >
              {showPreview && activeTab?.mode === "file" ? <Pencil /> : <Eye />}
              <HeaderLabel>{previewLabel}</HeaderLabel>
            </Button>
          </SimpleTooltip>
        )}
      </>
    )
  }

  function renderLocalEditorAction(): React.ReactNode {
    if (activeTab === null) return null
    return (
      // On the wrapper rather than the button, so it still answers a hover
      // while the button is disabled; a keyboard focus on the button inside
      // opens it too, because focus reaches the wrapper's handler.
      <SimpleTooltip
        content={
          localAccess
            ? "Open local editor"
            : "Only available when dux is opened locally, not over a remote URL."
        }
      >
        <span className="inline-flex max-md:hidden">
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="outline"
                  // The wrapper span, not the button, carries max-md:hidden.
                  className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL)}
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
              <HeaderLabel>Open local editor</HeaderLabel>
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
    // The cluster's one filled primary action, beside its outline peers: Save
    // commits the user's work, so it is the header's primary act. A
    // user-approved exception, like the launcher's filled verb; it still keeps
    // the header's one height.
    return (
      <Button
        variant="default"
        className={cn(HEADER_CONTROL, "max-md:h-10")}
        disabled={!dirty || fileBusy || readOnly}
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
              size="icon"
              variant="outline"
              className={cn(HEADER_ICON_CONTROL, "md:hidden max-md:size-10")}
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
    // Both of these leave the overlay rather than act on the file, and the
    // tenet would let them be quieter; they wear the cluster's outline anyway,
    // because a ghost label in a row of outlined buttons reads as text rather
    // than as a control, and these are the overlay's only ways out.
    return (
      <>
        <SimpleTooltip content="Open in new tab">
          <Button
            variant="outline"
            className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL, "max-md:h-10")}
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
            <HeaderLabel>Open in new tab</HeaderLabel>
          </Button>
        </SimpleTooltip>
        <SimpleTooltip content="Close">
          <Button
            variant="outline"
            className={cn(HEADER_CONTROL, COLLAPSES_BELOW_XL, "max-md:h-10")}
            onClick={() => closeEditor()}
          >
            <X />
            <HeaderLabel>Close</HeaderLabel>
          </Button>
        </SimpleTooltip>
      </>
    )
  }

  // What the editor is rooted at, named from the same facts the sidebar row is
  // drawn from, so the header and the row cannot call one thing two names. One
  // line on a computer; two on a phone, where the words sit above the chip.
  function renderRootTitle(): React.ReactNode {
    const named = standaloneEditorName(root, spine)
    const Glyph = named.glyph === "terminal" ? SquareTerminal : FileCode2
    // "Editor for <name>[ in <where>]" as prose segments. The leading words get
    // their own line on a phone; the names follow on one line that truncates
    // rather than wrapping the header, the first name holding on longest.
    const [lead, ...rest] = editorTitleProse(named)
    return (
      <>
        {/* The root's own glyph, so a terminal-rooted editor reads as one at a
            glance rather than as an agent whose name happens to be a path. */}
        <Glyph className="size-4 shrink-0 text-muted-foreground max-md:hidden" />
        <div className="flex min-w-0 flex-1 items-center gap-1.5 max-md:flex-col max-md:items-start max-md:gap-0.5">
          <span className="shrink-0 text-sm text-muted-foreground max-md:text-xs">
            {typeof lead === "string" ? lead.trim() : null}
          </span>
          <div className="flex min-w-0 max-w-full items-center gap-1.5 text-sm">
            {rest.map((segment, index) =>
              typeof segment === "string" ? (
                <span key={index} className="shrink-0 text-muted-foreground">
                  {segment.trim()}
                </span>
              ) : (
                <InlineCode
                  key={index}
                  className={cn(
                    "min-w-0 truncate whitespace-nowrap",
                    // The owner gives way faster than the terminal's own name.
                    rest.slice(0, index).some((s) => typeof s !== "string") &&
                      "shrink-[4]",
                  )}
                >
                  {segment.name}
                </InlineCode>
              ),
            )}
          </div>
        </div>
      </>
    )
  }

  function renderHeader(): React.ReactNode {
    return (
      <>
        {/* Header: what this editor is rooted at, the view toggle and the
          actions. The open file's path lives in the status bar below.
          Every control is HEADER_CONTROL's h-8 (32px), so min-h-12.25 (49px)
          is that plus the row's py-2 (16px) and its own border-b (1px; min-h
          is border-box): the bar keeps one height as controls come and go.
          For a finger every control is 40px, so the floor is 57px
          (pointer-coarse:min-h-14.25). On a phone the painted controls take
          the 40px floor, and the explorer toggle always renders there, so
          the row is constant too. */}
      <div className="flex min-h-12.25 items-center gap-2 pointer-coarse:min-h-14.25 border-b px-3 py-2">
        {standalone && (
          // The phone's Up control, the same generic Back every phone screen
          // leads with, and as quiet as theirs: it navigates rather than acts,
          // so it stays ghost beside the outline cluster. It PUSHES the screen
          // the editor is about rather than stepping back through history,
          // which has nothing behind it when the editor opened in a new tab.
          // A computer has no such control: there the browser's own Back and
          // closing the tab are the way out.
          <Button
            variant="ghost"
            size="icon"
            className="size-10 shrink-0 md:hidden"
            aria-label="Back"
            onClick={() => leaveStandaloneEditor(root)}
          >
            <ChevronLeft />
          </Button>
        )}
        {/* Explorer collapse/expand toggle: lives in the header, OUTSIDE the
            panel it hides, so it stays reachable while collapsed. */}
        <SimpleTooltip
          content={
            explorerCollapsed ? "Show the file explorer" : "Hide the file explorer"
          }
        >
          <Button
            size="icon"
            variant="outline"
            className={cn(HEADER_ICON_CONTROL, "max-md:size-10")}
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
        {renderRootTitle()}
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
            // Keep the explorer's pixel width when the group resizes rather
            // than rescaling it proportionally, or the width means different
            // things in the two shells. The content pane keeps the library's
            // relative behavior, which is also the one relative panel the group
            // requires.
            groupResizeBehavior="preserve-pixel-size"
            style={{ overflow: "hidden" }}
            collapsible
          >
            {/* min-w-0 so path truncation keeps working at narrow widths.
                Deliberately no border-r: the ResizableHandle beside this panel
                already draws the pane separator, and the app-wide idiom is
                panes bleeding to the shell edge with the handle as the only
                divider. A border here doubles the editor content's left edge
                while its bottom and right have none. */}
            <div className="flex h-full min-w-0 flex-col">
              {/* gap-2 (8px) is the repo's measured misclick spacing between
                  adjacent icon squares, and this bar has two of them: New file
                  and the explorer `⋯`, both 28px on both axes, with the search
                  field as the third interactive neighbour on the row. */}
              <div className="flex items-center gap-2 border-b p-2">
                <div className="relative flex-1">
                  <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search files…"
                    {...searchPick.fieldProps}
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
                {/* The explorer's own menu, on every surface: the tree keeps
                    itself fresh on events, and this is the way to ask it
                    anyway. Under the 40px floor on both axes to match New file
                    beside it, which sets this bar's density; the exemption is
                    paid for by the gap above rather than by the size. */}
                <DropdownMenu>
                  <DropdownMenuTrigger
                    render={
                      <Button
                        size="icon-sm"
                        variant="ghost"
                        aria-label="Explorer actions"
                      />
                    }
                  >
                    <Ellipsis />
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={refreshAllLoadedDirs}>
                      <RefreshCw />
                      Refresh files
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
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
                            id={searchPick.rowId(p)}
                            type="button"
                            onClick={() => requestOpen(p)}
                            className={cn(
                              "flex w-full items-center gap-1.5 rounded px-1 py-1 hover:bg-muted",
                              (p === openPath || p === searchPick.activeKey) &&
                                "bg-muted",
                            )}
                          >
                            {changedMap.get(p) && (
                              <FileStatusIcon status={changedMap.get(p)!} />
                            )}
                            {/* The full path, giving way at its start so the
                                filename stays visible. */}
                            <StartTruncatedText
                              text={p}
                              className="flex-1 font-mono text-sm"
                            />
                          </button>
                        ))}
                        {searchTruncated && (
                          <p className="px-1 py-2 text-xs text-muted-foreground">
                            The search index was capped, so results may be
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
                  refresh={treeRefresh}
                  onRefreshSettled={treeRefreshSettled}
                  onLoadedDirsChange={(dirs) => {
                    loadedTreeDirsRef.current = dirs
                  }}
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
                    busy={fileBusy}
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
              {/* The encoding notices: how this file was read decides what
                  a save would do, so it is said before anything is written.
                  Beside, never instead of, the disk banner above: separate
                  facts. */}
              {activeTab !== null &&
                activeTab.mode === "file" &&
                fileReady &&
                !binary &&
                activeBuffer !== undefined &&
                encodingNotices(
                  activeTab.path,
                  activeBuffer,
                  (kind) =>
                  noticeDismissed(key, activeTab.path, kind),
                ).map((notice) => (
                  <EncodingNotice
                    key={notice.kind}
                    notice={notice}
                    onDismiss={() => {
                      dismissNotice(key, activeTab.path, notice.kind)
                      setNoticeDismissals((n) => n + 1)
                    }}
                  />
                ))}
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
                  onEditorChange={setCodeEditor}
                  onImageDims={setImageDims}
                />
              </div>
            </div>
          </ResizablePanel>
        </ResizablePanelGroup>
        </div>
        {/* Full width under the explorer and the content: it describes the
            open file, whichever pane has focus. */}
        <EditorStatusBar
          plan={statusPlan}
          editor={codeEditor}
          languageLabel={activeLanguageLabel}
          languageItems={languagePickerItems()}
          encodingItems={encodingMenuItems()}
        />
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
        busy={fileBusy}
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
          writeBuffer(target.tabId, target.path, target.body, target.encoding)
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
      <ConfirmReopenEncodingDialog
        target={reopenTarget}
        present={
          reopenTarget !== null &&
          (tabs.find((t) => t.id === reopenTarget.tabId)?.dirty ?? false)
        }
        onClose={() => setReopenTarget(null)}
        onConfirm={() => {
          const target = reopenTarget!
          setReopenTarget(null)
          reloadFileInPlace(target.tabId, target.path, true, {
            encoding: target.encoding,
          })
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
  // A head answer has no working-copy side to preview from, so a markdown file
  // past the ceiling gets the patch rather than a preview of nothing.
  return ready && !(buffer?.diff?.binary ?? false) && !buffer?.diffHead
}

// Monaco's diff editor is what the content pane shows: not the patch head past
// the size ceiling, not the binary refusal, and not a preview.
function diffEditorIsShown(
  tab: EditorTab | null,
  diffReady: boolean,
  buffer: TabBuffer | undefined,
  preview: boolean,
): boolean {
  return (
    tab?.mode === "diff" &&
    !isImagePreviewPath(tab.path) &&
    diffReady &&
    !buffer?.diffHead &&
    !(buffer?.diff?.binary ?? false) &&
    !preview
  )
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
  onDraftChange: (value: string, path: string) => void
  onSave: () => void
  onMonacoReady: (monaco: MonacoInstance) => void
  onEditorChange: (editor: StatusBarEditor | null) => void
  onImageDims: (dims: ImageDimensions | null) => void
}

function EditorContentPane(props: EditorContentPaneProps) {
  const { root, tab, image, onImageDims } = props
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
        onDims={onImageDims}
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
  // Past the size ceiling the server answers with the head of git's own patch
  // instead of two sides, so there is nothing here for the diff editor to do.
  const diffHead = buffer?.diffHead
  if (diffHead) {
    if (diffHead.binary) {
      return (
        <CenteredPane>
          This file is binary, so it can&rsquo;t be diffed here.
        </CenteredPane>
      )
    }
    return (
      <ChunkBoundary>
        <Suspense fallback={<LoadingPane />}>
          <DiffHeadViewer
            text={diffHead.text}
            banner={diffHeadBanner(diffHead)}
          />
        </Suspense>
      </ChunkBoundary>
    )
  }
  if (buffer?.diff?.binary) {
    return (
      <CenteredPane>
        This file is binary, so it can&rsquo;t be diffed here.
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
  onEditorChange,
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
        This file is binary, so it can&rsquo;t be edited here.
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
          onEditorChange={onEditorChange}
          // A read that names the file's style hands the text over in LF
          // (see `editorTextOf`); an older server names none.
          lfText={(buffer?.eol ?? null) !== null}
          readOnly={(buffer?.readOnly ?? false) || (buffer?.readingLock ?? null) !== null}
          readOnlyReason={readOnlyReasonOf(buffer)}
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

// Read-only preview for a raster image tab, rendered straight from /raw with no
// /read and no buffer. A refused request or undecodable bytes error the <img>
// and swap in the error text; Retry bumps `attempt`, which keys the <img>, so a
// fresh element re-fires at the same URL (/raw already sends
// Cache-Control: no-cache). The caller keys the pane by path, so this state
// resets on a preview-replace.
//
// The pixel dimensions go to the status bar, which already names the path. It
// shows no byte size, because the pane never fetches the file through /read.
function ImagePreviewPane({
  src,
  path,
  onDims,
}: {
  src: string
  path: string
  onDims: (dims: ImageDimensions | null) => void
}) {
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
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
            onDims(null)
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
    <PreviewImageFrame>
      <img
        key={attempt}
        src={src}
        alt={path}
        className="max-h-full max-w-full object-contain"
        onLoad={(e) =>
          onDims({
            path,
            width: e.currentTarget.naturalWidth,
            height: e.currentTarget.naturalHeight,
          })
        }
        onError={() => {
          // A picture that failed to load has no size to report, whatever the
          // last successful load of this path measured.
          onDims(null)
          setFailed(true)
        }}
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
    <PreviewImageFrame>
      {url !== null && (
        <img src={url} alt={path} className="max-h-full max-w-full object-contain" />
      )}
    </PreviewImageFrame>
  )
}

// Shared frame for the two image panes: the image is contained in the pane, so
// a portrait image can never force a surprise scrollbar. The path they used to
// caption themselves with is the status bar's now.
function PreviewImageFrame({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-full min-h-0 items-center justify-center bg-muted/30 p-4">
      {children}
    </div>
  )
}

// What Monaco says when someone types into a read-only buffer: the same two
// reasons the status bar's read-only label gives.
function readOnlyReasonOf(buffer: TabBuffer | undefined): string | undefined {
  const lock = buffer?.readingLock ?? null
  if (lock?.kind === "reopen") {
    return `Reopening as ${lock.label}: the text is replaced when the file has been read again.`
  }
  if (lock?.kind === "reload") {
    return "Reloading from disk: the text is replaced when the file has been read again."
  }
  if (!buffer?.readOnly) return undefined
  return buffer.unwritable !== null && !buffer.pathReadOnly
    ? "This file is read-only: its encoding can't write it back without changing bytes you didn't touch. Reopen or convert it from the encoding menu in the status bar."
    : "This file is read-only: it is a symlink to an external file or a .git path."
}
