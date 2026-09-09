import { useRef, useState, useMemo, useCallback, useEffect } from "react"
import { ChevronRight, Loader2, RotateCw } from "lucide-react"
import { cn } from "@/lib/utils"
import { ScrollArea } from "@/components/ui/scroll-area"
import { ContextMenu, ContextMenuTrigger } from "@/components/ui/context-menu"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { FileTreeContextMenu } from "@/components/FileTreeContextMenu"
import { FileTreeIcon } from "@/components/FileTreeIcon"
import { dirIconKind, fileIconKind } from "@/lib/fileIcons"
import { fileApi } from "@/lib/fileApi"
import { dragCarriesFiles } from "@/lib/fileDrop"
import { classifyDroppedItems } from "@/lib/editorDrop"
import type { DroppedItems } from "@/lib/editorDrop"
import { useFilePicker } from "@/hooks/use-file-picker"
import { targetDirForCreate } from "@/lib/fileTreeOps"
import {
  ancestorDirs,
  descendantDirPaths,
  dirsToLoadFor,
  flattenLazy,
} from "@/lib/fileTree"
import type { DirState } from "@/lib/fileTree"
import type { EditorRoot } from "@/lib/editorRoot"

const noop = () => {}

const ROW_HEIGHT = 28 // px, must match the py-1 + text-sm row height
const OVERSCAN = 10 // rows to render above/below the viewport

interface FileTreeProps {
  root: EditorRoot
  openPath: string | null
  // path → raw git status code, for marking changed files in the tree.
  changed: Map<string, string>
  // A file whose ancestor chain is fetched and expanded on mount, and on every
  // later change, so a freshly created or deep-linked file is revealed.
  initialPath: string | null
  // Single click previews, double-click pins. A double-click also fires two
  // preceding clicks, which is harmless because `openFile` (lib/editorTabs.ts)
  // is idempotent for an already-open path.
  onOpen: (path: string, opts?: { pin?: boolean }) => void
  // Right-click menu callbacks. Optional so a caller exercising unrelated
  // behavior need not wire them; `EditorOverlay` provides every one.
  onNewFile?: (dir: string) => void
  onNewFolder?: (dir: string) => void
  onRename?: (path: string, isDir: boolean) => void
  onMove?: (path: string, isDir: boolean) => void
  onDelete?: (path: string, isDir: boolean) => void
  onInfo?: (path: string, isDir: boolean) => void
  // Bump the nonce (with the affected dir(s)) to force a refetch of those
  // directories after a create/rename/delete mutation lands.
  revalidate?: { dirs: string[]; nonce: number } | null
  // Whether the server accepts uploads at all (`file_drop_max_bytes > 0`). With
  // it off the tree does not highlight, accept a drop, or pretend it would work.
  fileDropEnabled?: boolean
  // Things dropped onto the tree, with the worktree-relative directory they
  // landed on ("" is the worktree root); the caller uploads and refreshes.
  //
  // It carries a `DroppedItems` rather than a `File[]` because a drop can also be
  // a folder, which dux does not take. The tree is the only place that can see
  // the `DataTransfer`, so it sorts the two apart and the caller reports both.
  onFilesDropped?: (dir: string, dropped: DroppedItems) => void
}

// Which drop target is under the pointer, as a row identity rather than a
// destination directory: a file row's destination is its parent, so several rows
// and the root can resolve to `""` while only one may light up.
const ROOT_DROP_KEY = "\u0000root"

export function FileTree({
  root,
  openPath,
  changed,
  initialPath,
  onOpen,
  onNewFile = noop,
  onNewFolder = noop,
  onRename = noop,
  onMove = noop,
  onDelete = noop,
  onInfo = noop,
  revalidate = null,
  fileDropEnabled = false,
  onFilesDropped,
}: FileTreeProps) {
  // The picker behind "Upload here…". It feeds the same `onFilesDropped` the
  // drag does, with the same per-row destination, so the two gestures cannot
  // land in two places. `folders` is always empty (no `webkitdirectory` here,
  // deliberately) and is passed rather than optional so the reporter keeps one
  // shape.
  const { input: pickerInput, open: openFilePicker } = useFilePicker()
  const uploadInto = (dir: string) => {
    void openFilePicker().then((files) => {
      if (files.length > 0) onFilesDropped?.(dir, { files, folders: [] })
    })
  }

  // The lazy loaded-directory cache: dirPath ("" = root) → DirState.
  const [dirs, setDirs] = useState<Map<string, DirState>>(() => new Map())
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  const [dropKey, setDropKey] = useState<string | null>(null)
  // The viewport arrives via a callback ref (state, not a plain ref) so the
  // measuring effect below re-runs when it mounts; a mount-only effect would
  // race the loading-spinner state and never attach.
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(400)
  // The dirs already requested, loading, resolved or errored alike, so effects
  // never auto-refetch one they have tried. Not cleared on failure (see the
  // `.catch` below): only the Retry button refetches an errored dir.
  const requestedRef = useRef<Set<string>>(new Set())
  // Unmount guard plus a per-dir request counter, so a stale response never
  // overwrites fresher state.
  const unmountedRef = useRef(false)
  const requestTokenRef = useRef<Map<string, number>>(new Map())

  useEffect(() => {
    return () => {
      unmountedRef.current = true
    }
  }, [])

  useEffect(() => {
    if (!viewportEl) return
    // ResizeObserver delivers an initial notification on observe(), so this
    // both seeds the height and tracks later resizes.
    const ro = new ResizeObserver(() =>
      setViewportHeight(viewportEl.clientHeight),
    )
    ro.observe(viewportEl)
    return () => ro.disconnect()
  }, [viewportEl])

  const fetchDir = useCallback(
    (dir: string) => {
      requestedRef.current.add(dir)
      const token = (requestTokenRef.current.get(dir) ?? 0) + 1
      requestTokenRef.current.set(dir, token)
      setDirs((prev) => {
        const next = new Map(prev)
        next.set(dir, { status: "loading" })
        return next
      })
      fileApi
        .tree(root, dir)
        .then((result) => {
          if (unmountedRef.current || requestTokenRef.current.get(dir) !== token)
            return
          setDirs((prev) => {
            const next = new Map(prev)
            next.set(dir, { status: "loaded", entries: result.entries })
            return next
          })
        })
        .catch((e) => {
          if (unmountedRef.current || requestTokenRef.current.get(dir) !== token)
            return
          // `dir` deliberately stays in `requestedRef`: dropping it makes the
          // missing-ancestors effect below re-request an errored dir on every
          // `dirs` change, retrying forever with no backoff. Only the Retry
          // button, which calls `fetchDir` directly, refetches one.
          setDirs((prev) => {
            const next = new Map(prev)
            next.set(dir, {
              status: "error",
              message:
                e instanceof Error ? e.message : "could not list directory",
            })
            return next
          })
        })
    },
    [root],
  )

  // Mount: fetch the root; when a deep-link target is present, also fetch and
  // expand its ancestor chain so the opened file is revealed without clicks.
  useEffect(() => {
    const target = initialPath ?? ""
    const toLoad = dirsToLoadFor(target, requestedRef.current)
    for (const d of toLoad) fetchDir(d)
    if (initialPath) {
      setExpanded((prev) => {
        const next = new Set(prev)
        for (const d of dirsToLoadFor(initialPath, new Set([""])))
          next.add(d)
        return next
      })
    }
    // Mount-only: the editor body remounts per session and open, and
    // `initialPath` is fixed for a mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Gates the parent-refetch below to once per opened file however often the
  // effect re-runs, which is what stops a refetch loop for a file that is not on
  // disk at all. The missing-ancestors fetch is guarded separately, by
  // `fetchDir` never clearing a failed dir from `requestedRef`.
  const revealCheckedRef = useRef<string | null>(null)
  // One-shot per `openPath`: without the latch every `dirs` change re-adds the
  // ancestors to `expanded`, overriding a user who collapsed one. After the
  // initial reveal, the user's collapse wins.
  const autoExpandedRef = useRef<string | null>(null)

  // Pull and expand the ancestors of an open file whose parents were never
  // loaded. A parent that is loaded but does not list the file is refetched once.
  useEffect(() => {
    if (!openPath) return
    const missing = dirsToLoadFor(openPath, requestedRef.current)
    for (const d of missing) fetchDir(d)
    if (revealCheckedRef.current !== openPath) {
      const parents = ancestorDirs(openPath)
      const parent = parents.length > 0 ? parents[parents.length - 1] : ""
      const st = dirs.get(parent)
      if (st?.status === "loaded") {
        revealCheckedRef.current = openPath
        if (!st.entries.some((e) => e.path === openPath)) fetchDir(parent)
      }
    }
    if (autoExpandedRef.current !== openPath) {
      autoExpandedRef.current = openPath
      setExpanded((prev) => {
        const wanted = dirsToLoadFor(openPath, new Set([""]))
        if (wanted.every((d) => prev.has(d))) return prev
        const next = new Set(prev)
        for (const d of wanted) next.add(d)
        return next
      })
    }
  }, [openPath, dirs, fetchDir])

  // Post-mutation revalidation: force-refetch the affected dirs, bypassing
  // `requestedRef` as the Retry button does, and expand each so a newly created
  // entry is visible without another click.
  useEffect(() => {
    if (!revalidate) return
    for (const d of revalidate.dirs) {
      requestedRef.current.delete(d)
      // `fetchDir` seeds `{ status: "loading" }` before it fetches, which the
      // lint's call-graph tracing reads as a set-state-in-effect; it is the same
      // escape hatch the Retry button uses.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      fetchDir(d)
    }
    setExpanded((prev) => {
      if (revalidate.dirs.every((d) => d === "" || prev.has(d))) return prev
      const next = new Set(prev)
      for (const d of revalidate.dirs) {
        if (d !== "") next.add(d)
      }
      return next
    })
    // Only the nonce may retrigger this: `dirs` would refetch on every unrelated
    // directory load, and `fetchDir` is stable per root.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revalidate?.nonce])

  const toggle = useCallback(
    (path: string, expandable: boolean) => {
      if (expanded.has(path)) {
        // Collapsing evicts this dir's listing and every descendant's, so a huge
        // subtree does not linger in memory; re-expanding refetches.
        const toEvict = [path, ...descendantDirPaths(dirs, path)]
        setDirs((prev) => {
          const next = new Map(prev)
          for (const d of toEvict) next.delete(d)
          return next
        })
        for (const d of toEvict) requestedRef.current.delete(d)
        setExpanded((prev) => {
          const next = new Set(prev)
          for (const d of toEvict) next.delete(d)
          return next
        })
      } else {
        setExpanded((prev) => {
          const next = new Set(prev)
          next.add(path)
          return next
        })
        if (expandable && !requestedRef.current.has(path)) fetchDir(path)
      }
    },
    [fetchDir, dirs, expanded],
  )

  // Re-flattens the whole visible tree on any `dirs` or `expanded` change, at a
  // cost linear in loaded nodes. Accepted: the list below is virtualized, so
  // render work is bounded by the viewport, and collapse evicts subtrees.
  const rows = useMemo(() => flattenLazy(dirs, expanded), [dirs, expanded])

  const rootState = dirs.get("")

  const totalHeight = rows.length * ROW_HEIGHT

  const firstVisible = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
  const lastVisible = Math.min(
    rows.length - 1,
    Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN,
  )
  const visibleRows = rows.slice(firstVisible, lastVisible + 1)

  // The native drag handlers one drop target needs, for the row identified by
  // `key` delivering into `dir`.
  //
  // `stopPropagation` keeps a row and the root filler that wraps it from both
  // claiming the drag: the bubbling event reaches the filler last, so the root
  // would win every time and retarget a folder drop to the worktree root.
  //
  // `preventDefault` on dragover is required too, or the browser refuses the
  // drop and navigates to the dropped file, throwing the editor away.
  //
  // Clearing on dragleave uses no depth counter: a leave fired while crossing
  // into a child self-heals on the next dragover, so the worst case is one frame
  // of missing highlight rather than a stuck one.
  const dropHandlers = useCallback(
    (key: string, dir: string) => {
      if (!fileDropEnabled || !onFilesDropped) return {}
      const claim = (e: React.DragEvent) => {
        if (!dragCarriesFiles(e.dataTransfer?.types)) return false
        e.preventDefault()
        e.stopPropagation()
        return true
      }
      return {
        onDragEnter: (e: React.DragEvent) => {
          if (claim(e)) setDropKey(key)
        },
        onDragOver: (e: React.DragEvent) => {
          if (!claim(e)) return
          e.dataTransfer.dropEffect = "copy"
          setDropKey(key)
        },
        onDragLeave: (e: React.DragEvent) => {
          if (!dragCarriesFiles(e.dataTransfer?.types)) return
          e.stopPropagation()
          setDropKey((current) => (current === key ? null : current))
        },
        onDrop: (e: React.DragEvent) => {
          if (!claim(e)) return
          setDropKey(null)
          // Sorted here because this is the only place the `DataTransfer` is
          // reachable, and reported even when it produced neither a file nor a
          // folder, or letting go of a folder looks like letting go of nothing.
          onFilesDropped(
            dir,
            classifyDroppedItems(
              Array.from(e.dataTransfer.files ?? []),
              Array.from(e.dataTransfer.items ?? []),
            ),
          )
        },
      }
    },
    [fileDropEnabled, onFilesDropped],
  )

  // The highlight on the row the drop would land in, as classes a caller can
  // merge into what the element already carries, through tokens.
  const dropClass = (key: string) =>
    dropKey === key && "bg-primary/10 ring-1 ring-primary"

  // Rows are windowed against the tree's own ScrollArea. Virtualizing against
  // any other element breaks silently, since the window only moves on scroll
  // events from the element it measures. The loading, error and empty states
  // render inside the same ScrollArea so the viewport exists from first paint.
  return (
    <ScrollArea
      className="min-h-0 flex-1"
      viewportRef={setViewportEl}
      onViewportScroll={(e) => {
        setScrollTop(e.currentTarget.scrollTop)
        // Track height here too: cheap, and covers environments where the
        // ResizeObserver is inert.
        setViewportHeight(e.currentTarget.clientHeight)
      }}
    >
      {/* The picker's hidden input, mounted with the tree so a menu item's
        * click reaches it synchronously: the browser's user activation is spent
        * by the time a promise resolves. */}
      {pickerInput}
      <ContextMenu>
      <ContextMenuTrigger
        render={
          <div
            data-testid="file-tree-drop-surface"
            // A right-click landing directly on this filler, rather than
            // bubbling from a row's own trigger, opens the root menu.
            // `min-h-full` covers the space below the last row so a click there
            // still hits it, and a drop on that space means the worktree root.
            {...dropHandlers(ROOT_DROP_KEY, "")}
            className={cn("min-h-full rounded p-1", dropClass(ROOT_DROP_KEY))}
          />
        }
      >
        {!rootState || rootState.status === "loading" ? (
          <div className="flex items-center justify-center py-4 text-muted-foreground">
            <Loader2 className="size-4 motion-safe:animate-spin" />
          </div>
        ) : rootState.status === "error" ? (
          <div className="flex flex-col items-start gap-1 px-1 py-2">
            <p className="text-sm text-destructive">{rootState.message}</p>
            <button
              type="button"
              onClick={() => fetchDir("")}
              className="flex items-center gap-1 rounded px-1 py-0.5 text-sm text-muted-foreground hover:bg-muted"
            >
              <RotateCw className="size-3.5" />
              Retry
            </button>
          </div>
        ) : rootState.entries.length === 0 ? (
          <p className="px-1 py-2 text-sm text-muted-foreground">
            No files in this worktree.
          </p>
        ) : (
          /* Total-height spacer so the scrollbar reflects the full list. */
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
          {visibleRows.map((row) =>
            row.isDir ? (
              <li key={row.path}>
                <ContextMenu>
                  <ContextMenuTrigger
                    render={
                      <button
                        type="button"
                        onClick={() => toggle(row.path, row.expandable)}
                        // This row's own trigger already opens its menu, so the
                        // event must not bubble to the root trigger above and
                        // open that one from the same right-click.
                        onContextMenu={(e) => e.stopPropagation()}
                        aria-expanded={expanded.has(row.path)}
                        // Dropping on a folder puts the files in it, routed
                        // through the same mapping the file row uses: two rows
                        // answering the destination question separately drift.
                        {...dropHandlers(
                          row.path,
                          targetDirForCreate({ kind: "dir", path: row.path }),
                        )}
                        className={cn(
                          "flex w-full items-center gap-1 rounded py-1 pr-1 text-left hover:bg-muted",
                          dropClass(row.path),
                        )}
                        style={{
                          paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                          height: ROW_HEIGHT,
                        }}
                      />
                    }
                  >
                    {row.state === "loading" ? (
                      <Loader2 className="size-3.5 shrink-0 text-muted-foreground motion-safe:animate-spin" />
                    ) : (
                      <ChevronRight
                        className={cn(
                          "size-3.5 shrink-0 text-muted-foreground transition-transform",
                          expanded.has(row.path) && "rotate-90",
                        )}
                      />
                    )}
                    <FileTreeIcon
                      kind={dirIconKind({
                        open: expanded.has(row.path),
                        empty: row.empty,
                      })}
                    />
                    <span className="min-w-0 flex-1 truncate text-sm font-medium">
                      {row.name}
                    </span>
                  </ContextMenuTrigger>
                  <FileTreeContextMenu
                    variant="dir"
                    onNewFile={() =>
                      onNewFile(targetDirForCreate({ kind: "dir", path: row.path }))
                    }
                    onNewFolder={() =>
                      onNewFolder(
                        targetDirForCreate({ kind: "dir", path: row.path }),
                      )
                    }
                    canUpload={fileDropEnabled}
                    onUpload={() =>
                      uploadInto(targetDirForCreate({ kind: "dir", path: row.path }))
                    }
                    onRename={() => onRename(row.path, true)}
                    onMove={() => onMove(row.path, true)}
                    onDelete={() => onDelete(row.path, true)}
                    onInfo={() => onInfo(row.path, true)}
                  />
                </ContextMenu>
              </li>
            ) : row.kind === "loading" ? (
              <li key={row.path}>
                <div
                  className="flex items-center gap-1 py-1 pr-1 text-muted-foreground"
                  style={{
                    paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                    height: ROW_HEIGHT,
                  }}
                >
                  <Loader2 className="size-3.5 shrink-0 motion-safe:animate-spin" />
                  <span className="text-sm">Loading…</span>
                </div>
              </li>
            ) : row.kind === "error" ? (
              <li key={row.path}>
                <button
                  type="button"
                  onClick={() =>
                    fetchDir(row.path.slice(0, -"/__error__".length))
                  }
                  className="flex w-full items-center gap-1 rounded py-1 pr-1 text-left text-muted-foreground hover:bg-muted"
                  style={{
                    paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                    height: ROW_HEIGHT,
                  }}
                >
                  <RotateCw className="size-3.5 shrink-0" />
                  <span className="text-sm">Failed to load — retry</span>
                </button>
              </li>
            ) : (
              <li key={row.path}>
                <ContextMenu>
                  <ContextMenuTrigger
                    render={
                      <button
                        type="button"
                        onClick={() => onOpen(row.path)}
                        onDoubleClick={() => onOpen(row.path, { pin: true })}
                        // See the dir row's identical comment: stops this
                        // row's right-click from also opening the root menu.
                        onContextMenu={(e) => e.stopPropagation()}
                        // A file is not a place to put a file, so a drop here
                        // targets the folder it is in, through the same mapping.
                        {...dropHandlers(
                          row.path,
                          targetDirForCreate({ kind: "file", path: row.path }),
                        )}
                        style={{
                          paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                          height: ROW_HEIGHT,
                        }}
                        className={cn(
                          "flex w-full items-center gap-1.5 rounded py-1 pr-1 text-left hover:bg-muted",
                          row.path === openPath && "bg-muted",
                          dropClass(row.path),
                        )}
                      />
                    }
                  >
                    <FileTreeIcon kind={fileIconKind(row.path)} />
                    <span className="min-w-0 flex-1 truncate font-mono text-sm">
                      {row.name}
                    </span>
                    {changed.get(row.path) && (
                      <FileStatusIcon status={changed.get(row.path)!} />
                    )}
                  </ContextMenuTrigger>
                  <FileTreeContextMenu
                    variant="file"
                    onNewFile={() =>
                      onNewFile(
                        targetDirForCreate({ kind: "file", path: row.path }),
                      )
                    }
                    onNewFolder={() =>
                      onNewFolder(
                        targetDirForCreate({ kind: "file", path: row.path }),
                      )
                    }
                    canUpload={fileDropEnabled}
                    // A file is not a place to put a file, so this targets the
                    // folder the file is IN: the same mapping the drop uses.
                    onUpload={() =>
                      uploadInto(targetDirForCreate({ kind: "file", path: row.path }))
                    }
                    onRename={() => onRename(row.path, false)}
                    onMove={() => onMove(row.path, false)}
                    onDelete={() => onDelete(row.path, false)}
                    onInfo={() => onInfo(row.path, false)}
                  />
                </ContextMenu>
              </li>
              ),
            )}
          </ul>
          </div>
        )}
      </ContextMenuTrigger>
      <FileTreeContextMenu
        variant="root"
        onNewFile={() => onNewFile(targetDirForCreate({ kind: "root" }))}
        onNewFolder={() => onNewFolder(targetDirForCreate({ kind: "root" }))}
        canUpload={fileDropEnabled}
        onUpload={() => uploadInto(targetDirForCreate({ kind: "root" }))}
        onRename={noop}
        onMove={noop}
        onDelete={noop}
        onInfo={noop}
      />
      </ContextMenu>
    </ScrollArea>
  )
}
