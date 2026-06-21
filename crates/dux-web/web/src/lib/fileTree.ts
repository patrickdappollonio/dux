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

/// Cap on the number of FileTreeNode objects that buildFileTree will create.
/// Prevents a multi-hundred-thousand-file repo from OOMing the browser tab.
export const FILETREE_NODE_CAP = 100_000

/// Result of buildFileTree when the cap is hit.
export interface FileTreeResult {
  nodes: FileTreeNode[]
  /// True when the input was truncated at FILETREE_NODE_CAP.
  capped: boolean
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
/// Uses a parallel Map<string, FileTreeNode> per directory for O(1) child
/// lookup so large repos (tens-of-thousands of files) stay linear.
export function buildFileTree(paths: string[]): FileTreeResult {
  const root: FileTreeNode = { name: "", path: "", isDir: true, children: [] }
  // childMap caches the children of each directory node by their lookup key
  // ("name:isDir") so we don't scan the children array on every insertion.
  const childMap = new Map<FileTreeNode, Map<string, FileTreeNode>>()
  childMap.set(root, new Map())

  let nodeCount = 0
  let capped = false

  outer: for (const p of paths) {
    const segments = p.split("/").filter(Boolean)
    let node = root
    let acc = ""
    for (let i = 0; i < segments.length; i++) {
      const seg = segments[i]
      acc = acc ? `${acc}/${seg}` : seg
      const isDir = i < segments.length - 1
      const key = `${seg}:${isDir}`
      let nodeChildren = childMap.get(node)
      if (!nodeChildren) {
        nodeChildren = new Map()
        childMap.set(node, nodeChildren)
      }
      let child = nodeChildren.get(key)
      if (!child) {
        if (nodeCount >= FILETREE_NODE_CAP) {
          capped = true
          break outer
        }
        child = { name: seg, path: acc, isDir, children: [] }
        node.children.push(child)
        nodeChildren.set(key, child)
        nodeCount++
      }
      node = child
    }
  }
  sortNodes(root.children)
  return { nodes: root.children, capped }
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
