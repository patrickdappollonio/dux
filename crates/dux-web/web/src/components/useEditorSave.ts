import { useState } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { FileConflictError, fileApi } from "@/lib/fileApi"
import {
  baselineSavedBuffer,
  changeSignalFor,
  diskFactKey,
  isBufferStale,
} from "@/lib/editorBuffers"
import type {
  ChangesSliceView,
  DiskState,
  TabBuffer,
} from "@/lib/editorBuffers"
import type { EditorRoot } from "@/lib/editorRoot"
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
  setBuffers,
  raiseDiskBanner,
}: UseEditorSaveOptions) {
  const [savingTabId, setSavingTabId] = useState<string | null>(null)
  const [savingPaths, setSavingPaths] = useState<Set<string>>(() => new Set())
  const [saveConflict, setSaveConflict] =
    useState<SaveConflictTarget | null>(null)

  function writeBuffer(
    tabId: string,
    path: string,
    body: string,
    expected?: { modified: string | null; size: number | null },
  ): void {
    setSavingTabId(tabId)
    setSavingPaths((prev) => {
      const next = new Set(prev)
      next.add(path)
      return next
    })
    fileApi
      .write(root, path, body, expected)
      .then((result) => {
        const tabStillOpen = tabsRef.current.some((tab) => tab.id === tabId)
        const outcome = saveResolutionOutcome(path, tabStillOpen)
        if (tabStillOpen) {
          setBuffers((prev) => {
            const current = prev.get(tabId)
            if (!current || isBufferStale(current, path)) return prev
            const next = new Map(prev)
            next.set(
              tabId,
              baselineSavedBuffer(
                current,
                body,
                { modified: result.modified, size: result.size },
                changeSignalFor(sliceRef.current, path),
              ),
            )
            return next
          })
          editorSetTabDirty(root, tabId, false)
        }
        if (outcome.tone === "warning") notifyWarning(outcome.message)
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
            deleted: error.deleted,
          })
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
