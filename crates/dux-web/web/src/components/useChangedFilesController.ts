import { useCallback, useMemo, useState } from "react"
import {
  filterChangedFiles,
  mergeChangedFilesRecaps,
  reconcileSelection,
  summarizeChangedFiles,
  type ChangedFileSelection,
  type ChangedFilesRecap,
} from "@/lib/changedFiles"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { git } from "@/lib/git"
import { notifyError, notifySuccess, notifyWarning } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"
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

function emptySelection(): ChangedFileSelection {
  return { staged: new Set(), unstaged: new Set() }
}

// Never mutated: every edit copies the sets first (see `editSelection`).
const EMPTY_SELECTION: ChangedFileSelection = emptySelection()
const EMPTY_FILES: ChangedFileView[] = []

// The pane's derived model, every piece memoized on exactly what it reads. The
// pane re-renders for reasons that move none of it (a busy flag, the discard
// dialog opening), and at tens of thousands of files a filter, a recap sum or a
// Set rebuild per render is what makes the tab stutter.
function useChangedFilesModel(
  selectedSessionId: string | null,
  changes: ChangesSlice,
  search: ScopedSearch,
  selection: ScopedSelection,
): ChangedFilesModel {
  const query = search.sessionId === selectedSessionId ? search.query : ""
  const slice = changes.sessionId === selectedSessionId ? changes : null
  const stagedSource = slice?.staged ?? EMPTY_FILES
  const unstagedSource = slice?.unstaged ?? EMPTY_FILES
  const changed = useMemo(
    () => ({ staged: stagedSource, unstaged: unstagedSource }),
    [stagedSource, unstagedSource],
  )
  const filtered = useMemo(
    () => ({
      staged: filterChangedFiles(stagedSource, query),
      unstaged: filterChangedFiles(unstagedSource, query),
    }),
    [stagedSource, unstagedSource, query],
  )
  // The recap describes exactly the rows visible beneath it, so it is summed over
  // the filtered lists and the header's figure is the visible sets added
  // together, never an unfiltered total.
  const recap = useMemo(() => {
    const staged = summarizeChangedFiles(filtered.staged)
    const unstaged = summarizeChangedFiles(filtered.unstaged)
    return { staged, unstaged, all: mergeChangedFilesRecaps(staged, unstaged) }
  }, [filtered])
  const scopedSelection: ChangedFileSelection =
    selection.sessionId === selectedSessionId ? selection : EMPTY_SELECTION
  const selected = useMemo(
    () => reconcileSelection(scopedSelection, changed),
    [scopedSelection, changed],
  )
  const visible = useMemo(() => {
    const visibleStaged = filtered.staged.map((file) => file.path)
    const visibleUnstaged = filtered.unstaged.map((file) => file.path)
    const visibleCount = visibleStaged.length + visibleUnstaged.length
    const allVisibleChecked =
      visibleCount > 0 &&
      visibleStaged.every((path) => selected.staged.has(path)) &&
      visibleUnstaged.every((path) => selected.unstaged.has(path))
    return { visibleStaged, visibleUnstaged, visibleCount, allVisibleChecked }
  }, [filtered, selected])

  return {
    changed,
    filtered,
    recap,
    query,
    filtering: query.trim() !== "",
    selected,
    anySelected: selected.staged.size > 0 || selected.unstaged.size > 0,
    ...visible,
  }
}

function bulkResultToast(
  verb: ChangesBulkVerb,
  result: { done: string[]; refused: string[] },
): void {
  const past = verb === "stage" ? "staged" : "unstaged"
  // A clean stage or unstage says nothing: the rows moving between the pane's
  // sections is the whole feedback. A refusal still speaks, because rows that
  // did not move are the ones there is nothing on screen to explain.
  if (result.refused.length === 0) return
  notifyWarning(
    `${formatRegularCount(result.done.length, "file")} ${past}. ${formatRegularCount(
      result.refused.length,
      "file",
    )} had already left the list, starting with ${result.refused[0]}.`,
  )
}

function discardResultToast(result: {
  done: string[]
  failed: { path: string; message: string }[]
}): void {
  if (result.failed.length === 0) {
    notifySuccess(
      `Discarded the changes to ${formatRegularCount(result.done.length, "file")}.`,
    )
    return
  }
  if (result.done.length === 0) {
    notifyError(
      prose`Nothing was discarded. ${chip(result.failed[0]!.path)}: ${result.failed[0]!.message}`,
    )
    return
  }
  notifyWarning(
    `Discarded the changes to ${formatRegularCount(result.done.length, "file")}. ${formatRegularCount(
      result.failed.length,
      "file",
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
  const model = useChangedFilesModel(selectedSessionId, changes, search, selection)

  const editSelection = useCallback(
    (mutate: (next: ChangedFileSelection) => void): void => {
      setSelection((previous) => {
        const base = previous.sessionId === sessionId ? previous : emptySelection()
        const next = {
          staged: new Set(base.staged),
          unstaged: new Set(base.unstaged),
        }
        mutate(next)
        return { sessionId, ...next }
      })
    },
    [sessionId],
  )

  const dropActed = (
    section: "staged" | "unstaged",
    paths: string[],
  ): void => {
    editSelection((next) => {
      for (const path of paths) next[section].delete(path)
    })
  }

  // Stable per session, because every row holds it and the rows are memoized:
  // a fresh function per render would re-render every mounted row with it.
  const toggleOne = useCallback(
    (section: "staged" | "unstaged", path: string): void => {
      editSelection((next) => {
        if (next[section].has(path)) next[section].delete(path)
        else next[section].add(path)
      })
    },
    [editSelection],
  )

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
