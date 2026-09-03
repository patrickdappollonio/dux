import { useState } from "react"
import {
  filterChangedFiles,
  mergeChangedFilesRecaps,
  reconcileSelection,
  summarizeChangedFiles,
  type ChangedFileSelection,
  type ChangedFilesRecap,
} from "@/lib/changedFiles"
import { git } from "@/lib/git"
import { notifyError, notifySuccess, notifyWarning } from "@/lib/notify"
import type { ChangesSlice } from "@/lib/store"
import type { ChangedFileView } from "@/lib/types"

export type ChangesBulkVerb = "stage" | "unstage"
export type ChangesBusyAction = ChangesBulkVerb | "discard" | null

interface ScopedSearch {
  sessionId: string
  query: string
}

interface ScopedSelection extends ChangedFileSelection {
  sessionId: string
}

interface ChangedFilesModel {
  changed: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  filtered: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  recap: {
    staged: ChangedFilesRecap
    unstaged: ChangedFilesRecap
    all: ChangedFilesRecap
  }
  query: string
  filtering: boolean
  selected: ChangedFileSelection
  anySelected: boolean
  visibleStaged: string[]
  visibleUnstaged: string[]
  visibleCount: number
  allVisibleChecked: boolean
}

function fileCount(count: number): string {
  return `${count} file${count === 1 ? "" : "s"}`
}

function emptySelection(): ChangedFileSelection {
  return { staged: new Set(), unstaged: new Set() }
}

function changedFilesModel(
  selectedSessionId: string | null,
  changes: ChangesSlice,
  search: ScopedSearch,
  selection: ScopedSelection,
): ChangedFilesModel {
  const query = search.sessionId === selectedSessionId ? search.query : ""
  const slice = changes.sessionId === selectedSessionId ? changes : null
  const changed = {
    staged: slice?.staged ?? [],
    unstaged: slice?.unstaged ?? [],
  }
  const filtered = {
    staged: filterChangedFiles(changed.staged, query),
    unstaged: filterChangedFiles(changed.unstaged, query),
  }
  // The recap describes exactly the rows visible beneath it, so it is summed
  // over the FILTERED lists, matching the first number in the group badge's
  // "3 of 17". The header's figure is the two visible sets added together, not
  // an unfiltered total.
  const stagedRecap = summarizeChangedFiles(filtered.staged)
  const unstagedRecap = summarizeChangedFiles(filtered.unstaged)
  const selected = reconcileSelection(
    selection.sessionId === selectedSessionId ? selection : emptySelection(),
    changed,
  )
  const visibleStaged = filtered.staged.map((file) => file.path)
  const visibleUnstaged = filtered.unstaged.map((file) => file.path)
  const visibleCount = visibleStaged.length + visibleUnstaged.length
  const allVisibleChecked =
    visibleCount > 0 &&
    visibleStaged.every((path) => selected.staged.has(path)) &&
    visibleUnstaged.every((path) => selected.unstaged.has(path))

  return {
    changed,
    filtered,
    recap: {
      staged: stagedRecap,
      unstaged: unstagedRecap,
      all: mergeChangedFilesRecaps(stagedRecap, unstagedRecap),
    },
    query,
    filtering: query.trim() !== "",
    selected,
    anySelected: selected.staged.size > 0 || selected.unstaged.size > 0,
    visibleStaged,
    visibleUnstaged,
    visibleCount,
    allVisibleChecked,
  }
}

function bulkResultToast(
  verb: ChangesBulkVerb,
  result: { done: string[]; refused: string[] },
): void {
  const past = verb === "stage" ? "staged" : "unstaged"
  if (result.refused.length === 0) {
    notifySuccess(`${fileCount(result.done.length)} ${past}.`)
    return
  }
  notifyWarning(
    `${fileCount(result.done.length)} ${past}. ${fileCount(
      result.refused.length,
    )} had already left the list, starting with ${result.refused[0]}.`,
  )
}

function discardResultToast(result: {
  done: string[]
  failed: { path: string; message: string }[]
}): void {
  if (result.failed.length === 0) {
    notifySuccess(`Discarded the changes to ${fileCount(result.done.length)}.`)
    return
  }
  if (result.done.length === 0) {
    notifyError(
      `Nothing was discarded. ${result.failed[0]!.path}: ${result.failed[0]!.message}`,
    )
    return
  }
  notifyWarning(
    `Discarded the changes to ${fileCount(result.done.length)}. ${fileCount(
      result.failed.length,
    )} could not be discarded, starting with ${result.failed[0]!.path}: ${
      result.failed[0]!.message
    }`,
  )
}

interface BulkTransaction {
  verb: ChangesBulkVerb
  sessionId: string
  paths: string[]
  dropActed: (section: "staged" | "unstaged", paths: string[]) => void
}

async function runBulkTransaction({
  verb,
  sessionId,
  paths,
  dropActed,
}: BulkTransaction): Promise<void> {
  const section = verb === "stage" ? "unstaged" : "staged"
  try {
    const result =
      verb === "stage"
        ? await git.stageMany(sessionId, paths)
        : await git.unstageMany(sessionId, paths)
    dropActed(section, paths)
    bulkResultToast(verb, result)
  } catch (error) {
    notifyError(
      error instanceof Error ? error.message : `could not ${verb} the files`,
    )
  }
}

interface DiscardTransaction {
  sessionId: string
  paths: string[]
  dropActed: (section: "unstaged", paths: string[]) => void
}

async function runDiscardTransaction({
  sessionId,
  paths,
  dropActed,
}: DiscardTransaction): Promise<void> {
  const result = await git.discardMany(sessionId, paths)
  dropActed("unstaged", paths)
  discardResultToast(result)
}

export function useChangedFilesController(
  selectedSessionId: string | null,
  changes: ChangesSlice,
) {
  const [search, setSearch] = useState<ScopedSearch>({ sessionId: "", query: "" })
  const [selection, setSelection] = useState<ScopedSelection>({
    sessionId: "",
    ...emptySelection(),
  })
  const [busy, setBusy] = useState<ChangesBusyAction>(null)
  const [discarding, setDiscarding] = useState(false)
  const sessionId = selectedSessionId ?? ""
  const model = changedFilesModel(selectedSessionId, changes, search, selection)

  const editSelection = (
    mutate: (next: ChangedFileSelection) => void,
  ): void => {
    setSelection((previous) => {
      const base = previous.sessionId === sessionId ? previous : emptySelection()
      const next = {
        staged: new Set(base.staged),
        unstaged: new Set(base.unstaged),
      }
      mutate(next)
      return { sessionId, ...next }
    })
  }

  const dropActed = (
    section: "staged" | "unstaged",
    paths: string[],
  ): void => {
    editSelection((next) => {
      for (const path of paths) next[section].delete(path)
    })
  }

  function toggleOne(section: "staged" | "unstaged", path: string): void {
    editSelection((next) => {
      if (next[section].has(path)) next[section].delete(path)
      else next[section].add(path)
    })
  }

  function toggleVisible(): void {
    const wanted = !model.allVisibleChecked
    editSelection((next) => {
      for (const path of model.visibleStaged) {
        if (wanted) next.staged.add(path)
        else next.staged.delete(path)
      }
      for (const path of model.visibleUnstaged) {
        if (wanted) next.unstaged.add(path)
        else next.unstaged.delete(path)
      }
    })
  }

  async function runBulk(verb: ChangesBulkVerb): Promise<void> {
    const section = verb === "stage" ? "unstaged" : "staged"
    const paths = [...model.selected[section]]
    if (busy !== null || paths.length === 0) return
    setBusy(verb)
    try {
      await runBulkTransaction({ verb, sessionId, paths, dropActed })
    } finally {
      setBusy(null)
    }
  }

  async function runDiscardMany(paths: string[]): Promise<void> {
    setDiscarding(false)
    if (busy !== null || paths.length === 0) return
    setBusy("discard")
    try {
      await runDiscardTransaction({ sessionId, paths, dropActed })
    } finally {
      setBusy(null)
    }
  }

  return {
    ...model,
    busy,
    discarding,
    setQuery: (query: string) => setSearch({ sessionId, query }),
    toggleOne,
    toggleVisible,
    clearSelection: () => setSelection({ sessionId, ...emptySelection() }),
    openDiscard: () => setDiscarding(true),
    closeDiscard: () => setDiscarding(false),
    runBulk,
    runDiscardMany,
  }
}
