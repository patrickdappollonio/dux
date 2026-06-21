import { useRef, useState, useMemo, useCallback, useEffect } from "react"
import { ChevronRight, File as FileIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import type { FileTreeNode } from "@/lib/fileTree"

const ROW_HEIGHT = 28 // px — must match the py-1 + text-sm row height
const OVERSCAN = 10  // rows to render above/below the viewport

interface FileTreeProps {
  nodes: FileTreeNode[]
  openPath: string | null
  // path → raw git status code, for marking changed files in the tree.
  changed: Map<string, string>
  // dir paths to expand on first render (the open file's ancestors).
  defaultExpanded: Set<string>
  onOpen: (path: string) => void
  capped?: boolean
}

interface FlatRow {
  node: FileTreeNode
  depth: number
}

function flattenTree(
  nodes: FileTreeNode[],
  expanded: Set<string>,
  depth = 0,
): FlatRow[] {
  const rows: FlatRow[] = []
  for (const node of nodes) {
    rows.push({ node, depth })
    if (node.isDir && expanded.has(node.path)) {
      rows.push(...flattenTree(node.children, expanded, depth + 1))
    }
  }
  return rows
}

export function FileTree({
  nodes,
  openPath,
  changed,
  defaultExpanded,
  onOpen,
  capped,
}: FileTreeProps) {
  const [expanded, setExpanded] = useState<Set<string>>(
    () => new Set(defaultExpanded),
  )
  const containerRef = useRef<HTMLDivElement>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(400)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    setViewportHeight(el.clientHeight)
    const ro = new ResizeObserver(() => setViewportHeight(el.clientHeight))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const toggle = useCallback((path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }, [])

  const rows = useMemo(
    () => flattenTree(nodes, expanded),
    [nodes, expanded],
  )

  const totalHeight = rows.length * ROW_HEIGHT

  const firstVisible = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
  const lastVisible = Math.min(
    rows.length - 1,
    Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN,
  )
  const visibleRows = rows.slice(firstVisible, lastVisible + 1)

  return (
    <div
      ref={containerRef}
      className="overflow-y-auto"
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      style={{ position: "relative" }}
    >
      {capped && (
        <div className="px-3 py-1 text-xs text-muted-foreground border-b">
          Tree too large to fully display — some entries omitted.
        </div>
      )}
      {/* Total-height spacer so the scrollbar reflects the full list. */}
      <div style={{ height: totalHeight, position: "relative" }}>
        <ul
          style={{
            position: "absolute",
            top: firstVisible * ROW_HEIGHT,
            left: 0,
            right: 0,
          }}
          className="flex flex-col"
        >
          {visibleRows.map(({ node, depth }) =>
            node.isDir ? (
              <li key={node.path}>
                <button
                  type="button"
                  onClick={() => toggle(node.path)}
                  aria-expanded={expanded.has(node.path)}
                  className="flex w-full items-center gap-1 rounded py-1 pr-1 text-left hover:bg-muted"
                  style={{ paddingLeft: `${depth * 0.75 + 0.25}rem`, height: ROW_HEIGHT }}
                >
                  <ChevronRight
                    className={cn(
                      "size-3.5 shrink-0 text-muted-foreground transition-transform",
                      expanded.has(node.path) && "rotate-90",
                    )}
                  />
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">
                    {node.name}
                  </span>
                </button>
              </li>
            ) : (
              <li key={node.path}>
                <button
                  type="button"
                  onClick={() => onOpen(node.path)}
                  style={{ paddingLeft: `${depth * 0.75 + 0.25}rem`, height: ROW_HEIGHT }}
                  className={cn(
                    "flex w-full items-center gap-1.5 rounded py-1 pr-1 text-left hover:bg-muted",
                    node.path === openPath && "bg-muted",
                  )}
                >
                  <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate font-mono text-sm">
                    {node.name}
                  </span>
                  {changed.get(node.path) && (
                    <FileStatusIcon status={changed.get(node.path)!} />
                  )}
                </button>
              </li>
            ),
          )}
        </ul>
      </div>
    </div>
  )
}
