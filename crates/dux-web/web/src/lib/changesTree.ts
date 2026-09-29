// React-free state for the Changes pane's expanded folders.
//
// A folded folder row can be expanded to show what is inside it, one level at
// a time, each level asked of the server when it is opened. What is expanded,
// and what each expanded folder was last seen to hold, lives here as an
// immutable map per agent: every transition returns a new map (or the same one
// when nothing moved), so the pane can hold it in state and memoize on it.
//
// A folder's key is its section and its path: the same folder can be a row on
// both sides (staged whole in part), and the two expand independently.

import type { ChangesSection } from "./changesWindow"
import type { ChangedFileView } from "./types"

// One expanded folder.
export interface FolderNode {
  // What the folder's row said when its children were last asked for. A
  // listing that moves it (a count, the fingerprint, what it holds) asks again.
  signature: string
  // What is inside, one level deep, or null until the first answer lands.
  children: ChangedFileView[] | null
  // Why the first answer failed, shown in place of the children.
  error: string | null
  // Why a later, quiet refresh failed: the children stay on screen, and the
  // folder's row says quietly that they could not be brought up to date.
  refreshError: string | null
  // A request for this folder is in flight.
  loading: boolean
}

export type Expansions = ReadonlyMap<string, FolderNode>

export const NO_EXPANSIONS: Expansions = new Map()

export function folderKey(section: ChangesSection, path: string): string {
  return `${section}:${path}`
}

// Whether a row can be expanded: a folded folder. A repository of its own and
// a worktree of this repository are not looked inside.
export function isExpandable(file: ChangedFileView): boolean {
  return file.kind === "directory"
}

// What about a folder row, when it moves, means its children may have too.
export function folderSignature(row: ChangedFileView): string {
  return [
    row.status,
    row.file_count ?? 0,
    row.fingerprint ?? "",
    row.nested_repositories ?? 0,
    row.linked_worktrees ?? 0,
    row.files_in_bare_repositories ?? 0,
    row.nested_repositories_not_staged ?? 0,
    row.linked_worktrees_not_staged ?? 0,
  ].join("|")
}

function withNode(exp: Expansions, key: string, node: FolderNode): Expansions {
  const next = new Map(exp)
  next.set(key, node)
  return next
}

// Open `row`, which starts loading its children.
export function expandFolder(
  exp: Expansions,
  section: ChangesSection,
  row: ChangedFileView,
): Expansions {
  return withNode(exp, folderKey(section, row.path), {
    signature: folderSignature(row),
    children: null,
    error: null,
    refreshError: null,
    loading: true,
  })
}

// Close a folder, forgetting it and every folder opened inside it: collapsing
// drops the children from memory, and opening it again asks afresh.
export function collapseFolder(
  exp: Expansions,
  section: ChangesSection,
  path: string,
): Expansions {
  const own = folderKey(section, path)
  const inside = `${own}/`
  let next: Map<string, FolderNode> | null = null
  for (const key of exp.keys()) {
    if (key === own || key.startsWith(inside)) {
      next ??= new Map(exp)
      next.delete(key)
    }
  }
  return next ?? exp
}

// The answer for a folder landed. An answer for a folder no longer expanded
// (collapsed while it was asked for) changes nothing.
export function settleFolder(
  exp: Expansions,
  section: ChangesSection,
  path: string,
  children: ChangedFileView[],
): Expansions {
  return settleFolderAndReconcile(exp, section, path, children).next
}

// A request for a folder failed. With nothing loaded yet the reason is shown in
// place of the children; with children already on screen they stay, and the
// folder's row carries the reason quietly.
export function failFolder(
  exp: Expansions,
  section: ChangesSection,
  path: string,
  message: string,
): Expansions {
  const key = folderKey(section, path)
  const node = exp.get(key)
  if (!node) return exp
  return withNode(exp, key, {
    ...node,
    loading: false,
    error: node.children === null ? message : null,
    refreshError: node.children === null ? null : message,
  })
}

// Ask for a folder again, after a failure.
export function retryFolder(
  exp: Expansions,
  section: ChangesSection,
  path: string,
): Expansions {
  const key = folderKey(section, path)
  const node = exp.get(key)
  if (!node) return exp
  return withNode(exp, key, { ...node, loading: true, error: null, refreshError: null })
}

// Every child row shown under the expanded folders among `rows` (a section's
// listing, or the part of it a filter shows), and under the folders expanded
// inside those, in no particular order. These are real rows a user can select
// and act on. Only folders reachable through shown parents count: an
// expansion whose parent is collapsed or gone holds nothing on screen.
export function loadedChildren(
  exp: Expansions,
  section: ChangesSection,
  rows: readonly ChangedFileView[],
): ChangedFileView[] {
  const out: ChangedFileView[] = []
  if (exp.size === 0) return out
  const walk = (level: readonly ChangedFileView[]) => {
    for (const row of level) {
      if (!isExpandable(row)) continue
      const children = exp.get(folderKey(section, row.path))?.children
      if (!children) continue
      out.push(...children)
      walk(children)
    }
  }
  walk(rows)
  return out
}

export interface ReconcileResult {
  next: Expansions
  // Folders to ask for again, quietly: their rows moved.
  refetch: { section: ChangesSection; path: string }[]
}

// Check the expanded folders among `rows` against those rows, and the folders
// expanded inside each against its own children, recursively. A folder whose
// row is gone or is no longer a folded folder is forgotten; one whose row moved
// is marked to be asked for again, keeping its children on screen, and what is
// expanded inside it waits for that answer, which is checked the same way when
// it lands. Every folder reached is recorded in `reached`.
function reconcileAmong(
  next: Map<string, FolderNode>,
  section: ChangesSection,
  rows: readonly ChangedFileView[],
  reached: Set<string>,
  refetch: ReconcileResult["refetch"],
): void {
  for (const row of rows) {
    const key = folderKey(section, row.path)
    const node = next.get(key)
    if (!node) continue
    if (!isExpandable(row)) continue
    reached.add(key)
    const signature = folderSignature(row)
    if (signature !== node.signature) {
      // A failure shown in place of the children gives way to the new
      // request; children on screen stay until its answer lands.
      next.set(key, { ...node, signature, loading: true, error: null })
      refetch.push({ section, path: row.path })
      // What is expanded inside waits for the new answer, untouched.
      for (const other of next.keys()) {
        if (other.startsWith(`${key}/`)) reached.add(other)
      }
      continue
    }
    if (node.children) reconcileAmong(next, section, node.children, reached, refetch)
  }
}

// Forget every expansion not reached from the rows, with everything under it.
function dropUnreached(
  next: Map<string, FolderNode>,
  reached: Set<string>,
  within: (key: string) => boolean,
): void {
  for (const key of [...next.keys()]) {
    if (within(key) && !reached.has(key)) next.delete(key)
  }
}

function sameExpansions(a: Expansions, b: Map<string, FolderNode>): boolean {
  if (a.size !== b.size) return false
  for (const [key, node] of b) if (a.get(key) !== node) return false
  return true
}

// Bring the expanded folders in line with a new listing. A folder is kept only
// while it is reachable: a row of the listing, or a folder among the children
// of a kept, expanded parent. A folder whose row is gone, or is no longer a
// folded folder, is forgotten with everything under it; a folder whose row
// moved is asked for again, keeping its old children on screen until the
// answer lands, and the folders expanded inside it are checked against that
// answer (see `settleFolderAndReconcile`).
export function reconcileExpansions(
  exp: Expansions,
  staged: readonly ChangedFileView[],
  unstaged: readonly ChangedFileView[],
): ReconcileResult {
  if (exp.size === 0) return { next: exp, refetch: [] }
  const next = new Map(exp)
  const reached = new Set<string>()
  const refetch: ReconcileResult["refetch"] = []
  reconcileAmong(next, "staged", staged, reached, refetch)
  reconcileAmong(next, "unstaged", unstaged, reached, refetch)
  dropUnreached(next, reached, () => true)
  return { next: sameExpansions(exp, next) ? exp : next, refetch }
}

// A folder's children landed: settle them, then check the folders expanded
// inside it against the NEW children, recursively. One that is gone is
// forgotten with its subtree (so nothing hidden stays checked or counted, and
// it comes back folded); one whose row moved is asked for again.
export function settleFolderAndReconcile(
  exp: Expansions,
  section: ChangesSection,
  path: string,
  children: ChangedFileView[],
): ReconcileResult {
  const key = folderKey(section, path)
  const node = exp.get(key)
  if (!node) return { next: exp, refetch: [] }
  const next = new Map(exp)
  next.set(key, { ...node, children, error: null, refreshError: null, loading: false })
  const reached = new Set<string>([key])
  const refetch: ReconcileResult["refetch"] = []
  reconcileAmong(next, section, children, reached, refetch)
  dropUnreached(next, reached, (other) => other.startsWith(`${key}/`))
  return { next, refetch }
}
