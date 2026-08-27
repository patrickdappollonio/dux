import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { FileApiError, fileApi } from "@/lib/fileApi"
import type { WorktreeEntryInfo } from "@/lib/fileInfo"
import {
  changeSignalFor,
  diskFactKey,
  fileSignalMoved,
  isBufferStale,
  stampFromInfo,
  stampsDiffer,
} from "@/lib/editorBuffers"
import type {
  ChangesSliceView,
  DiskState,
  TabBuffer,
} from "@/lib/editorBuffers"
import type { EditorRoot } from "@/lib/editorRoot"
import type { EditorTab } from "@/lib/editorTabs"
import type { MonacoInstance } from "@/components/CodeEditor"

type RaiseDiskBanner = (
  tabId: string,
  path: string,
  state: DiskState,
  fact: string,
) => void

interface EditorDiskBannerActions {
  raiseDiskBanner: RaiseDiskBanner
  dismissDiskBanner: (tabId: string, path: string) => void
}

interface UseEditorDiskFreshnessOptions {
  root: EditorRoot
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  openFileSignal: string
  slice: ChangesSliceView | null
  tabsRef: RefObject<readonly EditorTab[]>
  buffersRef: RefObject<Map<string, TabBuffer>>
  sliceRef: RefObject<ChangesSliceView | null>
  monacoRef: RefObject<MonacoInstance | null>
  setBuffers: Dispatch<SetStateAction<Map<string, TabBuffer>>>
  raiseDiskBanner: RaiseDiskBanner
  reloadFileInPlace: (tabId: string, path: string) => void
}

interface FreshnessRequest {
  tabId: string
  path: string
  signalAtRequest: string
}

export function createEditorDiskBannerActions(
  setBuffers: Dispatch<SetStateAction<Map<string, TabBuffer>>>,
): EditorDiskBannerActions {
  function raiseDiskBanner(
    tabId: string,
    path: string,
    state: DiskState,
    fact: string,
  ): void {
    setBuffers((previous) => {
      const buffer = previous.get(tabId)
      if (!buffer || isBufferStale(buffer, path)) return previous
      if (buffer.diskState === state && buffer.diskFact === fact) return previous
      const next = new Map(previous)
      next.set(tabId, { ...buffer, diskState: state, diskFact: fact })
      return next
    })
  }

  function dismissDiskBanner(tabId: string, path: string): void {
    setBuffers((previous) => {
      const buffer = previous.get(tabId)
      if (!buffer || isBufferStale(buffer, path)) return previous
      const next = new Map(previous)
      next.set(tabId, {
        ...buffer,
        diskState: "fresh",
        acknowledgedDisk: buffer.diskFact,
      })
      return next
    })
  }

  return { raiseDiskBanner, dismissDiskBanner }
}

function bufferCanBeChecked(
  buffer: TabBuffer | undefined,
  path: string,
): buffer is TabBuffer {
  if (!buffer || isBufferStale(buffer, path)) return false
  if (buffer.loadedPath !== path) return false
  return buffer.stamp.modified !== null || buffer.stamp.size !== null
}

function editorSelectionActive(
  monacoRef: RefObject<MonacoInstance | null>,
): boolean {
  const monaco = monacoRef.current
  if (!monaco) return false
  try {
    return monaco.editor
      .getEditors()
      .some((editor) => editor.getSelection()?.isEmpty() === false)
  } catch {
    return false
  }
}

export function useEditorDiskFreshness({
  root,
  activeTab,
  activeBuffer,
  openFileSignal,
  slice,
  tabsRef,
  buffersRef,
  sliceRef,
  monacoRef,
  setBuffers,
  raiseDiskBanner,
  reloadFileInPlace,
}: UseEditorDiskFreshnessOptions): void {
  const inFlightRef = useRef<Map<string, string>>(new Map())

  function adoptFreshnessSignal(
    request: FreshnessRequest,
    agrees: boolean,
  ): void {
    setBuffers((previous) => {
      const buffer = previous.get(request.tabId)
      if (!buffer || isBufferStale(buffer, request.path)) return previous
      const clearBanner = agrees && buffer.diskState !== "fresh"
      if (
        buffer.fileLoadedSignal === request.signalAtRequest &&
        !clearBanner
      ) {
        return previous
      }
      const next = new Map(previous)
      next.set(request.tabId, {
        ...buffer,
        fileLoadedSignal: request.signalAtRequest,
        ...(clearBanner
          ? { diskState: "fresh" as const, diskFact: null }
          : {}),
      })
      return next
    })
  }

  function handleFreshnessInfo(
    info: WorktreeEntryInfo,
    request: FreshnessRequest,
  ): void {
    const buffer = buffersRef.current.get(request.tabId)
    if (!bufferCanBeChecked(buffer, request.path)) return

    const onDisk = stampFromInfo(info)
    const fact = diskFactKey(onDisk)
    const agrees = !stampsDiffer(buffer.stamp, onDisk)
    if (agrees || buffer.acknowledgedDisk === fact) {
      adoptFreshnessSignal(request, agrees)
      return
    }

    const dirty =
      tabsRef.current.find((tab) => tab.id === request.tabId)?.dirty ?? false
    if (dirty) {
      raiseDiskBanner(request.tabId, request.path, "changed", fact)
      return
    }
    if (editorSelectionActive(monacoRef)) {
      raiseDiskBanner(request.tabId, request.path, "paused", fact)
      return
    }
    reloadFileInPlace(request.tabId, request.path)
  }

  function handleFreshnessError(
    error: unknown,
    request: FreshnessRequest,
  ): void {
    if (!(error instanceof FileApiError) || error.status !== 404) return
    const fact = diskFactKey(null)
    const buffer = buffersRef.current.get(request.tabId)
    if (buffer?.acknowledgedDisk === fact) return
    raiseDiskBanner(request.tabId, request.path, "deleted", fact)
  }

  function checkDiskFreshness(tabId: string, path: string): void {
    if (!bufferCanBeChecked(buffersRef.current.get(tabId), path)) return
    if (inFlightRef.current.get(tabId) === path) return
    inFlightRef.current.set(tabId, path)

    const request: FreshnessRequest = {
      tabId,
      path,
      signalAtRequest: changeSignalFor(sliceRef.current, path),
    }
    fileApi
      .info(root, path)
      .then((info) => handleFreshnessInfo(info, request))
      .catch((error) => handleFreshnessError(error, request))
      .finally(() => {
        if (inFlightRef.current.get(tabId) === path) {
          inFlightRef.current.delete(tabId)
        }
      })
  }

  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    if (!fileSignalMoved(activeBuffer, activeTab.path, slice)) return
    checkDiskFreshness(activeTab.id, activeTab.path)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    activeTab?.mode,
    activeTab?.id,
    activeTab?.path,
    openFileSignal,
    slice?.phase,
    activeBuffer?.fileLoadedSignal,
    activeBuffer?.loadedPath,
  ])

  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    const tabId = activeTab.id
    const path = activeTab.path
    const revalidate = () => checkDiskFreshness(tabId, path)
    window.addEventListener("focus", revalidate)
    return () => window.removeEventListener("focus", revalidate)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab?.mode, activeTab?.id, activeTab?.path])

  useEffect(() => {
    if (!activeTab || activeTab.mode !== "file") return
    checkDiskFreshness(activeTab.id, activeTab.path)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab?.mode, activeTab?.id, activeTab?.path])
}
