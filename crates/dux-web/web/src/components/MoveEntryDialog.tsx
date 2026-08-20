import { useEffect, useRef, useState } from "react"
import { ChevronRight, CornerLeftUp, Loader2, RotateCw } from "lucide-react"

import { FileTreeIcon } from "@/components/FileTreeIcon"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { dirIconKind } from "@/lib/fileIcons"
import { fileApi } from "@/lib/fileApi"
import type { DirEntry } from "@/lib/fileTree"
import { basename, moveTarget, parentDir, validateMove } from "@/lib/fileTreeOps"
import type { EditorRoot } from "@/lib/editorRoot"

export interface MoveEntryTarget {
  path: string
  isDir: boolean
}

interface MoveEntryDialogProps {
  root: EditorRoot
  target: MoveEntryTarget | null
  // True when `target` (or, for a folder, a descendant of it) has unsaved
  // changes, computed by the caller exactly as RenameEntryDialog's `isDirty`
  // is. A move is a rename on disk, so the same hazard applies: the tab would
  // be retargeted to the new path and reloaded, silently dropping the draft.
  isDirty: boolean
  onClose: () => void
  onSubmit: (destDir: string) => Promise<void>
}

// Move a file or folder to another directory.
//
// This is deliberately a MODAL and not cut/copy/paste. A clipboard model needs
// somewhere to hold a pending entry across unrelated interactions, a way to
// show that something is held, and its own conflict rules on paste; a modal
// answers the same question ("where should this go?") in one interaction with
// nothing to remember.
//
// The destination is chosen by BROWSING, one directory per request through the
// same lazy `tree` endpoint the file explorer uses, so a huge worktree costs
// nothing to open and an empty directory is still reachable (a list derived
// from file paths would omit it).
export function MoveEntryDialog({
  root,
  target,
  isDirty,
  onClose,
  onSubmit,
}: MoveEntryDialogProps) {
  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-lg">
        {target && (
          // Keyed by path so opening the dialog on a different entry starts a
          // fresh browse at that entry's own folder rather than wherever the
          // previous move happened to leave off.
          <MoveEntryDialogBody
            key={target.path}
            root={root}
            target={target}
            isDirty={isDirty}
            onClose={onClose}
            onSubmit={onSubmit}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

type BrowseState =
  | { status: "loading" }
  | { status: "loaded"; entries: DirEntry[] }
  | { status: "error"; message: string }

function MoveEntryDialogBody({
  root,
  target,
  isDirty,
  onClose,
  onSubmit,
}: {
  root: EditorRoot
  target: MoveEntryTarget
  isDirty: boolean
  onClose: () => void
  onSubmit: (destDir: string) => Promise<void>
}) {
  // Start where the entry already is: the common move is a short hop, and
  // starting at the root would make every move begin by re-walking there.
  const [dir, setDir] = useState(() => parentDir(target.path))
  const [submitting, setSubmitting] = useState(false)
  const [reloadNonce, setReloadNonce] = useState(0)
  // The last listing that came back, tagged with the directory (and attempt)
  // it belongs to. "Loading" is DERIVED from that tag not matching what is
  // being browsed now, rather than written by the effect: a synchronous
  // setState in an effect body cascades renders (and the lint says so).
  const [loaded, setLoaded] = useState<{
    key: string
    state: BrowseState
  } | null>(null)
  const key = `${reloadNonce}:${dir}`
  const state: BrowseState =
    loaded !== null && loaded.key === key ? loaded.state : { status: "loading" }

  useEffect(() => {
    let cancelled = false
    fileApi
      .tree(root, dir)
      .then((result) => {
        if (!cancelled) {
          setLoaded({ key, state: { status: "loaded", entries: result.entries } })
        }
      })
      .catch((e: unknown) => {
        if (cancelled) return
        setLoaded({
          key,
          state: {
            status: "error",
            message:
              e instanceof Error ? e.message : "could not list that directory",
          },
        })
      })
    return () => {
      cancelled = true
    }
  }, [root, dir, key])

  // Stepping into (or out of) a folder UNMOUNTS the button that was clicked,
  // and the browser then drops focus onto the dialog container, so a keyboard
  // user restarts their Tab walk at every level. Put focus back on the first
  // control of the freshly rendered list instead, which is where they were.
  // Skipped on the initial listing, where the dialog's own autofocus decides.
  const listRef = useRef<HTMLDivElement | null>(null)
  const navigatedRef = useRef(false)
  function navigateTo(next: string): void {
    navigatedRef.current = true
    setDir(next)
  }
  useEffect(() => {
    if (!navigatedRef.current) return
    if (state.status !== "loaded") return
    navigatedRef.current = false
    listRef.current?.querySelector("button")?.focus()
  }, [state.status, dir])

  const validation = validateMove(target.path, dir)
  const canSubmit = !isDirty && validation.ok && !submitting
  const destination = moveTarget(target.path, dir)

  function submit(): void {
    if (!canSubmit) return
    setSubmitting(true)
    onSubmit(dir).finally(() => setSubmitting(false))
  }

  // Only folders are destinations. A symlinked directory whose target escapes
  // the worktree is already excluded by this one test, because `list_dir`
  // reports it as `is_dir: false` (it sets `is_dir` and `expandable` to the
  // same value on every branch, so there is no such thing on the wire as a
  // directory that cannot be walked into, and a second `expandable` check here
  // would only look like it was doing something).
  const folders =
    state.status === "loaded" ? state.entries.filter((e) => e.is_dir) : []

  return (
    <>
      <DialogHeader>
        <DialogTitle className="truncate">
          Move {basename(target.path)}
        </DialogTitle>
      </DialogHeader>

      {/* Where the browse currently sits, and where the entry would land. */}
      <div className="flex flex-col gap-1">
        <p className="text-sm text-muted-foreground">
          Destination folder:{" "}
          <span className="font-mono break-all text-foreground">
            {dir === "" ? "/ (worktree root)" : dir}
          </span>
        </p>
        <p className="truncate font-mono text-sm">{destination}</p>
      </div>

      <ScrollArea className="h-56 rounded-md border">
        <div ref={listRef} className="flex flex-col p-1">
          {dir !== "" && (
            <button
              type="button"
              onClick={() => navigateTo(parentDir(dir))}
              className="flex items-center gap-1.5 rounded px-2 py-1 text-left text-sm hover:bg-muted max-md:min-h-10"
            >
              <CornerLeftUp className="size-3.5 shrink-0 text-muted-foreground" />
              Up one level
            </button>
          )}
          {state.status === "loading" ? (
            <div className="flex items-center justify-center py-6 text-muted-foreground">
              <Loader2 className="size-4 motion-safe:animate-spin" />
            </div>
          ) : state.status === "error" ? (
            <div className="flex flex-col items-start gap-1 px-2 py-2">
              <p className="text-sm text-destructive">{state.message}</p>
              <button
                type="button"
                onClick={() => setReloadNonce((n) => n + 1)}
                className="flex items-center gap-1 rounded px-1 py-0.5 text-sm text-muted-foreground hover:bg-muted max-md:min-h-10"
              >
                <RotateCw className="size-3.5" />
                Retry
              </button>
            </div>
          ) : folders.length === 0 ? (
            <p className="px-2 py-2 text-sm text-muted-foreground">
              No folders here. This folder can still be the destination.
            </p>
          ) : (
            folders.map((entry) => (
              <button
                key={entry.path}
                type="button"
                onClick={() => navigateTo(entry.path)}
                className="flex items-center gap-1.5 rounded px-2 py-1 text-left text-sm hover:bg-muted max-md:min-h-10"
              >
                <FileTreeIcon
                  kind={dirIconKind({ open: false, empty: false })}
                />
                <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
              </button>
            ))
          )}
        </div>
      </ScrollArea>

      {isDirty ? (
        <p className="text-sm text-destructive">
          Save or discard changes in this file before moving it.
        </p>
      ) : (
        !validation.ok && (
          // "It is already here" is the state every move dialog OPENS in, not
          // a mistake the user made, so it reads as a hint. Everything else
          // (a folder aimed inside itself) really is wrong and reads as such.
          <p
            className={
              dir === parentDir(target.path)
                ? "text-sm text-muted-foreground"
                : "text-sm text-destructive"
            }
          >
            {validation.error}
          </p>
        )
      )}
      {/* Misclick-safe spacing between the browse list and the buttons. */}
      <div className="h-2" />
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button disabled={!canSubmit} aria-busy={submitting} onClick={submit}>
          {submitting ? <Loader2 className="motion-safe:animate-spin" /> : null}
          Move here
        </Button>
      </DialogFooter>
    </>
  )
}
