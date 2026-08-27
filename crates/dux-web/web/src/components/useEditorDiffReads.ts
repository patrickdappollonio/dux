import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { fileApi } from "@/lib/fileApi"
import type { FileDiffContents } from "@/lib/fileApi"
import { emptyBuffer, isBufferStale, pruneByIds } from "@/lib/editorBuffers"
import type { TabBuffer } from "@/lib/editorBuffers"
import type { EditorRoot } from "@/lib/editorRoot"
import type { EditorTab } from "@/lib/editorTabs"

interface UseEditorDiffReadsOptions {
  root: EditorRoot
  tabs: readonly EditorTab[]
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  tabsRef: RefObject<readonly EditorTab[]>
  loadedSignalRef: RefObject<string>
  setBuffers: Dispatch<SetStateAction<Map<string, TabBuffer>>>
}

interface DiffRequest {
  tabId: string
  path: string
  token: number
}

function diffResultBase(
  previous: Map<string, TabBuffer>,
  tabs: readonly EditorTab[],
  request: DiffRequest,
): TabBuffer | null {
  const current = previous.get(request.tabId) ?? emptyBuffer(request.path)
  if (current.path === request.path) return current
  const currentPath = tabs.find((tab) => tab.id === request.tabId)?.path
  return currentPath === request.path ? emptyBuffer(request.path) : null
}

function withDiffResult(
  previous: Map<string, TabBuffer>,
  tabs: readonly EditorTab[],
  request: DiffRequest,
  diff: FileDiffContents,
  loadedSignal: string,
): Map<string, TabBuffer> {
  const base = diffResultBase(previous, tabs, request)
  if (base === null) return previous
  const next = new Map(previous)
  next.set(request.tabId, {
    ...base,
    diff,
    diffLoadedPath: request.path,
    diffLoadedSignal: loadedSignal,
    diffError: null,
  })
  return next
}

function withDiffError(
  previous: Map<string, TabBuffer>,
  tabs: readonly EditorTab[],
  request: DiffRequest,
  error: unknown,
): Map<string, TabBuffer> {
  const base = diffResultBase(previous, tabs, request)
  if (base === null) return previous
  const next = new Map(previous)
  next.set(request.tabId, {
    ...base,
    diffError: error instanceof Error ? error.message : "could not load diff",
  })
  return next
}

export function useEditorDiffReads({
  root,
  tabs,
  activeTab,
  activeBuffer,
  tabsRef,
  loadedSignalRef,
  setBuffers,
}: UseEditorDiffReadsOptions) {
  const requestTokenRef = useRef<Map<string, number>>(new Map())

  function nextRequest(tabId: string, path: string): DiffRequest {
    const token = (requestTokenRef.current.get(tabId) ?? 0) + 1
    requestTokenRef.current.set(tabId, token)
    return { tabId, path, token }
  }

  function isCurrentRequest(request: DiffRequest): boolean {
    return requestTokenRef.current.get(request.tabId) === request.token
  }

  function loadDiffBuffer(tabId: string, path: string): void {
    const request = nextRequest(tabId, path)
    fileApi
      .diff(root, path)
      .then((diff) => {
        if (!isCurrentRequest(request)) return
        setBuffers((previous) =>
          withDiffResult(
            previous,
            tabsRef.current,
            request,
            diff,
            loadedSignalRef.current,
          ),
        )
      })
      .catch((error) => {
        if (!isCurrentRequest(request)) return
        setBuffers((previous) =>
          withDiffError(previous, tabsRef.current, request, error),
        )
      })
  }

  useEffect(() => {
    if (!activeTab || activeTab.mode !== "diff") return
    if (
      activeBuffer &&
      !isBufferStale(activeBuffer, activeTab.path) &&
      activeBuffer.diffLoadedPath === activeTab.path
    ) {
      return
    }
    loadDiffBuffer(activeTab.id, activeTab.path)
    // The active tab and buffer identity fully determine whether to request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    activeTab?.mode,
    activeTab?.id,
    activeTab?.path,
    activeBuffer?.path,
    activeBuffer?.diffLoadedPath,
  ])

  useEffect(() => {
    const liveIds = new Set(tabs.map((tab) => tab.id))
    requestTokenRef.current = pruneByIds(requestTokenRef.current, liveIds)
  }, [tabs])

  function refreshDiff(): void {
    if (!activeTab) return
    const tabId = activeTab.id
    setBuffers((previous) => {
      const current = previous.get(tabId)
      if (!current) return previous
      const next = new Map(previous)
      next.set(tabId, { ...current, diffLoadedPath: null })
      return next
    })
  }

  return { loadDiffBuffer, refreshDiff }
}
