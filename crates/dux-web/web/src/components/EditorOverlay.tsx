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
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { closeEditor, useDux } from "@/lib/store"

// Monaco is multiple MB; keep it off the main bundle by loading it only when the
// editor actually opens.
const CodeEditor = lazy(() => import("./CodeEditor"))

// The overlay shell: owns the Dialog and the dirty-aware close guard. The body
// is keyed by session so each open mounts fresh — no manual state resets. A ref
// carries the body's dirty flag up so Esc/backdrop/X can all confirm before
// discarding unsaved work.
export function EditorOverlay() {
  const { editorTarget } = useDux()
  const dirtyRef = useRef(false)

  function requestClose(): void {
    if (dirtyRef.current && !window.confirm("Discard unsaved changes?")) return
    closeEditor()
  }

  return (
    <Dialog
      open={editorTarget !== null}
      onOpenChange={(open) => {
        if (!open) requestClose()
      }}
    >
      <DialogContent
        showCloseButton={false}
        className="flex h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(80rem,calc(100%-2rem))]"
      >
        <DialogTitle className="sr-only">Code editor</DialogTitle>
        {editorTarget && (
          <EditorBody
            key={editorTarget.sessionId}
            sessionId={editorTarget.sessionId}
            initialPath={editorTarget.initialPath}
            dirtyRef={dirtyRef}
            onClose={requestClose}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

interface EditorBodyProps {
  sessionId: string
  initialPath: string | null
  dirtyRef: React.RefObject<boolean>
  onClose: () => void
}

function EditorBody({
  sessionId,
  initialPath,
  dirtyRef,
  onClose,
}: EditorBodyProps) {
  const { viewModel } = useDux()
  const [openPath, setOpenPath] = useState<string | null>(initialPath)
  // `loaded` is the on-disk content; `draft` is the current editor buffer.
  const [loaded, setLoaded] = useState("")
  const [draft, setDraft] = useState("")
  const [loading, setLoading] = useState(initialPath !== null)
  const [saving, setSaving] = useState(false)
  const [binary, setBinary] = useState(false)
  // Monotonic token so a slow earlier read can never clobber a later one.
  const reqId = useRef(0)

  const dirty = openPath !== null && !binary && draft !== loaded
  // Surface the dirty flag to the shell's close guard (in an effect, so renders
  // stay pure).
  useEffect(() => {
    dirtyRef.current = dirty
  }, [dirty, dirtyRef])

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
    // Mount-only: the component is keyed by session, so a new open remounts.
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
          {dirty ? " ●" : ""}
        </span>
        <Button
          size="sm"
          disabled={!dirty || saving}
          aria-busy={saving}
          onClick={save}
        >
          {saving ? <Loader2 className="motion-safe:animate-spin" /> : <Save />}
          Save
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
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
                  onClick={() => load(f.path)}
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
          )}
        </div>
      </div>
    </>
  )
}
