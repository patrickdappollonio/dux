import { useEffect, useState } from "react"
import { Loader2 } from "lucide-react"

import { FileStatusIcon } from "@/components/FileStatusIcon"
import { InfoRow } from "@/components/InfoRow"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { FileApiError, fileApi } from "@/lib/fileApi"
import {
  entryKindLabel,
  formatBytes,
  formatModified,
  gitStatusRows,
} from "@/lib/fileInfo"
import type { WorktreeEntryInfo } from "@/lib/fileInfo"
import { basename } from "@/lib/fileTreeOps"

export interface FileInfoTarget {
  path: string
}

type InfoResult =
  | { kind: "ok"; info: WorktreeEntryInfo }
  | { kind: "error"; message: string }
  | { kind: "vanished" }

interface FileInfoDialogProps {
  sessionId: string
  target: FileInfoTarget | null
  onClose: () => void
}

// Read-only "File info…" panel: path, kind, size, modified time, permissions,
// and git status for one worktree entry. A Report-family surface in the TUI's
// vocabulary: it scrolls, and its only control is the dismissal.
//
// It fetches its own facts rather than taking them as props, because none of
// them live in the ViewModel: the file tree is a client-owned lazy cache and a
// stat is not broadcast to anybody. That also gives the vanished-target guard
// something REAL to key on. The other editor dialogs (New/Rename/Delete/Move)
// deliberately skip the guard because they have no live truth about the tree;
// this one asks the server directly, so a 404 means the entry is gone and the
// panel dismisses itself. A 400 is the opposite answer, a path that was
// REFUSED, and that must stay on screen with its reason.
//
// Be precise about WHEN it learns that, because the panel used to claim more
// than it did: there is no poll and no subscription, so the facts are exactly
// as fresh as the last fetch. Two things trigger one, the panel opening and
// the WINDOW REGAINING FOCUS, and the second is the whole reason a file can
// vanish "while the panel is open" at all. It covers the real journey (delete
// the file in a terminal or another tab, come back to this one) at the cost of
// one request per return, and it needs no timer. A file deleted by an agent
// while this tab stays focused is NOT noticed, and that is the accepted gap.
export function FileInfoDialog({
  sessionId,
  target,
  onClose,
}: FileInfoDialogProps) {
  const path = target?.path ?? null
  // Tagged with the path it describes, so "still loading" is DERIVED from the
  // tag not matching the path being asked about rather than written by a
  // synchronous setState in the effect body (which cascades renders).
  const [loaded, setLoaded] = useState<{
    path: string
    result: InfoResult
  } | null>(null)
  // Bumped to ask again for the SAME path. Not part of the `loaded` tag on
  // purpose: a revalidation must keep showing the facts it already has rather
  // than flashing the spinner, so only the answer changing is visible.
  const [revalidateNonce, setRevalidateNonce] = useState(0)
  const result = loaded !== null && loaded.path === path ? loaded.result : null
  const info = result?.kind === "ok" ? result.info : null
  const error = result?.kind === "error" ? result.message : null
  const vanished = result?.kind === "vanished"

  useEffect(() => {
    if (path === null) return
    let cancelled = false
    fileApi
      .info(sessionId, path)
      .then((value) => {
        if (!cancelled) setLoaded({ path, result: { kind: "ok", info: value } })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        // A 404 means the entry is GONE, which is what the vanished-target
        // guard below reacts to. Anything else (a 400 refusal, a transport
        // failure) is an answer the user needs to read.
        if (e instanceof FileApiError && e.status === 404) {
          setLoaded({ path, result: { kind: "vanished" } })
          return
        }
        setLoaded({
          path,
          result: {
            kind: "error",
            message:
              e instanceof Error ? e.message : "could not read file info",
          },
        })
      })
    return () => {
      cancelled = true
    }
  }, [sessionId, path, revalidateNonce])

  // The panel's only revalidation signal. A tab the user has come back to is
  // exactly when its facts are most likely to be stale, and it costs one
  // request rather than a timer that runs for as long as the panel is open.
  useEffect(() => {
    if (path === null) return
    const revalidate = () => setRevalidateNonce((n) => n + 1)
    window.addEventListener("focus", revalidate)
    return () => window.removeEventListener("focus", revalidate)
  }, [path])

  const isOpen = useVanishedTargetGuard(target !== null, !vanished, onClose)

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle className="truncate">
            {path === null ? "File info" : basename(path)}
          </DialogTitle>
        </DialogHeader>
        {/* Read-only and scrollable: a long path or a stack of git rows must
            never push the dismiss button off the dialog. */}
        <ScrollArea className="max-h-[60vh]">
          {error !== null ? (
            <p className="text-sm text-destructive">{error}</p>
          ) : info === null ? (
            <div className="flex items-center justify-center py-6 text-muted-foreground">
              <Loader2 className="size-4 motion-safe:animate-spin" />
            </div>
          ) : (
            <dl className="flex flex-col gap-2 pr-3">
              <InfoRow label="Path">
                <span className="font-mono break-all">{info.path}</span>
              </InfoRow>
              <InfoRow label="Kind">{entryKindLabel(info.kind)}</InfoRow>
              {info.symlink_target !== null && (
                <InfoRow label="Links to">
                  <span className="font-mono break-all">
                    {info.symlink_target}
                  </span>
                </InfoRow>
              )}
              <InfoRow label="Size">{formatBytes(info.size)}</InfoRow>
              <InfoRow label="Modified">
                {formatModified(info.modified)}
              </InfoRow>
              <InfoRow label="Permissions">
                <span className="font-mono">{info.permissions}</span>
              </InfoRow>
              <InfoRow label="Mode">
                <span className="font-mono">{info.mode}</span>
              </InfoRow>
              <InfoRow label="Git">
                <div className="flex flex-col gap-1">
                  {gitStatusRows(info.git).map((row) => (
                    <span
                      key={`${row.status ?? ""}-${row.label}`}
                      className="flex items-center gap-1.5"
                    >
                      {row.status !== undefined && (
                        <FileStatusIcon status={row.status} />
                      )}
                      {row.label}
                    </span>
                  ))}
                </div>
              </InfoRow>
            </dl>
          )}
        </ScrollArea>
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
