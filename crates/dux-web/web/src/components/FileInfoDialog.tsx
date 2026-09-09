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
import type { EditorRoot } from "@/lib/editorRoot"

export interface FileInfoTarget {
  path: string
}

type InfoResult =
  | { kind: "ok"; info: WorktreeEntryInfo }
  | { kind: "error"; message: string }
  | { kind: "vanished" }

// What a failed read means for the panel. A 404 says the entry is GONE, which the
// vanished-target guard dismisses on; anything else is an answer the user needs to
// read, so the panel stays open carrying it.
function infoResultFromError(e: unknown): InfoResult {
  if (e instanceof FileApiError && e.status === 404) return { kind: "vanished" }
  return {
    kind: "error",
    message: e instanceof Error ? e.message : "could not read file info",
  }
}

interface FileInfoDialogProps {
  root: EditorRoot
  target: FileInfoTarget | null
  onClose: () => void
}

// Read-only "File info…" panel for one worktree entry. It fetches its own facts,
// since none of them live in the ViewModel, which is also what the vanished-target
// guard keys on: a 404 dismisses the panel, while a 400 is a REFUSAL and stays on
// screen with its reason. There is no poll and no subscription, so the facts are as
// fresh as the last fetch, and only opening and the window regaining focus trigger
// one. A file deleted while this tab stays focused is not noticed; that gap is accepted.
export function FileInfoDialog({
  root,
  target,
  onClose,
}: FileInfoDialogProps) {
  const path = target?.path ?? null
  // Tagged with the path it describes, so "still loading" is DERIVED from the tag
  // rather than a synchronous setState in an effect body, which cascades renders.
  const [loaded, setLoaded] = useState<{
    path: string
    result: InfoResult
  } | null>(null)
  // Bumped to ask again for the SAME path, and deliberately not part of the
  // `loaded` tag: a revalidation keeps the facts it has rather than flashing.
  const [revalidateNonce, setRevalidateNonce] = useState(0)
  const result = loaded !== null && loaded.path === path ? loaded.result : null
  const info = result?.kind === "ok" ? result.info : null
  const error = result?.kind === "error" ? result.message : null
  const vanished = result?.kind === "vanished"

  useEffect(() => {
    if (path === null) return
    let cancelled = false
    fileApi
      .info(root, path)
      .then((value) => {
        if (!cancelled) setLoaded({ path, result: { kind: "ok", info: value } })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        setLoaded({ path, result: infoResultFromError(e) })
      })
    return () => {
      cancelled = true
    }
  }, [root, path, revalidateNonce])

  // The panel's only revalidation signal: a returned-to tab is when the facts are
  // most likely stale, and it costs one request rather than a running timer.
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
