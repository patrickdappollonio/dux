import { useEffect, useRef } from "react"
import type { Dispatch, RefObject, SetStateAction } from "react"
import { fileApi } from "@/lib/fileApi"
import { isDiffHeadAnswer } from "@/lib/fileApi"
import type { FileDiffAnswer } from "@/lib/fileApi"
import { emptyBuffer, isBufferStale, pruneByIds } from "@/lib/editorBuffers"
import type { TabBuffer } from "@/lib/editorBuffers"
import type { EditorRoot } from "@/lib/editorRoot"
import type { EditorTab } from "@/lib/editorTabs"
import { encodingLabel } from "@/lib/editorEncoding"
import { notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"

interface UseEditorDiffReadsOptions {
  root: EditorRoot
  tabs: readonly EditorTab[]
  activeTab: EditorTab | null
  activeBuffer: TabBuffer | undefined
  tabsRef: RefObject<readonly EditorTab[]>
  buffersRef: RefObject<Map<string, TabBuffer>>
  // The encoding chosen for each open path when no buffer carries it yet (an
  // address that names one, or a rename), the same map the file reads use.
  encodingChoicesRef: RefObject<Map<string, string>>
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
  answer: FileDiffAnswer,
  loadedSignal: string,
): Map<string, TabBuffer> {
  const base = diffResultBase(previous, tabs, request)
  if (base === null) return previous
  const next = new Map(previous)
  // One shape or the other, never both: a head answer leaves no sides behind
  // for the diff editor to render from a previous read of the same tab.
  const head = isDiffHeadAnswer(answer)
  next.set(request.tabId, {
    ...base,
    diff: head ? null : answer,
    diffHead: head ? answer.head : null,
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
  buffersRef,
  encodingChoicesRef,
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
    // The encoding the person reopened this file in, so the diff shows the
    // same characters the editor tab does.
    const buffer = buffersRef.current.get(tabId)
    const encoding =
      (buffer !== undefined && !isBufferStale(buffer, path)
        ? buffer.encodingRequested
        : null) ??
      encodingChoicesRef.current.get(path) ??
      null
    fileApi
      .diff(root, path, encoding)
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

  // Read the diff again in exactly `encoding`, the large-file view's way to
  // correct how the patch's lines were decoded. On success the buffer asks for
  // it from then on; a refusal (bytes not valid in it) leaves the diff on
  // screen as it was and says why, the way a refused reopen of a file does.
  function reloadDiffAs(tabId: string, path: string, encoding: string): void {
    const request = nextRequest(tabId, path)
    fileApi
      .diff(root, path, encoding)
      .then((diff) => {
        if (!isCurrentRequest(request)) return
        setBuffers((previous) => {
          const next = withDiffResult(
            previous,
            tabsRef.current,
            request,
            diff,
            loadedSignalRef.current,
          )
          const buffer = next.get(tabId)
          if (next === previous || buffer === undefined) return next
          next.set(tabId, { ...buffer, encodingRequested: encoding })
          return next
        })
        notifySuccess(
          prose`Reread the diff of ${chip(path)} as ${encodingLabel(encoding, false)}.`,
        )
      })
      .catch((error) => {
        if (!isCurrentRequest(request)) return
        const reason = error instanceof Error ? error.message : "the read failed"
        notifyError(
          prose`Couldn't read the diff of ${chip(path)} as ${encodingLabel(encoding, false)}: ${reason}. It is still shown as it was.`,
        )
      })
  }

  return { loadDiffBuffer, refreshDiff, reloadDiffAs }
}
