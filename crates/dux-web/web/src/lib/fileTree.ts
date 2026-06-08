// Pure helpers for turning a flat list of worktree-relative file paths into a
// nested tree for the editor's file browser. Kept free of React so it's
// trivially unit-testable.

export interface FileTreeNode {
  // The path segment (folder or file name).
  name: string
  // The full worktree-relative path of this node.
  path: string
  isDir: boolean
  // Empty for files.
  children: FileTreeNode[]
}

function sortNodes(nodes: FileTreeNode[]): void {
  // Directories first, then files; each group alphabetical (case-insensitive).
  nodes.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
  })
  for (const n of nodes) if (n.isDir) sortNodes(n.children)
}

/// Build the top-level nodes of a file tree from worktree-relative paths.
export function buildFileTree(paths: string[]): FileTreeNode[] {
  const root: FileTreeNode = { name: "", path: "", isDir: true, children: [] }
  for (const p of paths) {
    const segments = p.split("/").filter(Boolean)
    let node = root
    let acc = ""
    segments.forEach((seg, i) => {
      acc = acc ? `${acc}/${seg}` : seg
      const isDir = i < segments.length - 1
      let child = node.children.find((c) => c.name === seg && c.isDir === isDir)
      if (!child) {
        child = { name: seg, path: acc, isDir, children: [] }
        node.children.push(child)
      }
      node = child
    })
  }
  sortNodes(root.children)
  return root.children
}

/// The ancestor directory paths of a file path (e.g. "a/b/c.ts" → ["a", "a/b"]),
/// used to auto-expand the tree down to a file opened from elsewhere.
export function ancestorDirs(filePath: string): string[] {
  const segments = filePath.split("/").filter(Boolean)
  const dirs: string[] = []
  let acc = ""
  for (let i = 0; i < segments.length - 1; i++) {
    acc = acc ? `${acc}/${segments[i]}` : segments[i]
    dirs.push(acc)
  }
  return dirs
}
