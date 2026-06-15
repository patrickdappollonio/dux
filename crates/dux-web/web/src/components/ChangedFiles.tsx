import { useState } from "react"
import {
  Check,
  Ellipsis,
  EllipsisVertical,
  Loader2,
  Minus,
  MousePointerClick,
  Pencil,
  Plus,
  Search,
  Undo2,
} from "lucide-react"
import { toast } from "sonner"
import { git } from "@/lib/git"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Separator } from "@/components/ui/separator"
import {
  fileStatusMeta,
  filterChangedFiles,
  shouldShowChangedFiles,
} from "@/lib/changedFiles"
import { openCommit, openDiscard, openEditor, useDux } from "@/lib/store"
import type { ChangedFileView } from "@/lib/types"

interface FileRowProps {
  file: ChangedFileView
  action: "stage" | "unstage"
  sessionId: string
  onOpenDiff: (path: string) => void
}

function FileRow({ file, action, sessionId, onOpenDiff }: FileRowProps) {
  const { kind } = fileStatusMeta(file.status)
  const [busy, setBusy] = useState(false)

  async function runAction() {
    setBusy(true)
    try {
      if (action === "stage") {
        await git.stage(sessionId, file.path)
      } else {
        await git.unstage(sessionId, file.path)
      }
      // The file moves staged↔unstaged once the engine's changed-files refresh
      // arrives over the socket; that unmounts this row.
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "git operation failed")
    } finally {
      setBusy(false)
    }
  }

  // Discard is only offered on unstaged files (the "stage" action rows), mirroring
  // the TUI which blocks discarding staged files. An untracked file ("?") will be
  // deleted; a tracked one is restored — the dialog distinguishes them.
  function runDiscard() {
    openDiscard({ sessionId, path: file.path, untracked: kind === "untracked" })
  }

  return (
    <div
      role="row"
      className="group flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-muted max-md:min-h-11"
      onClick={() => onOpenDiff(file.path)}
    >
      {/* Status marker — a neutral file-status icon (shared FileStatusIcon),
          with a tooltip naming the status (Modified/Added/Deleted/…). */}
      <FileStatusIcon status={file.status} />

      {/* File path — monospace (it's a path/code identifier). Long paths
          ellipsize at the START (direction:rtl) so the filename at the end stays
          visible; text-left keeps short paths normally left-aligned. */}
      <span className="min-w-0 flex-1 truncate text-left font-mono text-sm text-foreground [direction:rtl]">
        {file.path}
      </span>

      {/* Additions / deletions (text-only, skip for binary). Added lines green,
          removed lines red, matching the diff viewer's gutter coloring. */}
      {!file.binary && (file.additions > 0 || file.deletions > 0) && (
        <span className="shrink-0 font-mono text-xs">
          {file.additions > 0 && (
            <span className="text-green-500">+{file.additions}</span>
          )}
          {file.additions > 0 && file.deletions > 0 && " "}
          {file.deletions > 0 && (
            <span className="text-red-500">−{file.deletions}</span>
          )}
        </span>
      )}

      {/* Row actions consolidated into a single ⋯ menu (like the sidebar's
          project/session rows). The wrapper consumes NO width until the row is
          hovered, the menu is open (trigger aria-expanded), or an action is in
          flight (trigger aria-busy — so the spinner stays visible after the menu
          closes) — its max-width animates open, so the path/counts use the full
          row otherwise. Always visible on touch at a ≥44px target. The
          stopPropagation keeps clicks on the trigger AND on the (portaled) menu
          items from bubbling to the row's open-diff onClick — React routes portal
          events through this React-tree ancestor. */}
      <div
        className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none md:max-w-0 md:opacity-0 md:group-hover:max-w-10 md:group-hover:opacity-100 md:has-[[aria-expanded=true]]:max-w-10 md:has-[[aria-expanded=true]]:opacity-100 md:has-[[aria-busy=true]]:max-w-10 md:has-[[aria-busy=true]]:opacity-100"
        onClick={(e) => e.stopPropagation()}
      >
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                disabled={busy}
                aria-busy={busy}
                aria-label={`Actions for ${file.path}`}
                className="shrink-0 max-md:size-11"
              />
            }
          >
            {busy ? <Loader2 className="motion-safe:animate-spin" /> : <Ellipsis />}
          </DropdownMenuTrigger>
          <DropdownMenuContent side="bottom" align="end">
            {/* Open in editor — desktop only (Monaco is poor on touch). Skipped
                for deleted files (nothing on disk to edit). */}
            {kind !== "deleted" && (
              <DropdownMenuItem
                className="hidden md:flex"
                onClick={() => openEditor(sessionId, file.path)}
              >
                <Pencil />
                Edit
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={() => void runAction()}>
              {action === "stage" ? <Plus /> : <Minus />}
              {action === "stage" ? "Stage" : "Unstage"}
            </DropdownMenuItem>
            {/* Discard — unstaged rows only (the TUI blocks discarding staged
                files). Destructive: a trailing "…" + a confirm dialog signal the
                danger; the item is left neutral (no red), the … + confirmation
                are the cue. */}
            {action === "stage" && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={runDiscard}>
                  <Undo2 />
                  Discard…
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  )
}

interface FileGroupProps {
  heading: string
  files: ChangedFileView[]
  // The unfiltered group size, so the badge can show "N of M" while a search is
  // active. Equal to `files.length` when nothing is filtered out.
  total: number
  filtering: boolean
  action: "stage" | "unstage"
  sessionId: string
  onOpenDiff: (path: string) => void
}

function FileGroup({ heading, files, total, filtering, action, sessionId, onOpenDiff }: FileGroupProps) {
  const [open, setOpen] = useState(true)

  // Hide a group that's empty in the source. While filtering, a group that has
  // source files but no matches stays hidden too (the overall empty state below
  // covers the "no matches anywhere" case).
  if (files.length === 0) return null

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded px-1 py-1 text-sm font-medium hover:bg-muted max-md:min-h-11">
        <span className="flex-1 text-left">{heading}</span>
        <Badge variant="secondary">
          {filtering ? `${files.length} of ${total}` : files.length}
        </Badge>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 flex flex-col gap-0.5">
          {files.map((f) => (
            <FileRow
              key={f.path}
              file={f}
              action={action}
              sessionId={sessionId}
              onOpenDiff={onOpenDiff}
            />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}

export function ChangedFiles() {
  const { viewModel, selectedSessionId } = useDux()

  // Changed-files search filter (frontend-only). The query is stored alongside
  // the session id it belongs to, so switching sessions yields an empty filter
  // without a set-state-in-effect: a stale entry (different session) reads as "".
  const [search, setSearch] = useState<{ sessionId: string; query: string }>({
    sessionId: "",
    query: "",
  })
  const query = search.sessionId === selectedSessionId ? search.query : ""
  const setQuery = (next: string) =>
    setSearch({ sessionId: selectedSessionId ?? "", query: next })

  // No session selected — muted empty state.
  if (!selectedSessionId) {
    return (
      <Empty className="h-full border-0">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <MousePointerClick />
          </EmptyMedia>
          <EmptyTitle>No session selected</EmptyTitle>
          <EmptyDescription>Select a session to see its changes.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    )
  }

  // The changed-files lists are GLOBAL engine state tagged with the session they
  // belong to. Only trust them when they match this client's selection; until
  // the server's watch catches up (selection just changed, or a reconnect is
  // re-establishing it) show a loading state rather than another session's files.
  const watchedSessionId = viewModel?.changed_files.watched_session_id ?? null
  const ready = shouldShowChangedFiles(watchedSessionId, selectedSessionId)
  if (!ready) {
    return (
      <Empty className="h-full border-0">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <Loader2 className="animate-spin" />
          </EmptyMedia>
          <EmptyTitle>Loading changes…</EmptyTitle>
          <EmptyDescription>Fetching this session's changes.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    )
  }

  const changed = viewModel?.changed_files ?? {
    staged: [],
    unstaged: [],
    watched_session_id: null,
  }
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0

  const filtering = query.trim() !== ""
  const filteredStaged = filterChangedFiles(changed.staged, query)
  const filteredUnstaged = filterChangedFiles(changed.unstaged, query)
  const hasMatches = filteredStaged.length > 0 || filteredUnstaged.length > 0
  const showSeparator = filteredStaged.length > 0 && filteredUnstaged.length > 0

  return (
    <>
      {/* Main card filling the pane */}
      <Card className="h-full rounded-none border-0 ring-0">
        {/* The git actions collapse into a single "Actions" menu so the header
            never overflows when the pane is narrow (e.g. on a tablet). */}
        <CardHeader className="flex items-center justify-between gap-2 border-b">
          <CardTitle>Changes</CardTitle>
          <CardAction className="self-center">
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    size="icon"
                    variant="outline"
                    aria-label="Changes actions"
                    className="max-md:size-11"
                  />
                }
              >
                <EllipsisVertical />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onClick={() => openCommit(selectedSessionId)}
                  disabled={changed.staged.length === 0}
                >
                  Commit…
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => {
                    if (!selectedSessionId) return
                    git
                      .push(selectedSessionId)
                      .catch((e) =>
                        toast.error(e instanceof Error ? e.message : "push failed")
                      )
                  }}
                >
                  Push
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => {
                    if (!selectedSessionId) return
                    git
                      .pull(selectedSessionId)
                      .catch((e) =>
                        toast.error(e instanceof Error ? e.message : "pull failed")
                      )
                  }}
                >
                  Pull
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </CardAction>
        </CardHeader>

        <CardContent className="flex min-h-0 flex-1 flex-col p-0">
          {/* Compact case-insensitive search over both lists. Only shown when
              there are changes to filter. Sized ≥44px tall for touch. */}
          {hasChanges && (
            <div className="border-b p-2">
              <div className="relative">
                <Search className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  type="search"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Filter changed files…"
                  aria-label="Filter changed files"
                  className="h-9 pl-8 max-md:h-11"
                />
              </div>
            </div>
          )}

          <ScrollArea className="min-h-0 flex-1">
            <div className="flex flex-col gap-1 p-3">
              {!hasChanges && (
                <Empty className="border-0 py-6">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <Check />
                    </EmptyMedia>
                    <EmptyTitle>No changes</EmptyTitle>
                    <EmptyDescription>This worktree is clean.</EmptyDescription>
                  </EmptyHeader>
                </Empty>
              )}

              {hasChanges && filtering && !hasMatches && (
                <Empty className="border-0 py-6">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <Search />
                    </EmptyMedia>
                    <EmptyTitle>No matching files</EmptyTitle>
                    <EmptyDescription>
                      No changed file matches “{query.trim()}”.
                    </EmptyDescription>
                  </EmptyHeader>
                </Empty>
              )}

              <FileGroup
                heading="Staged"
                files={filteredStaged}
                total={changed.staged.length}
                filtering={filtering}
                action="unstage"
                sessionId={selectedSessionId}
                onOpenDiff={(path) => openEditor(selectedSessionId, path, "diff")}
              />

              {showSeparator && <Separator className="my-1" />}

              <FileGroup
                heading="Unstaged"
                files={filteredUnstaged}
                total={changed.unstaged.length}
                filtering={filtering}
                action="stage"
                sessionId={selectedSessionId}
                onOpenDiff={(path) => openEditor(selectedSessionId, path, "diff")}
              />
            </div>
          </ScrollArea>
        </CardContent>
      </Card>
    </>
  )
}
