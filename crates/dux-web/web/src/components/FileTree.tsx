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

const noop = () => {}

const ROW_HEIGHT = 28 // px — must match the py-1 + text-sm row height
const OVERSCAN = 10 // rows to render above/below the viewport

interface FileTreeProps {
  sessionId: string
  openPath: string | null
  // path → raw git status code, for marking changed files in the tree.
  changed: Map<string, string>
  // A file whose ancestor chain should be fetched + expanded on mount (deep
  // link / opened-from-elsewhere). Later changes to openPath also pull any
  // not-yet-loaded parents so a freshly created file is revealed.
  initialPath: string | null
  // Single click = preview open (`onOpen(path)`); double-click = permanent
  // open / pin (`onOpen(path, { pin: true })`). A double-click also fires two
  // preceding `onClick`s, harmless since `openFile` (lib/editorTabs.ts) is
  // idempotent for an already-open path (it just activates), so the pin lands
  // cleanly right after.
  onOpen: (path: string, opts?: { pin?: boolean }) => void
  // Right-click menu callbacks (New File…/New Folder…/Rename…/Delete…). All
  // optional so tests exercising unrelated behavior don't need to wire them;
  // production usage (EditorOverlay) always provides all four.
  onNewFile?: (dir: string) => void
  onNewFolder?: (dir: string) => void
  onRename?: (path: string, isDir: boolean) => void
  onMove?: (path: string, isDir: boolean) => void
  onDelete?: (path: string, isDir: boolean) => void
  onInfo?: (path: string, isDir: boolean) => void
  // Bump the nonce (with the affected dir(s)) to force a refetch of those
  // directories after a create/rename/delete mutation lands.
  revalidate?: { dirs: string[]; nonce: number } | null
  // Whether the server accepts uploads at all (`file_drop_max_bytes > 0`).
  // With it off the tree does not highlight, does not accept a drop, and does
  // not pretend a drop would work.
  fileDropEnabled?: boolean
  // Things dropped from the DESKTOP onto the tree, with the worktree-relative
  // directory they were dropped on ("" = the worktree root). This is the
  // durable drop intent, "add this file to my project": the caller uploads
  // them and refreshes, exactly as it does after a move.
  //
  // It carries a `DroppedItems` rather than a `File[]` because a drop can also
  // be a FOLDER, which is a natural gesture on a file tree and is not something
  // dux takes. The tree sorts the two apart (it is the only place that can see
  // the `DataTransfer`) and the caller reports both.
  onFilesDropped?: (dir: string, dropped: DroppedItems) => void
}

// The key identifying which drop target is currently under the pointer.
//
// It is a ROW identity, not the destination directory, and the two are
// genuinely different: a file row's destination is its PARENT, so several rows
// and the root surface can all resolve to `""` while only the one the pointer
// is actually over may light up.
const ROOT_DROP_KEY = "\u0000root"

export function FileTree({
  sessionId,
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
  // The picker behind "Upload here…". It feeds the SAME `onFilesDropped` the
  // drag does, with the same per-row destination resolution, so the two
  // gestures cannot land in two places.
  //
  // `folders: []` always: a file picker cannot produce a directory (no
  // `webkitdirectory` here, deliberately), so the folder-refusal rung of the
  // tree's outcome ladder is unreachable from this gesture. It is passed
  // rather than made optional so the one shared reporter keeps one shape.
  const { input: pickerInput, open: openFilePicker } = useFilePicker()
  const uploadInto = (dir: string) => {
    void openFilePicker().then((files) => {
      if (files.length > 0) onFilesDropped?.(dir, { files, folders: [] })
    })
  }

  // The lazy loaded-directory cache: dirPath ("" = root) → DirState.
  const [dirs, setDirs] = useState<Map<string, DirState>>(() => new Map())
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  // The tree owns its ScrollArea and windows rows against that viewport. The
  // element arrives via a callback ref (state, not a plain ref) so the
  // measuring effect below re-runs when it mounts — a mount-only effect would
  // race the loading-spinner state and never attach.
  // Which drop target the pointer is over, by row identity. `null` is "no drag
  // in progress over the tree".
  const [dropKey, setDropKey] = useState<string | null>(null)
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(400)
  // The dirs already requested (loading OR resolved OR errored), so effects
  // never auto-refetch a dir they've already tried. A ref: fetch bookkeeping,
  // not render state. Deliberately NOT cleared on failure (see F9 note in
  // fetchDir's .catch below) — only the explicit Retry button refetches an
  // errored dir.
  const requestedRef = useRef<Set<string>>(new Set())
  // Unmount guard plus a per-dir request counter, so a stale response (the
  // same dir refetched again before the first call resolves, or the
  // component having unmounted) never overwrites fresher state.
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
        .tree(sessionId, dir)
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
          // Deliberately do NOT delete `dir` from requestedRef here: doing so
          // used to make the automatic dirsToLoadFor("missing ancestors")
          // effect below treat an errored dir as still-needing-a-fetch on
          // every subsequent `dirs` change, retrying it forever with no
          // backoff. Leaving it in requestedRef means only an explicit Retry
          // click (which calls fetchDir directly, bypassing requestedRef)
          // refetches an errored dir.
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
    [sessionId],
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
    // Mount-only: the editor body remounts per session/open, and initialPath is
    // fixed for a mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // The open file's parent-refetch (below) is gated by this ref so it fires
  // at most once per opened file, regardless of how many times the effect
  // re-runs as `dirs` changes (guards against a refetch loop when the file
  // genuinely isn't on disk, e.g. opened from a stale changed-files row). The
  // "missing ancestors" fetch above it is separately guarded: fetchDir never
  // clears a dir from requestedRef on failure (see F9 note in fetchDir), so
  // dirsToLoadFor stops treating an errored ancestor as "missing" after its
  // first attempt.
  const revealCheckedRef = useRef<string | null>(null)
  // Auto-expanding the open file's ancestor chain is also one-shot per
  // openPath: without this latch, every `dirs` change (e.g. fetching an
  // unrelated sibling directory) re-runs this effect and re-adds the
  // ancestors to `expanded`, silently overriding a user who had manually
  // collapsed one of them. After the initial reveal, the user's collapse wins.
  const autoExpandedRef = useRef<string | null>(null)

  // When the open file changes to a path whose parents were never loaded (a
  // file just created, or switched to from search), pull the missing ancestors
  // and expand them so the file is visible in the tree. A parent that IS
  // loaded but doesn't list the file (it was just created) is refetched once.
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

  // Post-mutation revalidation: a create/rename/delete landed on the server,
  // so force-refetch the affected dir(s) (bypassing requestedRef, same escape
  // hatch the Retry button uses) and make sure each is expanded so a newly
  // created entry is actually visible without an extra click.
  useEffect(() => {
    if (!revalidate) return
    for (const d of revalidate.dirs) {
      requestedRef.current.delete(d)
      // `fetchDir` seeds `{ status: "loading" }` via `setDirs` before it ever
      // fetches. The lint's static call-graph tracing flags this as a
      // set-state-in-effect even though it's the same escape-hatch pattern
      // the Retry button already uses to force a refetch (see EditorOverlay
      // for the identical, already-accepted disable).
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
    // Only the nonce should retrigger this: `dirs` is intentionally excluded
    // (it would refetch on every unrelated directory load) and `fetchDir` is
    // stable per sessionId.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revalidate?.nonce])

  const toggle = useCallback(
    (path: string, expandable: boolean) => {
      if (expanded.has(path)) {
        // Collapsing: evict this dir's cached listing and any loaded/loading/
        // errored descendants so a huge subtree doesn't linger in memory
        // forever. Re-expanding later refetches fresh data.
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

  // Re-flattens the whole visible tree on any `dirs`/`expanded` change,
  // including ones unrelated to what's currently rendered (O(n) in loaded
  // node count). Accepted cost: the list is virtualized below so render work
  // is bounded by viewport size regardless of flatten cost, and collapse now
  // evicts subtrees (see `toggle`), which keeps `dirs` itself bounded too.
  const rows = useMemo(() => flattenLazy(dirs, expanded), [dirs, expanded])

  const rootState = dirs.get("")

  const totalHeight = rows.length * ROW_HEIGHT

  const firstVisible = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
  const lastVisible = Math.min(
    rows.length - 1,
    Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN,
  )
  const visibleRows = rows.slice(firstVisible, lastVisible + 1)

  // The four native drag handlers one drop target needs, for the row
  // identified by `key` delivering into `dir`.
  //
  // `stopPropagation` is what keeps a row and the root filler underneath it
  // from both claiming the same drag: the filler wraps every row, so without
  // it the bubbling event would reach the filler LAST and the root would win
  // every time, quietly retargeting a folder drop to the worktree root.
  //
  // `preventDefault` on dragover is not optional either: without it the
  // browser refuses the drop and then NAVIGATES to the dropped file, throwing
  // the editor away.
  //
  // Clearing on dragleave without a depth counter is deliberate. A leave fired
  // while crossing into a child element self-heals on the very next dragover
  // (which fires continuously), so the worst case is one frame of missing
  // highlight rather than the stuck highlight a mismatched counter leaves
  // behind.
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
          // Sorted HERE because this is the only place the `DataTransfer` is
          // reachable, and reported unconditionally: a drop that produced
          // neither a file nor a folder still has to say so, or letting go of
          // a folder looks like letting go of nothing.
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

  // The highlight on the row the drop would land in. Returned as CLASSES so a
  // caller can merge them into whatever the element already carries, and
  // through tokens rather than literal colours.
  //
  // There used to be a `data-drop-target` attribute beside this that no CSS
  // ever read: the highlight tests asserted the attribute, so deleting every
  // `dropClass` call and leaving the tree visually inert kept them green. The
  // attribute is gone and the tests assert the classes, which is the thing the
  // user can actually see.
  const dropClass = (key: string) =>
    dropKey === key && "bg-primary/10 ring-1 ring-primary"

  // The tree renders inside its OWN ScrollArea and windows rows against that
  // viewport. Virtualizing against any other element breaks silently: the
  // window only moves on scroll events from the element it measures, so if an
  // ancestor scrolls instead, everything past the first screenful renders as
  // empty spacer. The loading/error/empty states render inside the same
  // ScrollArea so the viewport element exists from the first paint.
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
          click reaches it synchronously (the browser's user activation is
          spent by the time a promise resolves). */}
      {pickerInput}
      <ContextMenu>
      <ContextMenuTrigger
        render={
          <div
            data-testid="file-tree-drop-surface"
            // A right-click that lands directly on this filler (not bubbled
            // up from a row's own trigger, which stops propagation before it
            // gets here) opens the root menu: New File…/New Folder… at the
            // worktree root. `min-h-full` covers the empty space below the
            // last row so a click there still hits this trigger.
            //
            // It is the drop target for the same space and for the same
            // reason: a drop on empty tree space means the worktree root.
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
                        // Stop the native contextmenu event from bubbling to
                        // the root trigger above: this row's own trigger
                        // (attached to this same element) already opens its
                        // menu, so without this the root menu would ALSO try
                        // to open from the same right-click.
                        onContextMenu={(e) => e.stopPropagation()}
                        aria-expanded={expanded.has(row.path)}
                        // Dropping ON a folder puts the files IN it. Routed
                        // through the same mapping the file row uses rather
                        // than passing `row.path` straight in: it is the one
                        // place that answers "which directory does this row
                        // mean", and two rows answering it two ways is how the
                        // two drift.
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
                        // targets the folder the file is IN. The same mapping
                        // every other destination-taking tree action uses.
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
