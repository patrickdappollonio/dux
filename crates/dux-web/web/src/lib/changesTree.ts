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
  // Why the first answer failed, shown in place of the children. A failure of
  // a later, quiet refresh keeps the children on screen and marks nothing.
  error: string | null
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
  const key = folderKey(section, path)
  const node = exp.get(key)
  if (!node) return exp
  return withNode(exp, key, { ...node, children, error: null, loading: false })
}

// A request for a folder failed. With nothing loaded yet the reason is shown in
// place of the children; with children already on screen they stay, unmarked.
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
  return withNode(exp, key, { ...node, loading: true, error: null })
}

// Every child row loaded under an expanded folder of `section`, in no
// particular order. These are real rows a user can select and act on.
export function loadedChildren(
  exp: Expansions,
  section: ChangesSection,
): ChangedFileView[] {
  const prefix = `${section}:`
  const rows: ChangedFileView[] = []
  for (const [key, node] of exp) {
    if (key.startsWith(prefix) && node.children) rows.push(...node.children)
  }
  return rows
}

export interface ReconcileResult {
  next: Expansions
  // Folders to ask for again, quietly: their rows moved.
  refetch: { section: ChangesSection; path: string }[]
}

function parentPath(path: string): string | null {
  const cut = path.lastIndexOf("/")
  return cut > 0 ? path.slice(0, cut) : null
}

// Bring the expanded folders in line with a new listing. A folder whose row is
// gone (from the listing, or from its expanded parent's children), or is no
// longer a folded folder, is forgotten with everything under it. A folder whose
// row moved is asked for again, keeping its old children on screen until the
// answer lands. Parents are settled before their children, since a sub-folder's
// row lives in its parent's children.
export function reconcileExpansions(
  exp: Expansions,
  staged: readonly ChangedFileView[],
  unstaged: readonly ChangedFileView[],
): ReconcileResult {
  if (exp.size === 0) return { next: exp, refetch: [] }
  const top: Record<ChangesSection, Map<string, ChangedFileView>> = {
    staged: new Map(staged.map((row) => [row.path, row])),
    unstaged: new Map(unstaged.map((row) => [row.path, row])),
  }
  const entries = [...exp.entries()]
    .map(([key, node]) => {
      const cut = key.indexOf(":")
      return {
        key,
        node,
        section: key.slice(0, cut) as ChangesSection,
        path: key.slice(cut + 1),
      }
    })
    .sort((a, b) => a.path.split("/").length - b.path.split("/").length)

  let next: Map<string, FolderNode> | null = null
  const current = () => next ?? exp
  const refetch: ReconcileResult["refetch"] = []
  for (const { key, node, section, path } of entries) {
    const parent = parentPath(path)
    let row = top[section].get(path)
    if (row === undefined && parent !== null) {
      row = current()
        .get(folderKey(section, parent))
        ?.children?.find((child) => child.path === path)
    }
    if (row === undefined || !isExpandable(row)) {
      next ??= new Map(exp)
      next.delete(key)
      continue
    }
    const signature = folderSignature(row)
    if (signature === node.signature) continue
    next ??= new Map(exp)
    next.set(key, { ...node, signature, loading: true })
    refetch.push({ section, path })
  }
  return { next: current(), refetch }
}
