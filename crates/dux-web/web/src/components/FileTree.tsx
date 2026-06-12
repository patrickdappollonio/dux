import { useState } from "react"
import { ChevronRight, File as FileIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import type { FileTreeNode } from "@/lib/fileTree"

interface FileTreeBase {
  openPath: string | null
  // path → raw git status code, for marking changed files in the tree.
  changed: Map<string, string>
  // dir paths to expand on first render (the open file's ancestors).
  defaultExpanded: Set<string>
  onOpen: (path: string) => void
}

interface FileTreeProps extends FileTreeBase {
  nodes: FileTreeNode[]
}

export function FileTree({
  nodes,
  openPath,
  changed,
  defaultExpanded,
  onOpen,
}: FileTreeProps) {
  return (
    <ul className="flex flex-col">
      {nodes.map((node) => (
        <TreeItem
          key={node.path}
          node={node}
          depth={0}
          openPath={openPath}
          changed={changed}
          defaultExpanded={defaultExpanded}
          onOpen={onOpen}
        />
      ))}
    </ul>
  )
}

interface TreeItemProps extends FileTreeBase {
  node: FileTreeNode
  depth: number
}

function TreeItem({
  node,
  depth,
  openPath,
  changed,
  defaultExpanded,
  onOpen,
}: TreeItemProps) {
  // depth indent; leaves get extra left pad so their icon lines up past the
  // folder chevron column.
  const indent = { paddingLeft: `${depth * 0.75 + 0.25}rem` }

  if (!node.isDir) {
    const status = changed.get(node.path)
    return (
      <li>
        <button
          type="button"
          onClick={() => onOpen(node.path)}
          style={indent}
          className={cn(
            "flex w-full items-center gap-1.5 rounded py-1 pr-1 text-left hover:bg-muted",
            node.path === openPath && "bg-muted",
          )}
        >
          <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1 truncate font-mono text-sm">
            {node.name}
          </span>
          {status && <FileStatusIcon status={status} />}
        </button>
      </li>
    )
  }

  return (
    <FolderItem
      node={node}
      depth={depth}
      indent={indent}
      openPath={openPath}
      changed={changed}
      defaultExpanded={defaultExpanded}
      onOpen={onOpen}
    />
  )
}

function FolderItem({
  node,
  depth,
  indent,
  openPath,
  changed,
  defaultExpanded,
  onOpen,
}: TreeItemProps & { indent: React.CSSProperties }) {
  const [open, setOpen] = useState(() => defaultExpanded.has(node.path))
  return (
    <li>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        style={indent}
        aria-expanded={open}
        className="flex w-full items-center gap-1 rounded py-1 pr-1 text-left hover:bg-muted"
      >
        <ChevronRight
          className={cn(
            "size-3.5 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90",
          )}
        />
        <span className="min-w-0 flex-1 truncate text-sm font-medium">
          {node.name}
        </span>
      </button>
      {open && (
        <ul className="flex flex-col">
          {node.children.map((child) => (
            <TreeItem
              key={child.path}
              node={child}
              depth={depth + 1}
              openPath={openPath}
              changed={changed}
              defaultExpanded={defaultExpanded}
              onOpen={onOpen}
            />
          ))}
        </ul>
      )}
    </li>
  )
}
