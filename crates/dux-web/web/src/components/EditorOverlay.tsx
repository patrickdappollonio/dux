import { lazy, Suspense, useEffect, useRef, useState } from "react"
import { FileCode2, Loader2, Save, X } from "lucide-react"
import { toast } from "sonner"
import { fileApi } from "@/lib/fileApi"
import type { WorktreeFile } from "@/lib/fileApi"
import {
  editableFiles,
  shouldShowChangedFiles,
  statusGlyph,
} from "@/lib/changedFiles"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { closeEditor, useDux } from "@/lib/store"

// Monaco is multiple MB; keep it off the main bundle by loading it only when the
// editor actually opens.
const CodeEditor = lazy(() => import("./CodeEditor"))

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
          Edit changed files in this worktree.
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
  // Monotonic token so a slow earlier read can never clobber a later one.
  const reqId = useRef(0)

  const dirty = openPath !== null && !binary && draft !== loaded

  const watched = viewModel?.changed_files.watched_session_id ?? null
  const listReady = shouldShowChangedFiles(watched, sessionId)
  const files =
    listReady && viewModel
      ? editableFiles(
          viewModel.changed_files.staged,
          viewModel.changed_files.unstaged,
        )
      : []

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

  // Auto-load the initial file on open. setState happens only in the async
  // callbacks (never synchronously in the effect body), so re-renders stay pure;
  // `openPath`/`loading` were already seeded from `initialPath` above.
  useEffect(() => {
    if (initialPath === null) return
    const token = reqId.current
    let cancelled = false
    fileApi
      .read(sessionId, initialPath)
      .then((f) => {
        if (!cancelled) applyLoaded(token, f)
      })
      .catch((e) => {
        if (!cancelled) onLoadError(token, e)
      })
    return () => {
      cancelled = true
    }
    // Mount-only: the component is keyed by session+file, so a new open remounts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Open a file from the list. An event handler, so synchronous setState is fine.
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
  // closing — this is the most common in-editor action and must not silently
  // drop unsaved edits.
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

  return (
    <>
      {/* Header: open file path, dirty indicator, Save, Close. */}
      <div className="flex items-center gap-2 border-b px-3 py-2">
        <FileCode2 className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl] [unicode-bidi:plaintext]">
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

      {/* Body: changed-files list (left) + Monaco (right). */}
      <div className="flex min-h-0 flex-1">
        <ScrollArea className="w-64 shrink-0 border-r">
          <div className="flex flex-col gap-0.5 p-2">
            {files.length === 0 ? (
              <p className="px-1 py-2 text-xs text-muted-foreground">
                No changed files to edit. Make a change and it shows up here.
              </p>
            ) : (
              files.map((f) => (
                <button
                  key={f.path}
                  type="button"
                  onClick={() => requestSwitch(f.path)}
                  className={cn(
                    "flex items-center gap-2 rounded px-1 py-1 hover:bg-muted",
                    f.path === openPath && "bg-muted",
                  )}
                >
                  <Badge
                    variant="outline"
                    className="shrink-0 font-mono leading-none"
                  >
                    {statusGlyph(f.status)}
                  </Badge>
                  <span className="min-w-0 flex-1 truncate text-left font-mono text-xs [direction:rtl] [unicode-bidi:plaintext]">
                    {f.path}
                  </span>
                </button>
              ))
            )}
          </div>
        </ScrollArea>

        <div className="relative min-w-0 flex-1">
          {openPath === null ? (
            <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
              Select a file from the list to edit it.
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
