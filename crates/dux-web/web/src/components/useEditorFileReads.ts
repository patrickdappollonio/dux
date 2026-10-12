import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { fileApi, type WorktreeFile } from "@/lib/fileApi"
import {
  answerApplies,
  changeSignalFor,
  diskFactKey,
  editorTextOf,
  encodingFromFile,
  freshGeneration,
  fileLoadSeedBuffer,
  isBufferDirty,
  isBufferStale,
  pruneByIds,
  reloadedInPlace,
  shouldSkipFileLoad,
} from "@/lib/editorBuffers"
import type {
  ChangesSliceView,
  DiskState,
  TabBuffer,
} from "@/lib/editorBuffers"
import { rootKey, type EditorRoot } from "@/lib/editorRoot"
import {
  acquireLane,
  laneBusy,
  laneKey,
  laneOpWords,
  type LaneOp,
} from "@/lib/editorLane"
import type { EditorTab } from "@/lib/editorTabs"
import { isImagePreviewPath } from "@/lib/editorPreview"
import { encodingLabel } from "@/lib/editorEncoding"
import { notifyError, notifyInfo, notifySuccess } from "@/lib/notify"
import { chip, prose, type Prose } from "@/lib/prose"
import { editorSetTabDirty } from "@/lib/store"

interface UseEditorFileReadsOptions {
  root: EditorRoot
  tabs: readonly EditorTab[]
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  tabsRef: RefObject<readonly EditorTab[]>
  buffersRef: RefObject<Map<string, TabBuffer>>
  sliceRef: RefObject<ChangesSliceView | null>
  // False once the editor component that owns these buffers has unmounted: an
  // answer arriving after that belongs to nobody (see `answerApplies`).
  aliveRef: RefObject<boolean>
  // The encoding chosen per open path, which outlives a buffer across a rename.
  encodingChoicesRef: RefObject<Map<string, string>>
  setBuffers: Dispatch<SetStateAction<Map<string, TabBuffer>>>
  raiseDiskBanner: (
    tabId: string,
    path: string,
    state: DiskState,
    fact: string,
  ) => void
}

// File loads and in-place reloads share one token per tab. A later request
// always invalidates an earlier response, whichever kind of read issued it.
export function useEditorFileReads({
  root,
  tabs,
  activeTab,
  activeBuffer,
  tabsRef,
  buffersRef,
  sliceRef,
  aliveRef,
  encodingChoicesRef,
  setBuffers,
  raiseDiskBanner,
}: UseEditorFileReadsOptions) {
  const requestTokenRef = useRef<Map<string, number>>(new Map())

  function nextToken(tabId: string): number {
    const token = (requestTokenRef.current.get(tabId) ?? 0) + 1
    requestTokenRef.current.set(tabId, token)
    return token
  }

  function isCurrentRequest(tabId: string, token: number): boolean {
    return requestTokenRef.current.get(tabId) === token
  }

  function loadFileBuffer(tabId: string, path: string): void {
    const token = nextToken(tabId)
    const signalAtRequest = changeSignalFor(sliceRef.current, path)
    // A retry of the same file keeps asking for the encoding the person chose,
    // and so does a file that was renamed or moved with a choice on it; any
    // other file starts from the automatic rules.
    const previous = buffersRef.current.get(tabId)
    const requested =
      (previous !== undefined && !isBufferStale(previous, path)
        ? previous.encodingRequested
        : null) ??
      encodingChoicesRef.current.get(path) ??
      null
    setBuffers((prev) => {
      const next = new Map(prev)
      next.set(tabId, fileLoadSeedBuffer(path))
      return next
    })
    fileApi
      .read(root, path, requested)
      .then((file) => {
        if (!isCurrentRequest(tabId, token)) return
        setBuffers((prev) => {
          const current = prev.get(tabId)
          if (!current || current.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...current,
            ...encodingFromFile(file, requested),
            loadedPath: path,
            loading: false,
            loaded: editorTextOf(file),
            generation: freshGeneration(),
            draft: editorTextOf(file),
            binary: file.binary,
            readOnly: file.read_only ?? false,
            fileError: null,
            fileLoadedSignal: signalAtRequest,
            stamp: {
              modified: file.modified ?? null,
              size: file.size ?? null,
            },
            diskState: "fresh",
          })
          return next
        })
      })
      .catch((error) => {
        if (!isCurrentRequest(tabId, token)) return
        setBuffers((prev) => {
          const current = prev.get(tabId)
          if (!current || current.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...current,
            loading: false,
            errorPath: path,
            fileError:
              error instanceof Error ? error.message : "could not open file",
          })
          return next
        })
      })
  }

  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    if (isImagePreviewPath(activeTab.path)) {
      const tabId = activeTab.id
      setBuffers((prev) => {
        if (!prev.has(tabId)) return prev
        const next = new Map(prev)
        next.delete(tabId)
        return next
      })
      return
    }
    if (shouldSkipFileLoad(activeBuffer, activeTab.path)) return
    loadFileBuffer(activeTab.id, activeTab.path)
    // The loader is intentionally recreated; tab/path/buffer identity alone
    // decides whether this effect issues a request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    activeTab?.mode,
    activeTab?.id,
    activeTab?.path,
    activeBuffer?.path,
    activeBuffer?.loadedPath,
    activeBuffer?.loading,
    activeBuffer?.errorPath,
  ])

  useEffect(() => {
    const liveIds = new Set(tabs.map((tab) => tab.id))
    requestTokenRef.current = pruneByIds(requestTokenRef.current, liveIds)
  }, [tabs])

  // `asked` is an explicit request (the banner's Reload, a confirm, a reopen),
  // which replaces the text even when it is dirty. `options.encoding` reopens
  // the file in that encoding; without it the buffer's own requested encoding,
  // if any, is asked for again. `options.afterSave` is a save's own re-read of
  // a file whose answer no longer described the buffer.
  //
  // Every reload is an operation on the file's lane (lib/editorLane.ts): none
  // starts while another (a save, a conversion, another reopen or reload) is
  // out. A reload nobody asked for just does not start; one somebody asked for
  // says the file is busy. Its answer passes the one gate (`answerApplies`):
  // this editor still mounted, the tab still holding the reading it was asked
  // from.
  function reloadFileInPlace(
    tabId: string,
    path: string,
    asked = false,
    options: { encoding?: string; afterSave?: boolean } = {},
  ): void {
    const reopenAs = options.encoding ?? null
    const op: LaneOp =
      reopenAs !== null ? "reopen" : asked || options.afterSave ? "reload" : "freshness"
    const lane = laneKey(rootKey(root), path)
    const release = acquireLane(lane, op)
    if (release === null) {
      if (asked) {
        const busy = laneBusy(lane)
        notifyInfo(
          prose`${chip(path)} is busy with ${busy ? laneOpWords(busy) : "another operation"}; try again when it finishes.`,
        )
      }
      return
    }
    const from = buffersRef.current.get(tabId)
    if (from === undefined || isBufferStale(from, path)) {
      release()
      return
    }
    const ticket = { path, generation: from.generation ?? 0 }
    const signalAtRequest = changeSignalFor(sliceRef.current, path)
    const requested = reopenAs ?? from.encodingRequested ?? null
    // A reopen replaces the text when it lands, so until then the editor takes
    // no typing (see `TabBuffer.reopening`); the lane is what keeps a second
    // reopen from starting meanwhile.
    const setReopening = (label: string | null) =>
      setBuffers((prev) => {
        const current = prev.get(tabId)
        if (!current || !answerApplies(ticket, true, current)) return prev
        if ((current.reopening ?? null) === label) return prev
        const next = new Map(prev)
        next.set(tabId, { ...current, reopening: label })
        return next
      })
    if (reopenAs !== null) setReopening(encodingLabel(reopenAs, false))
    const applies = () =>
      answerApplies(ticket, aliveRef.current, buffersRef.current.get(tabId))
    fileApi
      .read(root, path, requested)
      .then((file) => {
        if (!applies()) return
        const atResolve = buffersRef.current.get(tabId)!
        const dirtyNow =
          !asked &&
          (isBufferDirty(atResolve) ||
            (tabsRef.current.find((tab) => tab.id === tabId)?.dirty ?? false))
        if (dirtyNow) {
          raiseDiskBanner(
            tabId,
            path,
            "changed",
            diskFactKey({
              modified: file.modified ?? null,
              size: file.size ?? null,
            }),
          )
          return
        }
        setBuffers((prev) => {
          const current = prev.get(tabId)
          if (!current || !answerApplies(ticket, true, current)) return prev
          const next = new Map(prev)
          next.set(
            tabId,
            reloadedInPlace(current, path, file, signalAtRequest, requested),
          )
          return next
        })
        // A fresh reading is its own baseline: nothing in the tab is unsaved.
        editorSetTabDirty(root, tabId, false)
        // Confirmed, because the only sign on screen may be the status bar's
        // label: a file that is all ASCII reads the same in most encodings.
        if (reopenAs !== null) notifySuccess(reopenedNotice(path, reopenAs, file))
      })
      .catch((error) => {
        if (!applies()) return
        setReopening(null)
        if (reopenAs !== null) {
          // The buffer is untouched: the file stays open as it was, in the
          // encoding it had, and the toast says why the reopen did not happen.
          const reason =
            error instanceof Error ? error.message : "the read failed"
          notifyError(
            prose`Couldn't reopen ${chip(path)} as ${encodingLabel(reopenAs, false)}: ${reason}. It is still open as it was.`,
          )
          return
        }
        if (!asked) return
        notifyError(
          error instanceof Error
            ? `could not reload from disk: ${error.message}`
            : "could not reload from disk",
        )
      })
      .finally(release)
  }

  return { loadFileBuffer, reloadFileInPlace }
}

// What a reopen says it did, truthfully: a file read-only for where it lives
// stays so in any encoding, and one its new encoding cannot write back is
// read-only in it; only otherwise does a save now write in that encoding.
export function reopenedNotice(
  path: string,
  reopenAs: string,
  file: WorktreeFile,
): Prose {
  const label = encodingLabel(file.encoding ?? reopenAs, file.bom ?? false)
  if (file.path_read_only) {
    return prose`Reopened ${chip(path)} as ${label}. It stays read-only: it is a symlink or a .git path, which the editor never saves.`
  }
  if (file.unwritable) {
    return prose`Reopened ${chip(path)} as ${label}, read-only: the notice above the text says why.`
  }
  return prose`Reopened ${chip(path)} as ${label}. A save now writes it in that encoding.`
}
