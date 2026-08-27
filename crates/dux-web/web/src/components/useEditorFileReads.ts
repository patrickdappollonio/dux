import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { fileApi } from "@/lib/fileApi"
import {
  changeSignalFor,
  diskFactKey,
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
import { notifyError } from "@/lib/notify"
import { editorSetTabDirty } from "@/lib/store"

interface UseEditorFileReadsOptions {
  root: EditorRoot
  tabs: readonly EditorTab[]
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  tabsRef: RefObject<readonly EditorTab[]>
  buffersRef: RefObject<Map<string, TabBuffer>>
  sliceRef: RefObject<ChangesSliceView | null>
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
    setBuffers((prev) => {
      const next = new Map(prev)
      next.set(tabId, fileLoadSeedBuffer(path))
      return next
    })
    fileApi
      .read(root, path)
      .then((file) => {
        if (!isCurrentRequest(tabId, token)) return
        setBuffers((prev) => {
          const current = prev.get(tabId)
          if (!current || current.path !== path) return prev
          const next = new Map(prev)
          next.set(tabId, {
            ...current,
            loadedPath: path,
            loading: false,
            loaded: file.content,
            draft: file.content,
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

  function reloadFileInPlace(
    tabId: string,
    path: string,
    requested = false,
  ): void {
    const token = nextToken(tabId)
    const signalAtRequest = changeSignalFor(sliceRef.current, path)
    fileApi
      .read(root, path)
      .then((file) => {
        if (!isCurrentRequest(tabId, token)) return
        const atResolve = buffersRef.current.get(tabId)
        const dirtyNow =
          !requested &&
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
            reloadedInPlace(current, path, file, signalAtRequest),
          )
          return next
        })
        editorSetTabDirty(root, tabId, false)
      })
      .catch((error) => {
        if (!isCurrentRequest(tabId, token)) return
        notifyError(
          error instanceof Error
            ? `could not reload from disk: ${error.message}`
            : "could not reload from disk",
        )
      })
  }

  return { loadFileBuffer, reloadFileInPlace }
}
