import { useState } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import {
  FileConflictError,
  FileUnmappableError,
  fileApi,
} from "@/lib/fileApi"
import {
  answerApplies,
  baselineSavedBuffer,
  bufferReadEncoding,
  changeSignalFor,
  diskFactKey,
  isBufferDirty,
  isBufferStale,
} from "@/lib/editorBuffers"
import type {
  ChangesSliceView,
  DiskState,
  TabBuffer,
} from "@/lib/editorBuffers"
import { rootKey, type EditorRoot } from "@/lib/editorRoot"
import { acquireLane, laneBusy, laneKey, laneOpWords } from "@/lib/editorLane"
import {
  encodingLabel,
  unmappableNotice,
  type SaveEncoding,
} from "@/lib/editorEncoding"
import { chip, prose } from "@/lib/prose"
import { saveResolutionOutcome } from "@/lib/editorTabs"
import {
  notifyError,
  notifyInfo,
  notifySuccess,
  notifyWarning,
} from "@/lib/notify"
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
  // False once the editor component that owns these buffers has unmounted.
  aliveRef: RefObject<boolean>
  // Read the file again, for a save whose answer no longer describes the
  // buffer (see `answerApplies`). Through a ref: the reader is built after
  // this.
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
  aliveRef,
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
  //
  // A save is an operation on the file's lane (lib/editorLane.ts): it does not
  // start while another operation on the file is out, and none starts while it
  // is. Its answer, success or failure, passes the one gate (`answerApplies`)
  // before it touches anything that is not a plain report of what happened:
  // the buffer, the tab's unsaved mark, the disk banner, the conflict dialog.
  function writeBuffer(
    tabId: string,
    path: string,
    body: string,
    encoding: SaveEncoding,
    expected?: { modified: string | null; size: number | null },
    converting = false,
  ): void {
    const lane = laneKey(rootKey(root), path)
    const release = acquireLane(lane, converting ? "convert" : "save")
    if (release === null) {
      const busy = laneBusy(lane)
      notifyInfo(
        prose`${chip(path)} is busy with ${busy ? laneOpWords(busy) : "another operation"}; nothing was saved. Try again when it finishes.`,
      )
      return
    }
    const from = buffersRef.current.get(tabId)
    const ticket = { path, generation: from?.generation ?? 0 }
    // The encoding the text being saved was read in: the file's, from the
    // reading this save is about.
    const readAs =
      from && !isBufferStale(from, path) ? bufferReadEncoding(from) : undefined
    const applies = () =>
      answerApplies(ticket, aliveRef.current, buffersRef.current.get(tabId))
    // Set when the file was written but the tab now holds another reading of
    // it: read it again once this save has let go of the file.
    let reread = false
    setSavingTabId(tabId)
    setSavingPaths((prev) => {
      const next = new Set(prev)
      next.add(path)
      return next
    })
    fileApi
      .write(root, path, body, expected, encoding, readAs)
      .then((result) => {
        const tabStillOpen = tabsRef.current.some((tab) => tab.id === tabId)
        const outcome = saveResolutionOutcome(path, tabStillOpen)
        if (applies()) {
          const now = buffersRef.current.get(tabId)!
          setBuffers((prev) => {
            const current = prev.get(tabId)
            if (!current || !answerApplies(ticket, true, current)) return prev
            const next = new Map(prev)
            next.set(
              tabId,
              baselineSavedBuffer(
                current,
                body,
                { modified: result.modified, size: result.size },
                changeSignalFor(sliceRef.current, path),
                encoding,
                result,
              ),
            )
            return next
          })
          // Unsaved is whatever the draft holds beyond what this save wrote,
          // text and mark, never a blanket "saved": typing while the save was
          // out is still only in the tab, and closing it must still ask.
          editorSetTabDirty(
            root,
            tabId,
            isBufferDirty(
              baselineSavedBuffer(
                now,
                body,
                { modified: result.modified, size: result.size },
                "",
                encoding,
                result,
              ),
            ),
          )
        } else if (aliveRef.current && tabStillOpen) {
          const now = buffersRef.current.get(tabId)
          reread = now !== undefined && !isBufferStale(now, path)
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
          // The banner and the dialog are offers about THIS reading of the
          // file (overwrite with this body, or take the disk's). Anywhere
          // else they would write text that is no longer what the tab holds.
          if (!applies()) {
            notifyError(error.message)
            return
          }
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
        release()
        setSavingTabId((id) => (id === tabId ? null : id))
        setSavingPaths((prev) => {
          if (!prev.has(path)) return prev
          const next = new Set(prev)
          next.delete(path)
          return next
        })
        if (reread && aliveRef.current) rereadRef.current?.(tabId, path)
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
