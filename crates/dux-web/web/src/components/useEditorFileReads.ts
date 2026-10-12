import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { fileApi } from "@/lib/fileApi"
import {
  changeSignalFor,
  diskFactKey,
  editorTextOf,
  encodingFromFile,
  freshGeneration,
  fileLoadSeedBuffer,
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
import type { EditorRoot } from "@/lib/editorRoot"
import type { EditorTab } from "@/lib/editorTabs"
import { isImagePreviewPath } from "@/lib/editorPreview"
import { encodingLabel } from "@/lib/editorEncoding"
import { notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"
import { editorSetTabDirty } from "@/lib/store"

interface UseEditorFileReadsOptions {
  root: EditorRoot
  tabs: readonly EditorTab[]
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  tabsRef: RefObject<readonly EditorTab[]>
  buffersRef: RefObject<Map<string, TabBuffer>>
  sliceRef: RefObject<ChangesSliceView | null>
  // The tab a save is in flight for, if any.
  savingTabIdRef: RefObject<string | null>
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
  savingTabIdRef,
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
  // if any, is asked for again.
  function reloadFileInPlace(
    tabId: string,
    path: string,
    asked = false,
    options: { encoding?: string; afterSave?: boolean } = {},
  ): void {
    // A save in flight for this tab answers for the text it sent; replacing
    // that text underneath it (with a reading the save's answer would then
    // relabel) is how a conversion double-encodes. A reload nobody asked for
    // waits: once the save lands, its stamp is the baseline the next check
    // compares against. The save's own re-read is the exception.
    const saving = () => !options.afterSave && savingTabIdRef.current === tabId
    if (!asked && saving()) return
    const token = nextToken(tabId)
    const signalAtRequest = changeSignalFor(sliceRef.current, path)
    const reopenAs = options.encoding ?? null
    const requested =
      reopenAs ?? buffersRef.current.get(tabId)?.encodingRequested ?? null
    fileApi
      .read(root, path, requested)
      .then((file) => {
        if (!isCurrentRequest(tabId, token)) return
        if (!asked && saving()) return
        const atResolve = buffersRef.current.get(tabId)
        const dirtyNow =
          !asked &&
          ((atResolve !== undefined &&
            !isBufferStale(atResolve, path) &&
            atResolve.draft !== atResolve.loaded) ||
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
          if (!current || isBufferStale(current, path)) return prev
          const next = new Map(prev)
          next.set(
            tabId,
            reloadedInPlace(current, path, file, signalAtRequest, requested),
          )
          return next
        })
        editorSetTabDirty(root, tabId, false)
        // Confirmed, because the only sign on screen may be the status bar's
        // label: a file that is all ASCII reads the same in most encodings.
        if (reopenAs !== null) {
          notifySuccess(
            file.unwritable
              ? prose`Reopened ${chip(path)} as ${encodingLabel(file.encoding ?? reopenAs, file.bom ?? false)}, read-only: the notice above the text says why.`
              : prose`Reopened ${chip(path)} as ${encodingLabel(file.encoding ?? reopenAs, file.bom ?? false)}. A save now writes it in that encoding.`,
          )
        }
      })
      .catch((error) => {
        if (!isCurrentRequest(tabId, token)) return
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
        notifyError(
          error instanceof Error
            ? `could not reload from disk: ${error.message}`
            : "could not reload from disk",
        )
      })
  }

  return { loadFileBuffer, reloadFileInPlace }
}
