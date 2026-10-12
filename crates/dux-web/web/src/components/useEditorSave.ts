import { useState } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import {
  FileConflictError,
  FileUnmappableError,
  fileApi,
} from "@/lib/fileApi"
import {
  baselineSavedBuffer,
  changeSignalFor,
  diskFactKey,
  isBufferStale,
  saveLandsOn,
} from "@/lib/editorBuffers"
import type {
  ChangesSliceView,
  DiskState,
  TabBuffer,
} from "@/lib/editorBuffers"
import type { EditorRoot } from "@/lib/editorRoot"
import {
  encodingLabel,
  unmappableNotice,
  type SaveEncoding,
} from "@/lib/editorEncoding"
import { chip, prose } from "@/lib/prose"
import { saveResolutionOutcome } from "@/lib/editorTabs"
import { notifyError, notifySuccess, notifyWarning } from "@/lib/notify"
import { editorSetTabDirty } from "@/lib/store"
import type { SaveConflictTarget } from "@/components/SaveConflictDialog"

interface OpenTabRef {
  id: string
}

interface UseEditorSaveOptions {
  root: EditorRoot
  tabsRef: RefObject<readonly OpenTabRef[]>
  sliceRef: RefObject<ChangesSliceView | null>
  buffersRef: RefObject<Map<string, TabBuffer>>
  // Read the file again, for a save whose answer no longer describes the
  // buffer (see `saveLandsOn`). Through a ref: the reader is built after this.
  rereadRef: RefObject<((tabId: string, path: string) => void) | null>
  setBuffers: Dispatch<SetStateAction<Map<string, TabBuffer>>>
  raiseDiskBanner: (
    tabId: string,
    path: string,
    state: DiskState,
    fact: string,
  ) => void
}

// Owns the complete save transaction: in-flight state, successful rebasing,
// conflict questions, and cleanup shared by every resolution path.
export function useEditorSave({
  root,
  tabsRef,
  sliceRef,
  buffersRef,
  rereadRef,
  setBuffers,
  raiseDiskBanner,
}: UseEditorSaveOptions) {
  const [savingTabId, setSavingTabId] = useState<string | null>(null)
  const [savingPaths, setSavingPaths] = useState<Set<string>>(() => new Set())
  const [saveConflict, setSaveConflict] =
    useState<SaveConflictTarget | null>(null)

  // `encoding` is what the text is written in: the buffer's own for an
  // ordinary save, or the one the person picked to convert the file to.
  // `converting` says it was a conversion, which the success toast names.
  function writeBuffer(
    tabId: string,
    path: string,
    body: string,
    encoding: SaveEncoding,
    expected?: { modified: string | null; size: number | null },
    converting = false,
  ): void {
    // Which reading of the file this save is about: an answer arriving after
    // something else replaced the buffer's text must not relabel that text.
    const sentGeneration = buffersRef.current.get(tabId)?.generation ?? 0
    setSavingTabId(tabId)
    setSavingPaths((prev) => {
      const next = new Set(prev)
      next.add(path)
      return next
    })
    fileApi
      .write(root, path, body, expected, encoding)
      .then((result) => {
        const tabStillOpen = tabsRef.current.some((tab) => tab.id === tabId)
        const outcome = saveResolutionOutcome(path, tabStillOpen)
        const now = buffersRef.current.get(tabId)
        const lands = now === undefined || saveLandsOn(now, sentGeneration)
        if (tabStillOpen && !lands) {
          // The file was written, but the buffer now holds a different
          // reading of it; read the file again rather than misdescribe it.
          rereadRef.current?.(tabId, path)
        } else if (tabStillOpen) {
          setBuffers((prev) => {
            const current = prev.get(tabId)
            if (
              !current ||
              isBufferStale(current, path) ||
              !saveLandsOn(current, sentGeneration)
            )
              return prev
            const next = new Map(prev)
            next.set(
              tabId,
              baselineSavedBuffer(
                current,
                body,
                { modified: result.modified, size: result.size },
                changeSignalFor(sliceRef.current, path),
                encoding,
              ),
            )
            return next
          })
          editorSetTabDirty(root, tabId, false)
        }
        if (outcome.tone === "warning") notifyWarning(outcome.message)
        else if (converting)
          notifySuccess(
            prose`Saved ${chip(path)} as ${encodingLabel(encoding.encoding, encoding.bom)}.`,
          )
        else notifySuccess(outcome.message)
      })
      .catch((error) => {
        if (error instanceof FileConflictError) {
          raiseDiskBanner(
            tabId,
            path,
            error.deleted ? "deleted" : "changed",
            diskFactKey(
              error.deleted
                ? null
                : { modified: error.modified, size: error.size },
            ),
          )
          setSaveConflict({
            tabId,
            path,
            body,
            encoding,
            deleted: error.deleted,
          })
          return
        }
        if (error instanceof FileUnmappableError) {
          // Not sticky (weighed): nothing is lost, the edits stay in the
          // buffer under its unsaved dot, and the error window is long enough
          // to read where the character is.
          notifyError(unmappableNotice(path, error))
          return
        }
        notifyError(
          error instanceof Error ? error.message : "could not save file",
        )
      })
      .finally(() => {
        setSavingTabId((id) => (id === tabId ? null : id))
        setSavingPaths((prev) => {
          if (!prev.has(path)) return prev
          const next = new Set(prev)
          next.delete(path)
          return next
        })
      })
  }

  return {
    savingTabId,
    savingPaths,
    saveConflict,
    setSaveConflict,
    writeBuffer,
  }
}
