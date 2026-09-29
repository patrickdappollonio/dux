import { useCallback, useMemo, useState } from "react"
import {
  filterChangedFiles,
  mergeChangedFilesRecaps,
  reconcileSelection,
  summarizeChangedFiles,
  uncoveredPaths,
  type ChangedFileSelection,
  type ChangedFilesRecap,
} from "@/lib/changedFiles"
import { NO_EXPANSIONS, loadedChildren, type Expansions } from "@/lib/changesTree"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { git, type BatchResult, type DiscardConfirmation } from "@/lib/git"
import { notifyError, notifyInfo, notifySuccess, notifyWarning } from "@/lib/notify"
import { chip, joinProse, prose, type Prose } from "@/lib/prose"
import {
  bulkLeftOutNotice,
  discardLeftOutReason,
  discardOutcome,
  leftOutNotice,
  selectedFileCount,
  stageLeftOutReason,
} from "@/lib/discardOutcome"
import { discardActsOn, stageActsOn } from "@/lib/changedFiles"
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
  // Every row a user can select and act on: the listing's own rows and the
  // rows loaded under expanded folders.
  actionable: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
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
  expansions: Expansions,
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
  // A row loaded under an expanded folder is as real as a row of the listing:
  // it can be checked, counted and acted on, so the selection survives against
  // both.
  const actionable = useMemo(
    () =>
      expansions.size === 0
        ? changed
        : {
            staged: [
              ...changed.staged,
              ...loadedChildren(expansions, "staged", changed.staged),
            ],
            unstaged: [
              ...changed.unstaged,
              ...loadedChildren(expansions, "unstaged", changed.unstaged),
            ],
          },
    [changed, expansions],
  )
  const scopedSelection: ChangedFileSelection =
    selection.sessionId === selectedSessionId ? selection : EMPTY_SELECTION
  const selected = useMemo(
    () => reconcileSelection(scopedSelection, actionable),
    [scopedSelection, actionable],
  )
  // Every row shown: the filtered listing, and the rows under the expanded
  // folders among it, which "Select all" covers as much as the rest.
  const visible = useMemo(() => {
    const shown = (section: "staged" | "unstaged") => [
      ...filtered[section].map((file) => file.path),
      ...loadedChildren(expansions, section, filtered[section]).map((file) => file.path),
    ]
    const visibleStaged = shown("staged")
    const visibleUnstaged = shown("unstaged")
    const visibleCount = visibleStaged.length + visibleUnstaged.length
    const allVisibleChecked =
      visibleCount > 0 &&
      visibleStaged.every((path) => selected.staged.has(path)) &&
      visibleUnstaged.every((path) => selected.unstaged.has(path))
    return { visibleStaged, visibleUnstaged, visibleCount, allVisibleChecked }
  }, [filtered, selected, expansions])

  return {
    changed,
    actionable,
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
  result: BatchResult,
  rows: readonly ChangedFileView[],
): void {
  const past = verb === "stage" ? "staged" : "unstaged"
  // Repositories inside a staged folder are left out on purpose, which only
  // a message can say.
  const notice = verb === "stage" ? leftOutNotice(result) : null
  if (notice) notifyInfo(notice)
  // A clean stage or unstage says nothing: the rows moving between the pane's
  // sections is the whole feedback. A refusal still speaks, because rows that
  // did not move are the ones there is nothing on screen to explain. The count
  // is of files, so a folded folder counts what is inside it.
  if (result.refused.length === 0) return
  const done = selectedFileCount(new Set(result.done), rows)
  const reasons = result.reasons ?? {}
  const gone = result.refused.filter((path) => reasons[path] === undefined)
  const explained = result.refused.flatMap((path) => reasons[path] ?? [])
  const parts: Prose[] = [prose`${formatRegularCount(done, "file")} ${past}.`]
  if (gone.length > 0) {
    parts.push(
      prose`${formatRegularCount(gone.length, "file")} had already left the list, starting with ${chip(gone[0]!)}.`,
    )
  }
  // The server's own sentence says why it refused a path, relayed as it came;
  // the first one is spelled out and the rest are counted.
  if (explained.length > 0) {
    parts.push([explained[0]!])
    if (explained.length > 1) {
      const more = explained.length - 1
      parts.push(prose`${more} more ${more === 1 ? "row was" : "rows were"} refused too.`)
    }
  }
  notifyWarning(joinProse(parts, " "))
}

function discardResultToast(
  result: {
    done: string[]
    // The server's count of what each done folder actually took.
    deleted?: Record<string, number>
    failed: { path: string; message: string }[]
  },
  rows: readonly ChangedFileView[],
): void {
  // The rows discarded, as they were listed: a path the list no longer has is
  // worded as a plain file.
  const byPath = new Map(rows.map((row) => [row.path, row]))
  const doneRows = result.done.map(
    (path) =>
      byPath.get(path) ?? {
        path,
        status: "M",
        additions: 0,
        deletions: 0,
        binary: false,
        diff_excluded: false,
      },
  )
  if (result.failed.length === 0) {
    notifySuccess(discardOutcome(doneRows, result.deleted))
    return
  }
  if (result.done.length === 0) {
    notifyError(
      prose`Nothing was discarded. ${chip(result.failed[0]!.path)}: ${result.failed[0]!.message}`,
    )
    return
  }
  notifyWarning(
    prose`${discardOutcome(doneRows, result.deleted)} ${formatRegularCount(
      result.failed.length,
      "row",
    )} could not be discarded, starting with ${chip(result.failed[0]!.path)}: ${
      result.failed[0]!.message
    }`,
  )
}

interface BulkTransaction {
  verb: ChangesBulkVerb
  sessionId: string
  paths: string[]
  rows: readonly ChangedFileView[]
  dropActed: (section: "staged" | "unstaged", paths: string[]) => void
}

async function runBulkTransaction({
  verb,
  sessionId,
  paths,
  rows,
  dropActed,
}: BulkTransaction): Promise<void> {
  const section = verb === "stage" ? "unstaged" : "staged"
  try {
    const result =
      verb === "stage"
        ? await git.stageMany(sessionId, paths)
        : await git.unstageMany(sessionId, paths)
    dropActed(section, paths)
    bulkResultToast(verb, result, rows)
  } catch (error) {
    notifyError(
      error instanceof Error ? error.message : `could not ${verb} the files`,
    )
  }
}

interface DiscardTransaction {
  sessionId: string
  paths: string[]
  // What each folder was when the dialog opened, which is what was confirmed.
  confirmations: Readonly<Record<string, DiscardConfirmation>>
  rows: readonly ChangedFileView[]
  dropActed: (section: "unstaged", paths: string[]) => void
}

async function runDiscardTransaction({
  sessionId,
  paths,
  confirmations,
  rows,
  dropActed,
}: DiscardTransaction): Promise<void> {
  // What the user confirmed for each folder travels with its request.
  const result = await git.discardMany(sessionId, paths, confirmations)
  dropActed("unstaged", paths)
  discardResultToast(result, rows)
}

// Why each selected row a bulk action will leave out is left out, in the
// order the list shows them.
function leftOutReasons(
  selected: ReadonlySet<string>,
  rows: readonly ChangedFileView[],
  reason: (file: ChangedFileView) => Prose | null,
): Prose[] {
  if (selected.size === 0) return []
  return rows.flatMap((file) => {
    if (!selected.has(file.path)) return []
    const why = reason(file)
    return why === null ? [] : [why]
  })
}

export function useChangedFilesController(
  selectedSessionId: string | null,
  changes: ChangesSlice,
  expansions: Expansions = NO_EXPANSIONS,
) {
  const [search, setSearch] = useState<ScopedSearch>({ sessionId: "", query: "" })
  const [selection, setSelection] = useState<ScopedSelection>({
    sessionId: "",
    ...emptySelection(),
  })
  const [busy, setBusy] = useState<ChangesBusyAction>(null)
  const [discarding, setDiscarding] = useState(false)
  const sessionId = selectedSessionId ?? ""
  const model = useChangedFilesModel(
    selectedSessionId,
    changes,
    search,
    selection,
    expansions,
  )

  // A check belongs to a row on screen. When a loaded listing no longer holds a
  // checked path (it was staged elsewhere, its folder lost it), the check is
  // forgotten for good rather than only hidden, so the row does not come back
  // checked if it returns. Only against a loaded listing for this agent, so a
  // listing still loading clears nothing. Adjusted during render, React's
  // pattern for state derived from what was rendered: it runs again at once
  // with the pruned selection, and settles because that one needs no pruning.
  const listingLoaded =
    changes.sessionId === selectedSessionId && changes.phase === "loaded"
  if (
    listingLoaded &&
    selectedSessionId &&
    selection.sessionId === selectedSessionId &&
    model.selected !== selection
  ) {
    setSelection({
      sessionId: selectedSessionId,
      staged: model.selected.staged,
      unstaged: model.selected.unstaged,
    })
  }

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

  // Collapsing a folder hides what is under it, so it unchecks it too: a
  // checked row nobody can see must not be counted or acted on.
  const uncheckUnder = useCallback(
    (section: "staged" | "unstaged", folder: string): void => {
      const inside = `${folder}/`
      editSelection((next) => {
        for (const path of [...next[section]]) {
          if (path.startsWith(inside)) next[section].delete(path)
        }
      })
    },
    [editSelection],
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
    // A worktree of this repository is never staged (the server refuses it),
    // so a bulk stage leaves it out rather than failing the whole batch.
    const { actionable } = model
    const unstageable =
      verb === "stage"
        ? new Set(actionable.unstaged.filter((f) => !stageActsOn(f)).map((f) => f.path))
        : new Set<string>()
    // A checked row inside a checked folder is already part of it.
    const paths = uncoveredPaths(model.selected[section]).filter(
      (path) => !unstageable.has(path),
    )
    if (busy !== null || paths.length === 0) return
    const leftOut =
      verb === "stage"
        ? leftOutReasons(model.selected.unstaged, actionable.unstaged, stageLeftOutReason)
        : []
    setBusy(verb)
    try {
      await runBulkTransaction({
        verb,
        sessionId,
        paths,
        rows: verb === "stage" ? actionable.unstaged : actionable.staged,
        dropActed,
      })
      const notice = bulkLeftOutNotice(leftOut)
      if (notice) notifyInfo(notice)
    } finally {
      setBusy(null)
    }
  }

  async function runDiscardMany(
    paths: string[],
    confirmations: Readonly<Record<string, DiscardConfirmation>>,
  ): Promise<void> {
    setDiscarding(false)
    if (busy !== null || paths.length === 0) return
    const leftOut = leftOutReasons(
      model.selected.unstaged,
      model.actionable.unstaged,
      discardLeftOutReason,
    )
    setBusy("discard")
    try {
      await runDiscardTransaction({
        sessionId,
        paths,
        confirmations,
        rows: model.actionable.unstaged,
        dropActed,
      })
      const notice = bulkLeftOutNotice(leftOut)
      if (notice) notifyInfo(notice)
    } finally {
      setBusy(null)
    }
  }

  return {
    ...model,
    // What the bulk bar counts: files, so a checked folder counts what is
    // inside it, the same count its dialog and its toast use.
    // A checked row inside a checked folder is counted once, in the folder.
    selectedCounts: {
      staged: selectedFileCount(
        new Set(uncoveredPaths(model.selected.staged)),
        model.actionable.staged,
      ),
      unstaged: selectedFileCount(
        new Set(uncoveredPaths(model.selected.unstaged)),
        model.actionable.unstaged,
        stageActsOn,
      ),
      discard: selectedFileCount(
        new Set(uncoveredPaths(model.selected.unstaged)),
        model.actionable.unstaged,
        discardActsOn,
      ),
    },
    busy,
    discarding,
    setQuery: (query: string) => setSearch({ sessionId, query }),
    toggleOne,
    toggleVisible,
    uncheckUnder,
    clearSelection: () => setSelection({ sessionId, ...emptySelection() }),
    openDiscard: () => setDiscarding(true),
    closeDiscard: () => setDiscarding(false),
    runBulk,
    runDiscardMany,
  }
}
