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
import { shouldShowChangedFiles } from "@/lib/changedFiles"
import { OPEN_IN_EDITORS } from "@/lib/editors"
import { ancestorDirs, buildFileTree } from "@/lib/fileTree"
import { isLocalAccessHost } from "@/lib/localAccess"
import { isMarkdownPath } from "@/lib/markdown"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { EditorIcon } from "@/components/EditorIcon"
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
import { closeEditor, useDux } from "@/lib/store"
import type { EditorViewMode } from "@/lib/store"

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

// A pending discard confirmation: closing the overlay, or switching to another
// file, while the current buffer has unsaved edits.
type PendingDiscard = { kind: "close" } | { kind: "switch"; path: string }

// The overlay shell: owns the Dialog and is desktop-only (Monaco is poor on
// touch, and every entry point is already gated to desktop). The body is keyed
// by session+file+mode so each open mounts fresh — no manual state resets. A ref
// lets the body intercept Esc/backdrop closes so they run the same dirty guard as
// the in-body Close button.
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
            key={`${editorTarget.sessionId}:${editorTarget.initialPath ?? ""}:${editorTarget.initialMode}`}
            sessionId={editorTarget.sessionId}
            initialPath={editorTarget.initialPath}
            initialMode={editorTarget.initialMode}
            closeReqRef={closeReqRef}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

interface EditorBodyProps {
  sessionId: string
  initialPath: string | null
  initialMode: EditorViewMode
  closeReqRef: React.RefObject<() => void>
}

function EditorBody({
  sessionId,
  initialPath,
  initialMode,
  closeReqRef,
}: EditorBodyProps) {
  const { viewModel } = useDux()
  const [openPath, setOpenPath] = useState<string | null>(initialPath)
  // "file" shows the editable Monaco buffer; "diff" shows the read-only Monaco
  // DiffEditor (HEAD vs working copy). Toggled in the header.
  const [mode, setMode] = useState<EditorViewMode>(initialMode)
  // File-buffer state. `loaded` is the on-disk content; `draft` is the editor
  // buffer; `loadedPath` is which file the buffer currently holds (null until the
  // first successful read), so the load effect knows when a (re)read is needed.
  const [loaded, setLoaded] = useState("")
  const [draft, setDraft] = useState("")
  const [loadedPath, setLoadedPath] = useState<string | null>(null)
  // The path currently being saved (null = idle). Path-scoped so a save in flight
  // for one file doesn't disable the Save button on a file the user switched to.
  const [saving, setSaving] = useState<string | null>(null)
  const [binary, setBinary] = useState(false)
  // Diff state, loaded lazily and independently of the file buffer. The diff is
  // cached per FILE PATH (`diffLoadedPath`) and refetched fresh whenever you open
  // or switch to a file's diff — simple and always-correct-on-open. While you keep
  // viewing one file, an external write (agent/git) does NOT silently refetch;
  // instead `diffStale` lights a reload button. `diffLoadedSignal` is the changed-
  // files signal captured when the diff was fetched, so a later signal change flags
  // staleness. A save invalidates the cache (clears diffLoadedPath).
  const [diff, setDiff] = useState<FileDiffContents | null>(null)
  const [diffLoadedPath, setDiffLoadedPath] = useState<string | null>(null)
  const [diffLoadedSignal, setDiffLoadedSignal] = useState<string>("")
  // Load errors carry the path they happened on. Loading itself is DERIVED (the
  // loaded path / diff path not yet matching the current one) rather than a
  // separate flag, so the load effects never call setState synchronously — they
  // only resolve in their async callbacks. Scoping the error to its path means a
  // stale error never shows against a different file and needs no synchronous reset.
  const [fileError, setFileError] = useState<{ path: string; message: string } | null>(null)
  const [diffError, setDiffError] = useState<{ path: string; message: string } | null>(null)
  // True once the file buffer holds the CURRENT open path.
  const fileReady = openPath !== null && loadedPath === openPath
  // Refs to the latest open path + change-signal, read by async callbacks (a save
  // or diff fetch resolving) so they see the CURRENT value rather than the one
  // captured when they started: openPathRef lets a stale save skip its state
  // writes; openFileSignalRef stamps a freshly-loaded diff with the load-time signal.
  const openPathRef = useRef(openPath)
  const openFileSignalRef = useRef("")
  const [pendingDiscard, setPendingDiscard] = useState<PendingDiscard | null>(
    null,
  )
  // The worktree's browsable files (fetched from the editor's session directly,
  // independent of the changed-files watch).
  const [treeFiles, setTreeFiles] = useState<string[]>([])
  const [treeLoading, setTreeLoading] = useState(true)
  const [search, setSearch] = useState("")
  const [newFileOpen, setNewFileOpen] = useState(false)
  const [newFilePath, setNewFilePath] = useState("")
  const [creating, setCreating] = useState(false)
  // Markdown preview toggle: render the buffer instead of the Monaco editor.
  const [preview, setPreview] = useState(false)
  // "Open editor" request in flight.
  const [openingEditor, setOpeningEditor] = useState(false)

  // The buffer is dirty only when it holds the open file and has unsaved edits.
  // Independent of `mode` so edits made in file mode still guard a file switch
  // even while the diff is showing. Diff mode is read-only so it never dirties.
  const dirty =
    openPath !== null &&
    loadedPath === openPath &&
    !binary &&
    draft !== loaded
  const isMarkdown = openPath !== null && isMarkdownPath(openPath)
  // Markdown preview is available only for a loaded, non-binary markdown file in
  // file mode — one source of truth for both the toggle button and the render.
  const canPreview = mode === "file" && isMarkdown && fileReady && !binary
  const showPreview = preview && canPreview
  // "Open editor" spawns a GUI editor on the SERVER, so it only helps when the
  // server is the user's own machine. Enable for local-access URLs; for remote
  // URLs keep the control but disable it with an explanatory tooltip.
  const localAccess = isLocalAccessHost(window.location.hostname)

  // Mark the tree's changed files. Sourced from the (watched) changed-files
  // broadcast, guarded so a different session's list never leaks in. Stores the
  // raw git status code per path; FileStatusIcon maps it to an icon + label.
  const watched = viewModel?.changed_files.watched_session_id ?? null
  const changedMap = useMemo(() => {
    const map = new Map<string, string>()
    if (!shouldShowChangedFiles(watched, sessionId) || !viewModel) return map
    const { staged, unstaged } = viewModel.changed_files
    for (const f of [...unstaged, ...staged]) {
      if (!map.has(f.path)) map.set(f.path, f.status)
    }
    return map
  }, [viewModel, watched, sessionId])

  // A per-file change-signal for the open file from the same broadcast: status +
  // line counts move when the file's content changes (best-effort — an edit that
  // keeps identical +/- counts won't move it). Used ONLY to flag a stale diff, not
  // to key the cache, so it never drives a refetch. Scanning unstaged then staged
  // avoids allocating a combined array on every tick.
  const openFileSignal = useMemo(() => {
    if (
      openPath === null ||
      !viewModel ||
      !shouldShowChangedFiles(watched, sessionId)
    ) {
      return ""
    }
    const { staged, unstaged } = viewModel.changed_files
    const f =
      unstaged.find((x) => x.path === openPath) ??
      staged.find((x) => x.path === openPath)
    return f ? `${f.status}:${f.additions}:${f.deletions}` : ""
  }, [openPath, viewModel, watched, sessionId])
  // The diff is cached per path; ready when the loaded diff is for the open file.
  // While ready, a change-signal differing from the one captured at load means the
  // file changed underneath — surface a reload button (diffStale).
  const diffReady = openPath !== null && diffLoadedPath === openPath
  const diffStale = diffReady && openFileSignal !== diffLoadedSignal
  // This file's save is in flight (path-scoped — see `saving`).
  const isSaving = saving !== null && saving === openPath

  const tree = useMemo(() => buildFileTree(treeFiles), [treeFiles])
  const defaultExpanded = useMemo(
    () => new Set(initialPath ? ancestorDirs(initialPath) : []),
    [initialPath],
  )
  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase()
    if (!needle) return []
    return treeFiles
      .filter((f) => f.toLowerCase().includes(needle))
      .slice(0, MAX_SEARCH_RESULTS)
  }, [search, treeFiles])

  // Fetch the worktree file list on open. The file/diff content loads are driven
  // by the mode-aware effects below. Mount-only: the body is keyed by
  // session+file+mode, so a new open remounts.
  useEffect(() => {
    let cancelled = false
    fileApi
      .list(sessionId)
      .then((files) => {
        if (!cancelled) setTreeFiles(files)
      })
      .catch(() => {
        if (!cancelled) toast.error("could not list worktree files")
      })
      .finally(() => {
        if (!cancelled) setTreeLoading(false)
      })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Load the file buffer lazily: only in file mode, only when the buffer doesn't
  // already hold the open file. Skipped entirely in diff mode, so clicking a
  // DELETED changed file (diff opens by default) never hits a doomed working
  // read — the diff endpoint handles the missing side server-side. Unlike the
  // diff, the buffer is NOT auto-refreshed when the file changes on disk under us:
  // re-reading could silently clobber unsaved edits, so file mode shows the
  // content as loaded (the save/dirty model owns writes). Reopen to pull fresh.
  useEffect(() => {
    if (mode !== "file" || openPath === null || loadedPath === openPath) return
    let cancelled = false
    const path = openPath
    fileApi
      .read(sessionId, path)
      .then((f) => {
        if (cancelled) return
        setBinary(f.binary)
        setLoaded(f.content)
        setDraft(f.content)
        setLoadedPath(path)
        setFileError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setFileError({
          path,
          message: e instanceof Error ? e.message : "could not open file",
        })
      })
    return () => {
      cancelled = true
    }
  }, [mode, openPath, loadedPath, sessionId])

  // Load the diff lazily: only in diff mode, only when the cache doesn't already
  // hold the open file. Refetches on a file switch and on manual reload (which
  // clears diffLoadedPath); it does NOT refetch on a change-signal tick — that
  // lights the reload button instead. Stamps the loaded diff with the signal at
  // resolve time so a subsequent change is detected as stale.
  useEffect(() => {
    if (mode !== "diff" || openPath === null || diffLoadedPath === openPath) return
    let cancelled = false
    const path = openPath
    fileApi
      .diff(sessionId, path)
      .then((d) => {
        if (cancelled) return
        setDiff(d)
        setDiffLoadedPath(path)
        setDiffLoadedSignal(openFileSignalRef.current)
        setDiffError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDiffError({
          path,
          message: e instanceof Error ? e.message : "could not load diff",
        })
      })
    return () => {
      cancelled = true
    }
  }, [mode, openPath, diffLoadedPath, sessionId])

  // Switching files discards the current buffer, so guard it the same way as
  // closing — it must not silently drop unsaved edits. The mode-aware effects
  // reload the new file's content for whichever view is showing.
  function requestSwitch(path: string): void {
    if (path === openPath) return
    if (dirty) {
      setPendingDiscard({ kind: "switch", path })
      return
    }
    // Sync the ref BEFORE the state update so an in-flight save's microtask sees
    // the new path immediately (the post-render effect would lag by a commit).
    openPathRef.current = path
    setOpenPath(path)
    setPreview(false)
  }

  function requestClose(): void {
    if (dirty) {
      setPendingDiscard({ kind: "close" })
      return
    }
    closeEditor()
  }

  // Let the shell's Esc/backdrop close run this same dirty guard. Updated every
  // render (so it sees the latest `dirty`), reset on unmount so a stale closure
  // can't fire against an unmounted body. (openPathRef is synced synchronously at
  // the switch sites so a save microtask can't read a stale value.)
  useEffect(() => {
    closeReqRef.current = requestClose
    // Kept current here (an effect, not render) so the diff fetch's late callback
    // stamps the diff with the signal as of load-resolve time.
    openFileSignalRef.current = openFileSignal
    return () => {
      closeReqRef.current = closeEditor
    }
  })

  function confirmDiscard(): void {
    const pending = pendingDiscard
    setPendingDiscard(null)
    if (pending?.kind === "switch") {
      openPathRef.current = pending.path
      setOpenPath(pending.path)
      setPreview(false)
    } else if (pending?.kind === "close") {
      closeEditor()
    }
  }

  function save(): void {
    if (openPath === null || binary || isSaving || !dirty) return
    const path = openPath
    const body = draft
    setSaving(path)
    fileApi
      .write(sessionId, path, body)
      .then(() => {
        // If the user switched files while the write was in flight, this callback
        // is stale — don't write `loaded`/diff state for a file no longer open
        // (which would leave a phantom-dirty buffer on the new file). The toast
        // still fires: the save did happen.
        if (openPathRef.current === path) {
          setLoaded(body)
          // The on-disk content changed, so the cached diff is stale — drop it so
          // re-entering diff refetches.
          setDiffLoadedPath(null)
        }
        toast.success(`Saved ${path}`)
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not save file")
      })
      // Clear only if THIS file's save is still the active one — a newer save for a
      // different file must not be cleared by an older one resolving late.
      .finally(() => setSaving((s) => (s === path ? null : s)))
  }

  // Reload the diff for the open file — the "file changed underneath you" reload
  // button. Dropping diffLoadedPath makes the diff-load effect refetch fresh.
  function refreshDiff(): void {
    setDiffLoadedPath(null)
  }

  // Open the current file in a locally-installed GUI editor (server-side spawn).
  // `editorKey` is the one the user picked from the menu; the server launches it
  // or reports it isn't installed. Only reachable when `localAccess` is true.
  function openInEditorAction(editorKey: string): void {
    if (openPath === null || openingEditor) return
    setOpeningEditor(true)
    fileApi
      .openInEditor(sessionId, openPath, editorKey)
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
      .then((files) => {
        setTreeFiles(files)
        setNewFileOpen(false)
        setNewFilePath("")
        // A brand-new file is for editing, so land in file mode.
        setMode("file")
        requestSwitch(path)
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not create file")
      })
      .finally(() => setCreating(false))
  }

  return (
    <>
      {/* Header: open file path, view toggle, dirty indicator, actions. */}
      <div className="flex items-center gap-2 border-b px-3 py-2">
        <FileCode2 className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl]">
          {openPath ?? "Select a file"}
        </span>
        {/* Dirty dot kept OUTSIDE the truncating span so it can't be clipped on
            a long path; sr-only text announces the state to screen readers. */}
        {dirty && (
          <>
            <span
              className="shrink-0 text-primary"
              aria-hidden="true"
              title="Unsaved changes"
            >
              ●
            </span>
            <span className="sr-only">unsaved changes</span>
          </>
        )}
        {/* File / Diff view toggle — a segmented control. Hidden until a file is
            open (nothing to view otherwise). */}
        {openPath !== null && (
          <div
            className="flex shrink-0 items-center gap-0.5 rounded-md border p-0.5"
            role="group"
            aria-label="View mode"
          >
            <Button
              size="sm"
              variant={mode === "file" ? "default" : "ghost"}
              aria-pressed={mode === "file"}
              onClick={() => setMode("file")}
            >
              <FileText />
              File
            </Button>
            <Button
              size="sm"
              variant={mode === "diff" ? "default" : "ghost"}
              aria-pressed={mode === "diff"}
              onClick={() => setMode("diff")}
            >
              <GitCompare />
              Diff
            </Button>
          </div>
        )}
        {/* "File changed underneath you" reload — shown in diff mode when the
            changed-files broadcast indicates the open file moved since the diff was
            loaded. We don't auto-refetch (avoids churn); the user reloads on click. */}
        {mode === "diff" && diffStale && (
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
            onClick={() => setPreview((p) => !p)}
          >
            {showPreview ? <Pencil /> : <Eye />}
            {showPreview ? "Edit" : "Preview"}
          </Button>
        )}
        {/* Open in a local GUI editor — a menu of supported editors. A disabled
            trigger swallows hover events (pointer-events:none), so the tooltip
            lives on a wrapping span that always receives them. */}
        {openPath !== null && (
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
        {mode === "file" && (
          <Button
            size="sm"
            disabled={!dirty || isSaving}
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
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="New file"
              title="New file"
              onClick={() => setNewFileOpen(true)}
            >
              <FilePlus />
            </Button>
          </div>
          <ScrollArea className="min-h-0 flex-1">
            <div className="p-1">
              {treeLoading ? (
                <div className="flex items-center justify-center py-4 text-muted-foreground">
                  <Loader2 className="size-4 motion-safe:animate-spin" />
                </div>
              ) : search.trim() ? (
                filtered.length === 0 ? (
                  <p className="px-1 py-2 text-sm text-muted-foreground">
                    No files match.
                  </p>
                ) : (
                  filtered.map((p) => (
                    <button
                      key={p}
                      type="button"
                      onClick={() => requestSwitch(p)}
                      className={cn(
                        "flex w-full items-center gap-1.5 rounded px-1 py-1 hover:bg-muted",
                        p === openPath && "bg-muted",
                      )}
                    >
                      {changedMap.has(p) && (
                        <FileStatusIcon status={changedMap.get(p)!} />
                      )}
                      {/* Full path → start-ellipsize so the filename stays visible. */}
                      <span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl]">
                        {p}
                      </span>
                    </button>
                  ))
                )
              ) : tree.length === 0 ? (
                <p className="px-1 py-2 text-sm text-muted-foreground">
                  No files in this worktree.
                </p>
              ) : (
                <FileTree
                  nodes={tree}
                  openPath={openPath}
                  changed={changedMap}
                  defaultExpanded={defaultExpanded}
                  onOpen={requestSwitch}
                />
              )}
            </div>
          </ScrollArea>
        </div>

        <div className="relative min-w-0 flex-1">
          {openPath === null ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
              Select a file from the tree to view or edit it.
            </div>
          ) : mode === "diff" ? (
            // Read-only Monaco diff (HEAD vs working copy).
            diffError?.path === openPath ? (
              <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
                {diffError.message}
              </div>
            ) : !diffReady ? (
              <div className="flex h-full items-center justify-center text-muted-foreground">
                <Loader2 className="size-5 motion-safe:animate-spin" />
              </div>
            ) : diff?.binary ? (
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
                    path={openPath}
                    original={diff?.original ?? ""}
                    modified={diff?.modified ?? ""}
                  />
                </Suspense>
              </ChunkBoundary>
            )
          ) : fileError?.path === openPath ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
              {fileError.message}
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
                <MarkdownPreview content={draft} />
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
                  path={openPath}
                  value={draft}
                  onChange={setDraft}
                  onSave={save}
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

      {/* Styled unsaved-changes confirmation (replaces window.confirm to honor
          the design-system + Space-activates-focused-button tenet). */}
      <Dialog
        open={pendingDiscard !== null}
        onOpenChange={(open) => {
          if (!open) setPendingDiscard(null)
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Discard unsaved changes?</DialogTitle>
            <DialogDescription>
              Your edits haven&rsquo;t been saved. They will be lost.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              autoFocus
              onClick={() => setPendingDiscard(null)}
            >
              Keep editing
            </Button>
            <Button variant="destructive" onClick={confirmDiscard}>
              Discard
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
