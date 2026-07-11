import { useRef, useState, useMemo, useCallback, useEffect } from "react"
import { ChevronRight, File as FileIcon, Loader2, RotateCw } from "lucide-react"
import { cn } from "@/lib/utils"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { fileApi } from "@/lib/fileApi"
import {
  ancestorDirs,
  descendantDirPaths,
  dirsToLoadFor,
  flattenLazy,
} from "@/lib/fileTree"
import type { DirState } from "@/lib/fileTree"

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
  onOpen: (path: string) => void
}

export function FileTree({
  sessionId,
  openPath,
  changed,
  initialPath,
  onOpen,
}: FileTreeProps) {
  // The lazy loaded-directory cache: dirPath ("" = root) → DirState.
  const [dirs, setDirs] = useState<Map<string, DirState>>(() => new Map())
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  const containerRef = useRef<HTMLDivElement>(null)
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
    const el = containerRef.current
    if (!el) return
    setViewportHeight(el.clientHeight)
    const ro = new ResizeObserver(() => setViewportHeight(el.clientHeight))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

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
  if (!rootState || rootState.status === "loading") {
    return (
      <div className="flex items-center justify-center py-4 text-muted-foreground">
        <Loader2 className="size-4 motion-safe:animate-spin" />
      </div>
    )
  }
  if (rootState.status === "error") {
    return (
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
    )
  }
  if (rootState.entries.length === 0) {
    return (
      <p className="px-1 py-2 text-sm text-muted-foreground">
        No files in this worktree.
      </p>
    )
  }

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
          {visibleRows.map((row) =>
            row.isDir ? (
              <li key={row.path}>
                <button
                  type="button"
                  onClick={() => toggle(row.path, row.expandable)}
                  aria-expanded={expanded.has(row.path)}
                  className="flex w-full items-center gap-1 rounded py-1 pr-1 text-left hover:bg-muted"
                  style={{
                    paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                    height: ROW_HEIGHT,
                  }}
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
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">
                    {row.name}
                  </span>
                </button>
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
                <button
                  type="button"
                  onClick={() => onOpen(row.path)}
                  style={{
                    paddingLeft: `${row.depth * 0.75 + 0.25}rem`,
                    height: ROW_HEIGHT,
                  }}
                  className={cn(
                    "flex w-full items-center gap-1.5 rounded py-1 pr-1 text-left hover:bg-muted",
                    row.path === openPath && "bg-muted",
                  )}
                >
                  <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate font-mono text-sm">
                    {row.name}
                  </span>
                  {changed.get(row.path) && (
                    <FileStatusIcon status={changed.get(row.path)!} />
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
