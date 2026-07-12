import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react"
import {
  ChevronDown,
  CircleAlert,
  ExternalLink,
  Eye,
  FileCode2,
  FilePlus,
  FileText,
  GitCompare,
  Loader2,
  Pencil,
  Save,
  Search,
  X,
} from "lucide-react"
import { toast } from "sonner"
import { fileApi } from "@/lib/fileApi"
import type { FileDiffContents } from "@/lib/fileApi"
import { OPEN_IN_EDITORS } from "@/lib/editors"
import { isBufferStale, pruneByIds, pruneSetByIds } from "@/lib/editorBuffers"
import { dirtyCloseMessage, shouldPromoteOnEdit } from "@/lib/editorTabs"
import { isLocalAccessHost } from "@/lib/localAccess"
import { isMarkdownPath } from "@/lib/markdown"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import type { MonacoInstance } from "@/components/CodeEditor"
import { EditorIcon } from "@/components/EditorIcon"
import { EditorTabsStrip } from "@/components/EditorTabsStrip"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { Button } from "@/components/ui/button"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { FileTree } from "@/components/FileTree"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  closeEditor,
  editorOpenFile,
  editorPinTab,
  editorSetTabDirty,
  editorSetTabMode,
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

// The overlay shell: owns the Dialog and is desktop-only (Monaco is poor on
// touch, and every entry point is already gated to desktop). The body is keyed
// by SESSION ONLY (not file/mode) so opening a new file (or a preview-replace)
// while the overlay is already open never remounts it and drops the tab list.
// A ref lets the body intercept Esc/backdrop closes so they run the same dirty
// guard as the in-body Close button.
export function EditorOverlay() {
  const { editorTarget } = useDux()
  const isMobile = useIsMobile()
  // Default close handler (used before a body mounts / after it unmounts).
  const closeReqRef = useRef<() => void>(closeEditor)

  if (isMobile) return null

  return (
    <Dialog
      open={editorTarget !== null}
      onOpenChange={(open) => {
        if (!open) closeReqRef.current()
      }}
    >
      <DialogContent
        showCloseButton={false}
        className="flex h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(80rem,calc(100%-2rem))]"
      >
        <DialogTitle className="sr-only">Code editor</DialogTitle>
        <DialogDescription className="sr-only">
          Browse, edit, and diff files in this worktree.
        </DialogDescription>
        {editorTarget && (
          <EditorBody
            key={editorTarget.sessionId}
            sessionId={editorTarget.sessionId}
            closeReqRef={closeReqRef}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

// One tab's Monaco buffer + diff cache, keyed by TAB ID in `EditorBody`'s
// `buffers` map. `path` is the path this entry was fetched FOR: every read
// must check it against the tab's CURRENT path via `isBufferStale` before
// trusting `loaded`/`draft`/diff fields, because `openFile` rule 2 (preview-
// replace) reuses a tab's id while swapping its path out from under it. A
// stale entry is treated as unloaded and re-fetched, never rendered.
interface TabBuffer {
  path: string
  // The path whose content is actually held in `loaded`/`draft`, or null
  // while a fetch for `path` is in flight / has never completed.
  loadedPath: string | null
  // True from the moment `loadFileBuffer` seeds this entry until its fetch
  // settles (success OR error). `loadFileBuffer` seeds a fresh buffer
  // synchronously before its async read resolves (so a stale buffer never
  // renders even for one frame), and that synchronous `setState` changes
  // `activeBuffer?.loadedPath` (undefined -> null), which re-triggers the
  // load effect on the very next render. Without this flag the effect's
  // "already loaded?" check (`loadedPath === path`) sees `null` both before
  // AND during the in-flight fetch and can't tell them apart, so it fires a
  // SECOND `fileApi.read` for the same tab+path. This is harmless for
  // correctness (the request-token check discards the first response), but
  // it doubles the read cost on every fresh open. `loading` is the marker
  // that lets the effect distinguish "already fetching, don't refetch" from
  // "settled with an error, retry on next visit."
  loading: boolean
  loaded: string
  draft: string
  binary: boolean
  readOnly: boolean
  diff: FileDiffContents | null
  diffLoadedPath: string | null
  diffLoadedSignal: string
  fileError: string | null
  diffError: string | null
}

function emptyBuffer(path: string): TabBuffer {
  return {
    path,
    loadedPath: null,
    loading: true,
    loaded: "",
    draft: "",
    binary: false,
    readOnly: false,
    diff: null,
    diffLoadedPath: null,
    diffLoadedSignal: "",
    fileError: null,
    diffError: null,
  }
}

interface EditorBodyProps {
  sessionId: string
  closeReqRef: React.RefObject<() => void>
}

function EditorBody({ sessionId, closeReqRef }: EditorBodyProps) {
  const { changes, editorTarget, editorTabs } = useDux()
  const tabsState = editorTabs[sessionId]
  const tabs = useMemo(() => tabsState?.tabs ?? [], [tabsState])
  const activeTab = tabs.find((t) => t.id === tabsState?.activeId) ?? null

  // Per-tab Monaco buffers (file content + diff cache), keyed by tab id, kept
  // OUT of the global store deliberately (see lib/editorTabs.ts header
  // comment): putting file contents in zustand-style global state would fire
  // a store-wide update on every keystroke.
  const [buffers, setBuffers] = useState<Map<string, TabBuffer>>(
    () => new Map(),
  )
  const activeBuffer = activeTab ? buffers.get(activeTab.id) : undefined

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

  const [savingTabId, setSavingTabId] = useState<string | null>(null)
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

  // "Discard unsaved changes?" for closing the WHOLE overlay (Esc/backdrop/
  // Close button) when ANY tab is dirty. Per-tab close uses the store-target
  // `ConfirmCloseEditorTabDialog` instead; this is the overlay-level guard so
  // Esc/backdrop/Close can never silently drop edits in some other tab.
  const [overlayCloseConfirmOpen, setOverlayCloseConfirmOpen] = useState(false)
  const [newFileOpen, setNewFileOpen] = useState(false)
  const [newFilePath, setNewFilePath] = useState("")
  const [creating, setCreating] = useState(false)
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

  const dirty = activeTab?.dirty ?? false
  const isMarkdown = activeTab !== null && isMarkdownPath(activeTab.path)
  const fileReady =
    activeTab !== null &&
    activeBuffer !== undefined &&
    !isBufferStale(activeBuffer, activeTab.path) &&
    activeBuffer.loadedPath === activeTab.path
  const binary = fileReady ? (activeBuffer?.binary ?? false) : false
  const readOnly = fileReady ? (activeBuffer?.readOnly ?? false) : false
  // Markdown preview is available only for a loaded, non-binary markdown file in
  // file mode — one source of truth for both the toggle button and the render.
  const canPreview =
    activeTab?.mode === "file" && isMarkdown && fileReady && !binary
  const showPreview =
    activeTab !== null && previewOpenTabIds.has(activeTab.id) && canPreview
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
  // This tab's save is in flight.
  const isSaving = savingTabId !== null && savingTabId === activeTab?.id

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase()
    if (!needle) return []
    return searchIndex
      .filter((f) => f.toLowerCase().includes(needle))
      .slice(0, MAX_SEARCH_RESULTS)
  }, [search, searchIndex])

  // Fetch the search index (a capped flat walk) on open. The TREE loads itself
  // lazily per directory. Mount-only: the body is keyed by session, so a new
  // session remounts.
  useEffect(() => {
    let cancelled = false
    fileApi
      .list(sessionId)
      .then((result) => {
        if (!cancelled) {
          setSearchIndex(result.files)
          setSearchTruncated(result.truncated ?? false)
        }
      })
      .catch(() => {
        if (!cancelled) toast.error("could not index worktree files for search")
      })
      .finally(() => {
        if (!cancelled) setSearchLoading(false)
      })
    return () => {
      cancelled = true
    }
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
    // Seed/replace this tab's buffer for the new path. This is the re-key
    // step the preview-replace fix depends on: without it, a stale buffer
    // entry for the tab's OLD path could still render while the new fetch
    // is pending.
    setBuffers((prev) => {
      const next = new Map(prev)
      next.set(tabId, emptyBuffer(path))
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
            fileError: e instanceof Error ? e.message : "could not open file",
          })
          return next
        })
      })
  }

  // Load the active tab's file buffer lazily: only in file mode, only when the
  // cached buffer doesn't already hold CURRENT content for this tab (absent,
  // stale per `isBufferStale`, e.g. a preview-replace swapped this tab's
  // path) AND isn't already mid-fetch for this exact path (`loading`).
  // Skipped entirely in diff mode. Unlike the diff, the buffer is NOT
  // auto-refreshed when the file changes on disk under us: re-reading could
  // silently clobber unsaved edits.
  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    if (
      activeBuffer &&
      !isBufferStale(activeBuffer, activeTab.path) &&
      (activeBuffer.loadedPath === activeTab.path || activeBuffer.loading)
    )
      return
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
    // eslint-disable-next-line react-hooks/set-state-in-effect
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
  ])

  // Fetch and store a tab's diff cache for `path`. Extracted for the same
  // reason as `loadFileBuffer` above; also a plain function for the same
  // compiler-derived-lint-rule reason.
  function loadDiffBuffer(tabId: string, path: string): void {
    const token = (diffRequestTokenRef.current.get(tabId) ?? 0) + 1
    diffRequestTokenRef.current.set(tabId, token)
    fileApi
      .diff(sessionId, path)
      .then((d) => {
        if (diffRequestTokenRef.current.get(tabId) !== token) return
        setBuffers((prev) => {
          const cur = prev.get(tabId) ?? emptyBuffer(path)
          if (cur.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...cur,
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
          const cur = prev.get(tabId) ?? emptyBuffer(path)
          if (cur.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...cur,
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

  function requestClose(): void {
    if (tabs.some((t) => t.dirty)) {
      setOverlayCloseConfirmOpen(true)
      return
    }
    closeEditor()
  }

  // Let the shell's Esc/backdrop close run this same dirty guard. Updated every
  // render (so it sees the latest `tabs`), reset on unmount so a stale closure
  // can't fire against an unmounted body.
  useEffect(() => {
    closeReqRef.current = requestClose
    // Kept current here (an effect, not render) so the diff fetch's late callback
    // stamps the diff with the signal as of load-resolve time.
    openFileSignalRef.current = openFileSignal
    return () => {
      closeReqRef.current = closeEditor
    }
  })

  function confirmOverlayClose(): void {
    setOverlayCloseConfirmOpen(false)
    closeEditor()
  }

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
    fileApi
      .write(sessionId, path, body)
      .then(() => {
        // Stale-save guard: if this tab's buffer no longer belongs to `path` (a
        // preview-replace reused the tab id for a different file while the save
        // was in flight), don't resurrect a buffer for a file no longer open in
        // this tab.
        setBuffers((prev) => {
          const cur = prev.get(tabId)
          if (!cur || isBufferStale(cur, path)) return prev
          const next = new Map(prev)
          next.set(tabId, { ...cur, loaded: body, diffLoadedPath: null })
          return next
        })
        editorSetTabDirty(sessionId, tabId, false)
        toast.success(`Saved ${path}`)
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not save file")
      })
      .finally(() =>
        setSavingTabId((id) => (id === tabId ? null : id)),
      )
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
      .then((editor) => toast.success(`Opening in ${editor}…`))
      .catch((e) =>
        toast.error(e instanceof Error ? e.message : "could not open in editor"),
      )
      .finally(() => setOpeningEditor(false))
  }

  function createFile(): void {
    const path = newFilePath.trim()
    if (!path || creating) return
    setCreating(true)
    fileApi
      .write(sessionId, path, "")
      .then(() => fileApi.list(sessionId))
      .then((result) => {
        // Refresh the search index so the new file is findable; the tree pulls
        // the file's parent directory itself when the tab opens on it.
        setSearchIndex(result.files)
        setSearchTruncated(result.truncated ?? false)
        setNewFileOpen(false)
        setNewFilePath("")
        // A brand-new file is a deliberate open, so pin it and force file
        // mode (it can't have a diff since it was just created).
        editorOpenFile(sessionId, path, { mode: "file", pin: true })
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not create file")
      })
      .finally(() => setCreating(false))
  }

  const openPath = activeTab?.path ?? null
  const dirtyTabCount = tabs.filter((t) => t.dirty).length

  return (
    <>
      {/* Header: open file path, view toggle, dirty indicator, actions. */}
      <div className="flex items-center gap-2 border-b px-3 py-2">
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
        {/* File / Diff view toggle — a segmented control. Hidden until a file is
            open (nothing to view otherwise). Sets the ACTIVE TAB's mode. */}
        {activeTab && (
          <div
            className="flex shrink-0 items-center gap-0.5 rounded-md border p-0.5"
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
              className="text-amber-500"
              aria-label="Reload diff — the file changed on disk"
              onClick={refreshDiff}
            >
              <CircleAlert />
              Reload
            </Button>
          </SimpleTooltip>
        )}
        {/* Markdown preview toggle — file mode only. */}
        {canPreview && (
          <Button
            size="sm"
            variant={showPreview ? "default" : "ghost"}
            aria-pressed={showPreview}
            onClick={togglePreview}
          >
            {showPreview ? <Pencil /> : <Eye />}
            {showPreview ? "Edit" : "Preview"}
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
            <span className="inline-flex">
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
                  Open editor
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
        {/* Save — file mode only (diff is read-only). */}
        {activeTab?.mode === "file" && (
          <Button
            size="sm"
            disabled={!dirty || isSaving || readOnly}
            aria-busy={isSaving}
            onClick={save}
          >
            {isSaving ? <Loader2 className="motion-safe:animate-spin" /> : <Save />}
            Save
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={requestClose}>
          <X />
          Close
        </Button>
      </div>

      {/* Tab strip, only renders once the session has at least one tab. */}
      <EditorTabsStrip sessionId={sessionId} />

      {/* Body: worktree file tree (left) + Monaco editor/diff (right). */}
      <div className="flex min-h-0 flex-1">
        <div className="flex w-64 shrink-0 flex-col border-r">
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
                onClick={() => setNewFileOpen(true)}
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
            />
          )}
        </div>

        <div className="relative min-w-0 flex-1">
          {activeTab === null ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
              Select a file from the tree to view or edit it.
            </div>
          ) : activeTab.mode === "diff" ? (
            // Read-only Monaco diff (HEAD vs working copy).
            activeBuffer?.diffError && !isBufferStale(activeBuffer, activeTab.path) ? (
              <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
                {activeBuffer.diffError}
              </div>
            ) : !diffReady ? (
              <div className="flex h-full items-center justify-center text-muted-foreground">
                <Loader2 className="size-5 motion-safe:animate-spin" />
              </div>
            ) : activeBuffer?.diff?.binary ? (
              <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
                This file is binary and can&rsquo;t be diffed here.
              </div>
            ) : (
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
                  />
                </Suspense>
              </ChunkBoundary>
            )
          ) : activeBuffer?.fileError && !isBufferStale(activeBuffer, activeTab.path) ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
              {activeBuffer.fileError}
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
            // Rendered markdown of the current buffer (unsaved edits included).
            // Lazy like Monaco, so the same ChunkBoundary + Suspense applies.
            <ChunkBoundary>
              <Suspense
                fallback={
                  <div className="flex h-full items-center justify-center text-muted-foreground">
                    <Loader2 className="size-5 motion-safe:animate-spin" />
                  </div>
                }
              >
                <MarkdownPreview
                  content={activeBuffer?.draft ?? ""}
                  sessionId={sessionId}
                  path={activeTab.path}
                />
              </Suspense>
            </ChunkBoundary>
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
      </div>

      {/* New-file prompt. */}
      <Dialog
        open={newFileOpen}
        onOpenChange={(open) => {
          if (!open) {
            setNewFileOpen(false)
            setNewFilePath("")
          }
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>New file</DialogTitle>
            <DialogDescription>
              Worktree-relative path. The parent folder must already exist.
            </DialogDescription>
          </DialogHeader>
          <Input
            value={newFilePath}
            onChange={(e) => setNewFilePath(e.target.value)}
            placeholder="src/example.ts"
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                createFile()
              }
            }}
          />
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => {
                setNewFileOpen(false)
                setNewFilePath("")
              }}
            >
              Cancel
            </Button>
            <Button
              disabled={!newFilePath.trim() || creating}
              aria-busy={creating}
              onClick={createFile}
            >
              {creating ? (
                <Loader2 className="motion-safe:animate-spin" />
              ) : null}
              Create
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Styled unsaved-changes confirmation for closing the WHOLE overlay
          (Esc/backdrop/Close) when any tab is dirty. Per-tab close instead uses
          the store-target `ConfirmCloseEditorTabDialog`. */}
      <Dialog
        open={overlayCloseConfirmOpen}
        onOpenChange={(open) => {
          if (!open) setOverlayCloseConfirmOpen(false)
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Discard unsaved changes?</DialogTitle>
            <DialogDescription>{dirtyCloseMessage(dirtyTabCount)}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              autoFocus
              onClick={() => setOverlayCloseConfirmOpen(false)}
            >
              Keep editing
            </Button>
            <Button variant="destructive" onClick={confirmOverlayClose}>
              Discard
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
