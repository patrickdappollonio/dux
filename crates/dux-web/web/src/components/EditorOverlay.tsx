import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react"
import { FileCode2, FilePlus, Loader2, Save, Search, X } from "lucide-react"
import { toast } from "sonner"
import { fileApi } from "@/lib/fileApi"
import type { WorktreeFile } from "@/lib/fileApi"
import { shouldShowChangedFiles, statusGlyph } from "@/lib/changedFiles"
import { ancestorDirs, buildFileTree } from "@/lib/fileTree"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { FileTree } from "@/components/FileTree"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { closeEditor, useDux } from "@/lib/store"

// Monaco is multiple MB; keep it off the main bundle by loading it only when the
// editor actually opens.
const CodeEditor = lazy(() => import("./CodeEditor"))

// Cap how many results the search list renders so a 1-char query in a huge repo
// can't mount thousands of rows.
const MAX_SEARCH_RESULTS = 300

// A pending discard confirmation: closing the overlay, or switching to another
// file, while the current buffer has unsaved edits.
type PendingDiscard = { kind: "close" } | { kind: "switch"; path: string }

// The overlay shell: owns the Dialog and is desktop-only (Monaco is poor on
// touch, and every entry point is already gated to desktop). The body is keyed
// by session+file so each open mounts fresh — no manual state resets. A ref lets
// the body intercept Esc/backdrop closes so they run the same dirty guard as the
// in-body Close button.
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
          Browse and edit files in this worktree.
        </DialogDescription>
        {editorTarget && (
          <EditorBody
            key={`${editorTarget.sessionId}:${editorTarget.initialPath ?? ""}`}
            sessionId={editorTarget.sessionId}
            initialPath={editorTarget.initialPath}
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
  closeReqRef: React.RefObject<() => void>
}

function EditorBody({ sessionId, initialPath, closeReqRef }: EditorBodyProps) {
  const { viewModel } = useDux()
  const [openPath, setOpenPath] = useState<string | null>(initialPath)
  // `loaded` is the on-disk content; `draft` is the current editor buffer.
  const [loaded, setLoaded] = useState("")
  const [draft, setDraft] = useState("")
  const [loading, setLoading] = useState(initialPath !== null)
  const [saving, setSaving] = useState(false)
  const [binary, setBinary] = useState(false)
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
  // Monotonic token so a slow earlier read can never clobber a later one.
  const reqId = useRef(0)

  const dirty = openPath !== null && !binary && draft !== loaded

  // Badge the tree's changed files. Sourced from the (watched) changed-files
  // broadcast, guarded so a different session's list never leaks in.
  const watched = viewModel?.changed_files.watched_session_id ?? null
  const changedMap = useMemo(() => {
    const map = new Map<string, string>()
    if (!shouldShowChangedFiles(watched, sessionId) || !viewModel) return map
    const { staged, unstaged } = viewModel.changed_files
    for (const f of [...unstaged, ...staged]) {
      if (!map.has(f.path)) map.set(f.path, statusGlyph(f.status))
    }
    return map
  }, [viewModel, watched, sessionId])

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

  function applyLoaded(token: number, f: WorktreeFile): void {
    if (reqId.current !== token) return
    setBinary(f.binary)
    setLoaded(f.content)
    setDraft(f.content)
    setLoading(false)
  }

  function onLoadError(token: number, e: unknown): void {
    if (reqId.current !== token) return
    toast.error(e instanceof Error ? e.message : "could not open file")
    setOpenPath(null)
    setLoading(false)
  }

  // Fetch the worktree file list + auto-load the initial file on open. setState
  // happens only in async callbacks (never synchronously in the effect body), so
  // re-renders stay pure; `openPath`/`loading` were seeded from `initialPath`.
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
    if (initialPath !== null) {
      const token = reqId.current
      fileApi
        .read(sessionId, initialPath)
        .then((f) => {
          if (!cancelled) applyLoaded(token, f)
        })
        .catch((e) => {
          if (!cancelled) onLoadError(token, e)
        })
    }
    return () => {
      cancelled = true
    }
    // Mount-only: the component is keyed by session+file, so a new open remounts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Open a file. An event handler, so synchronous setState is fine.
  function load(path: string): void {
    const token = ++reqId.current
    setOpenPath(path)
    setLoading(true)
    setBinary(false)
    fileApi
      .read(sessionId, path)
      .then((f) => applyLoaded(token, f))
      .catch((e) => onLoadError(token, e))
  }

  // Switching files discards the current buffer, so guard it the same way as
  // closing — it must not silently drop unsaved edits.
  function requestSwitch(path: string): void {
    if (path === openPath) return
    if (dirty) {
      setPendingDiscard({ kind: "switch", path })
      return
    }
    load(path)
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
  // can't fire against an unmounted body.
  useEffect(() => {
    closeReqRef.current = requestClose
    return () => {
      closeReqRef.current = closeEditor
    }
  })

  function confirmDiscard(): void {
    const pending = pendingDiscard
    setPendingDiscard(null)
    if (pending?.kind === "switch") load(pending.path)
    else if (pending?.kind === "close") closeEditor()
  }

  function save(): void {
    if (openPath === null || binary || saving || !dirty) return
    setSaving(true)
    const path = openPath
    const body = draft
    fileApi
      .write(sessionId, path, body)
      .then(() => {
        setLoaded(body)
        toast.success(`Saved ${path}`)
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not save file")
      })
      .finally(() => setSaving(false))
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
        requestSwitch(path)
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : "could not create file")
      })
      .finally(() => setCreating(false))
  }

  return (
    <>
      {/* Header: open file path, dirty indicator, Save, Close. */}
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
        <Button
          size="sm"
          disabled={!dirty || saving}
          aria-busy={saving}
          onClick={save}
        >
          {saving ? <Loader2 className="motion-safe:animate-spin" /> : <Save />}
          Save
        </Button>
        <Button size="sm" variant="ghost" onClick={requestClose}>
          <X />
          Close
        </Button>
      </div>

      {/* Body: worktree file tree (left) + Monaco (right). */}
      <div className="flex min-h-0 flex-1">
        <div className="flex w-64 shrink-0 flex-col border-r">
          <div className="flex items-center gap-1 border-b p-2">
            <div className="relative flex-1">
              <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search files…"
                className="h-8 pl-7 text-xs"
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
                  <p className="px-1 py-2 text-xs text-muted-foreground">
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
                        <Badge
                          variant="outline"
                          className="shrink-0 font-mono text-[10px] leading-none"
                        >
                          {changedMap.get(p)}
                        </Badge>
                      )}
                      {/* Full path → start-ellipsize so the filename stays visible. */}
                      <span className="min-w-0 flex-1 truncate text-left font-mono text-xs [direction:rtl]">
                        {p}
                      </span>
                    </button>
                  ))
                )
              ) : tree.length === 0 ? (
                <p className="px-1 py-2 text-xs text-muted-foreground">
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
              Select a file from the tree to edit it.
            </div>
          ) : loading ? (
            <div className="flex h-full items-center justify-center text-muted-foreground">
              <Loader2 className="size-5 motion-safe:animate-spin" />
            </div>
          ) : binary ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
              This file is binary and can&rsquo;t be edited here.
            </div>
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
