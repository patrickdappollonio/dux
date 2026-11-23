import { useEffect, useState } from "react"

import { Spinner } from "@/components/Spinner"
import { ErrorLine } from "@/components/ErrorLine"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { InfoRow } from "@/components/InfoRow"
import { Modal, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
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
// since none of them live in the ViewModel, which is also what its vanished
// target keys on: a 404 dismisses the panel, while a 400 is a REFUSAL and stays on
// screen with its reason. There is no poll and no subscription, so the facts are as
// fresh as the last fetch, and only opening and the window regaining focus trigger
// one. A file deleted while this tab stays focused is not noticed; that gap is accepted.
export function FileInfoDialog(props: FileInfoDialogProps) {
  const life = useModalLifetime(props.target !== null)
  // Keyed by the open, so an answer from an earlier open (a path that was gone
  // then) never decides this one.
  return life.mounted ? <FileInfoPanel key={life.epoch} {...props} onClosed={life.onClosed} /> : null
}

function FileInfoPanel({
  root,
  target,
  onClose,
  onClosed,
}: FileInfoDialogProps & { onClosed: () => void }) {
  // The entry this open is about; the last one is kept while the close animates.
  const [last, setLast] = useState(target)
  if (target !== null && target !== last) setLast(target)
  const open = target !== null
  const path = (target ?? last)?.path ?? null
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
    if (!open || path === null) return
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
  }, [open, root, path, revalidateNonce])

  // The panel's only revalidation signal: a returned-to tab is when the facts are
  // most likely stale, and it costs one request rather than a running timer.
  useEffect(() => {
    if (!open || path === null) return
    const revalidate = () => setRevalidateNonce((n) => n + 1)
    window.addEventListener("focus", revalidate)
    return () => window.removeEventListener("focus", revalidate)
  }, [open, path])

  return (
    <Modal
      kind="report"
      open={open}
      vanished={open && vanished}
      onClose={() => onClose()}
      onClosed={onClosed}
      size="lg"
      title={
        <span className="block truncate">{path === null ? "File info" : basename(path)}</span>
      }
    >
      {error !== null ? (
        <ErrorLine>{error}</ErrorLine>
      ) : info === null ? (
        <div className="flex items-center justify-center py-6 text-muted-foreground">
          <Spinner />
        </div>
      ) : (
        <dl className="flex flex-col gap-2">
          <InfoRow label="Path">
            <InlineCode>{info.path}</InlineCode>
          </InfoRow>
          <InfoRow label="Kind">{entryKindLabel(info.kind)}</InfoRow>
          {info.symlink_target !== null && (
            <InfoRow label="Links to">
              <InlineCode>{info.symlink_target}</InlineCode>
            </InfoRow>
          )}
          <InfoRow label="Size">{formatBytes(info.size)}</InfoRow>
          <InfoRow label="Modified">{formatModified(info.modified)}</InfoRow>
          <InfoRow label="Permissions">
            <InlineCode>{info.permissions}</InlineCode>
          </InfoRow>
          <InfoRow label="Mode">
            <InlineCode>{info.mode}</InlineCode>
          </InfoRow>
          <InfoRow label="Git">
            <div className="flex flex-col gap-1">
              {gitStatusRows(info.git).map((row) => (
                <span
                  key={`${row.status ?? ""}-${row.label}`}
                  className="flex items-center gap-1.5"
                >
                  {row.status !== undefined && <FileStatusIcon status={row.status} />}
                  {row.label}
                </span>
              ))}
            </div>
          </InfoRow>
        </dl>
      )}
    </Modal>
  )
}
