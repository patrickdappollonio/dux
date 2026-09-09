// The move's composition, lifted out of EditorBody so it can be tested. A move is a rename on
// the wire, plus retargeting every open editor tab that pointed at the moved path or anything
// under it, plus revalidating both the directory it left and the one it arrived in, plus
// reindexing the search list.

import { moveTarget, parentDir } from "@/lib/fileTreeOps"
import { movedMessage } from "@/lib/editorMutations"

export interface MoveEntryDeps {
  /** The wire call. A move deliberately reuses the rename route. */
  rename: (from: string, to: string) => Promise<void>
  /** Dismiss the dialog. Only on success: a failure keeps it open. */
  clearTarget: () => void
  /** Retarget open editor tabs from the old path (or prefix) to the new one. */
  retargetTabs: (from: string, to: string) => void
  /** Force the lazy file tree to refetch these directories. */
  revalidateDirs: (dirs: string[]) => void
  refreshSearchIndex: () => Promise<void>
  /** Confirm the move, naming the entry and where it went. */
  reportSuccess: (message: string) => void
  reportError: (message: string) => void
}

// Returns a promise that resolves once everything has settled, success or
// failure, because the dialog's submit button keys its busy state off it.
export function performMove(
  from: string,
  destDir: string,
  deps: MoveEntryDeps,
): Promise<void> {
  const to = moveTarget(from, destDir)
  return deps
    .rename(from, to)
    .then(() => {
      deps.clearTarget()
      // Said before the refetches: the move has already landed, and a rejected revalidation
      // must not turn a successful move into silence.
      deps.reportSuccess(movedMessage(from, destDir))
      deps.retargetTabs(from, to)
      // Both ends: the source directory lost an entry and the destination
      // gained one, and the tree caches them independently.
      deps.revalidateDirs([parentDir(from), parentDir(to)])
      return deps.refreshSearchIndex()
    })
    .catch((e: unknown) => {
      deps.reportError(e instanceof Error ? e.message : "could not move")
    })
}
