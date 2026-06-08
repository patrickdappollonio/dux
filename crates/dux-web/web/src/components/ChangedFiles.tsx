import { useMemo, useState, useSyncExternalStore } from "react"
import {
  Check,
  Hash,
  Loader2,
  Minus,
  MousePointerClick,
  Pencil,
  Plus,
  Search,
  Undo2,
  X,
} from "lucide-react"
import { toast } from "sonner"
import { git } from "@/lib/git"
import { BrailleSpinner } from "@/components/BrailleSpinner"
import { SimpleTooltip } from "@/components/SimpleTooltip"
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
  Sheet,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import {
  getHighlighterReady,
  highlightLine,
  languageForPath,
  subscribeHighlighter,
} from "@/lib/highlight"
import {
  filterChangedFiles,
  shouldShowChangedFiles,
  statusGlyph,
} from "@/lib/changedFiles"
import {
  closeDiff,
  openCommit,
  openDiscard,
  openEditor,
  requestDiff,
  toggleDiffLineNumbers,
  useDux,
} from "@/lib/store"
import type { DuxState } from "@/lib/store"
import type { ChangedFileView, DiffLine, FileDiff } from "@/lib/types"

interface FileRowProps {
  file: ChangedFileView
  action: "stage" | "unstage"
  sessionId: string
  onOpenDiff: (path: string) => void
}

function FileRow({ file, action, sessionId, onOpenDiff }: FileRowProps) {
  const glyph = statusGlyph(file.status)
  const [busy, setBusy] = useState(false)

  async function handleAction(e: React.MouseEvent) {
    e.stopPropagation()
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
  function handleDiscard(e: React.MouseEvent) {
    e.stopPropagation()
    openDiscard({ sessionId, path: file.path, untracked: statusGlyph(file.status) === "?" })
  }

  return (
    <div
      role="row"
      className="group flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-muted max-md:min-h-11"
      onClick={() => onOpenDiff(file.path)}
    >
      {/* Status glyph. leading-none so the single uppercase letter centers in
          the fixed-height badge instead of riding high. */}
      <Badge variant="outline" className="shrink-0 font-mono leading-none">
        {glyph}
      </Badge>

      {/* File path — monospace (it's a path/code identifier). Long paths
          ellipsize at the START (direction:rtl) so the filename at the end stays
          visible; text-left keeps short paths normally left-aligned. */}
      <span className="min-w-0 flex-1 truncate text-left font-mono text-xs text-foreground [direction:rtl]">
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

      {/* Action buttons. On desktop the wrapper consumes NO width until the row
          is hovered — its max-width animates open, so the path/counts use the
          full row otherwise and the content slides left to make room (not a hard
          cut). On touch (no hover) it's always visible at a ≥44px target. */}
      <div className="flex shrink-0 items-center gap-0.5 overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none md:max-w-0 md:opacity-0 md:group-hover:max-w-64 md:group-hover:opacity-100">
        {/* Open in editor — desktop only (Monaco is poor on touch). Skipped for
            deleted files (nothing on disk to edit). */}
        {glyph !== "D" && (
          <Button
            variant="ghost"
            size="sm"
            aria-label={`Open ${file.path} in editor`}
            className="hidden shrink-0 md:inline-flex"
            onClick={(e) => {
              e.stopPropagation()
              openEditor(sessionId, file.path)
            }}
          >
            <Pencil />
            Edit
          </Button>
        )}

        {/* Discard — unstaged rows only (the TUI blocks discarding staged files).
            Destructive-tinted; opens a confirm dialog (it's destructive). */}
        {action === "stage" && (
          <Button
            variant="ghost"
            size="sm"
            aria-label={`Discard changes to ${file.path}`}
            className="shrink-0 text-destructive hover:text-destructive max-md:min-h-11"
            onClick={handleDiscard}
          >
            <Undo2 />
            Discard
          </Button>
        )}

        <Button
          variant="ghost"
          size="sm"
          disabled={busy}
          aria-busy={busy}
          className="shrink-0 max-md:min-h-11"
          onClick={handleAction}
        >
          {busy ? (
            <Loader2 className="motion-safe:animate-spin" />
          ) : action === "stage" ? (
            <Plus />
          ) : (
            <Minus />
          )}
          {action === "stage" ? "Stage" : "Unstage"}
        </Button>
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
  const { viewModel, selectedSessionId, currentDiff, showDiffLineNumbers } =
    useDux()

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
        <CardHeader className="border-b">
          <CardTitle>Changes</CardTitle>
          <CardAction className="flex items-center gap-1 max-md:[&_button]:min-h-11">
            <Button
              size="sm"
              onClick={() => openCommit(selectedSessionId)}
              disabled={changed.staged.length === 0}
            >
              Commit…
            </Button>
            <Button
              size="sm"
              variant="outline"
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
            </Button>
            <Button
              size="sm"
              variant="outline"
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
            </Button>
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

          <ScrollArea className="flex-1">
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
                onOpenDiff={(path) => requestDiff(selectedSessionId, path)}
              />

              {showSeparator && <Separator className="my-1" />}

              <FileGroup
                heading="Unstaged"
                files={filteredUnstaged}
                total={changed.unstaged.length}
                filtering={filtering}
                action="stage"
                sessionId={selectedSessionId}
                onOpenDiff={(path) => requestDiff(selectedSessionId, path)}
              />
            </div>
          </ScrollArea>
        </CardContent>
      </Card>

      {/* Diff sheet — renders the structured per-file diff from the server */}
      <Sheet
        open={currentDiff !== null}
        onOpenChange={(open) => {
          if (!open) closeDiff()
        }}
      >
        {/* Width is set with the same data-[side=right] modifier the Sheet
            primitive uses for its 3/4 default, so tailwind-merge dedupes
            deterministically (a plain w-* would not override the modified one):
            near-full on phones, capped to 3xl on desktop. */}
        <SheetContent
          side="right"
          showCloseButton={false}
          className="data-[side=right]:w-[92vw] data-[side=right]:sm:max-w-3xl"
        >
          <SheetHeader className="flex-row items-center justify-between gap-2">
            {/* Left-ellipsize (direction:rtl) so the filename at the path's end
                stays visible — the title is how you know which file the diff is. */}
            <SimpleTooltip content={currentDiff?.path ?? ""}>
              <SheetTitle className="min-w-0 truncate text-left font-mono text-sm [direction:rtl]">
                {currentDiff?.path ?? ""}
              </SheetTitle>
            </SimpleTooltip>
            {/* Labeled (icon + text) buttons of the same size so they sit on one
                baseline — the diff sheet replaces the Sheet's default icon-only
                close with an explicit "Close" for consistency. */}
            <div className="flex shrink-0 items-center gap-1">
              {/* Open the current file in the editor (desktop only — Monaco is
                  poor on touch). Closes the diff so the editor takes the screen. */}
              <Button
                variant="ghost"
                size="sm"
                className="hidden md:inline-flex"
                onClick={() => {
                  if (!currentDiff) return
                  openEditor(currentDiff.sessionId, currentDiff.path)
                  closeDiff()
                }}
              >
                <Pencil />
                Edit
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className={
                  showDiffLineNumbers
                    ? "max-md:min-h-11 text-primary"
                    : "max-md:min-h-11"
                }
                aria-pressed={showDiffLineNumbers}
                onClick={toggleDiffLineNumbers}
              >
                <Hash />
                Line numbers
              </Button>
              <SheetClose
                render={
                  <Button variant="ghost" size="sm" className="max-md:min-h-11" />
                }
              >
                <X />
                Close
              </SheetClose>
            </div>
          </SheetHeader>
          <div className="min-h-0 flex-1 overflow-auto px-4 pb-4">
            <DiffBody state={currentDiff} showLineNumbers={showDiffLineNumbers} />
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}

type DiffState = DuxState["currentDiff"]

function DiffBody({
  state,
  showLineNumbers,
}: {
  state: DiffState
  showLineNumbers: boolean
}) {
  if (!state) return null
  if (state.loading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <BrailleSpinner className="text-primary" /> Loading diff…
      </div>
    )
  }
  if (state.error) {
    return <div className="py-6 text-sm text-destructive">{state.error}</div>
  }
  const diff = state.diff
  if (!diff) return null
  if (diff.binary) {
    return (
      <div className="py-6 text-sm text-muted-foreground">
        Binary file ({diff.old_size} → {diff.new_size} bytes). No text diff available.
      </div>
    )
  }
  if (diff.unchanged || diff.hunks.length === 0) {
    return <div className="py-6 text-sm text-muted-foreground">No changes.</div>
  }
  return <DiffHunks diff={diff} showLineNumbers={showLineNumbers} />
}

// Renders the hunk rows for a real (non-binary, non-empty) diff. Lives in its
// own component so its hooks always run in a stable order — DiffBody's early
// returns must not gate the `useMemo`s below (React rules-of-hooks).
function DiffHunks({
  diff,
  showLineNumbers,
}: {
  diff: FileDiff
  showLineNumbers: boolean
}) {
  const language = useMemo(() => languageForPath(diff.path), [diff.path])
  // The highlighter (highlight.js) loads lazily in its own async chunk. Subscribe
  // so this component re-renders the moment it's ready; until then highlightLine
  // returns escaped plain text. `highlighterReady` flips false → true once and
  // feeds the useMemo below so the lines re-highlight on arrival.
  const highlighterReady = useSyncExternalStore(
    subscribeHighlighter,
    getHighlighterReady,
  )
  // Precompute highlighted HTML per line to avoid re-highlighting on re-render.
  const hunks = useMemo(
    () =>
      diff.hunks.map((hunk) => ({
        header: hunk.header,
        lines: hunk.lines.map((line) => ({
          line,
          html: highlightLine(line.content, language, highlighterReady),
        })),
      })),
    [diff, language, highlighterReady],
  )
  return (
    <div className="overflow-x-auto rounded border font-mono text-xs leading-relaxed">
      {/* w-max so the block grows to the widest line and every row's +/- tint
          spans the full code width (not just the viewport); min-w-full so short
          diffs still fill the panel. Rows below inherit this width. */}
      <div className="w-max min-w-full">
        {hunks.map((hunk, hi) => (
          <div key={hi}>
            <div className="bg-muted px-2 py-0.5 text-muted-foreground">
              {hunk.header}
            </div>
            {hunk.lines.map(({ line, html }, li) => (
              <DiffRow
                key={li}
                line={line}
                html={html}
                showLineNumbers={showLineNumbers}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  )
}

function DiffRow({
  line,
  html,
  showLineNumbers,
}: {
  line: DiffLine
  html: string
  showLineNumbers: boolean
}) {
  const sign = line.kind === "insert" ? "+" : line.kind === "delete" ? "-" : " "
  const rowClass =
    line.kind === "insert"
      ? "bg-green-600/15"
      : line.kind === "delete"
        ? "bg-red-600/15"
        : ""
  const signClass =
    line.kind === "insert"
      ? "text-green-500"
      : line.kind === "delete"
        ? "text-red-500"
        : "text-muted-foreground"
  return (
    <div className={`flex ${rowClass}`}>
      {/* Old/new line-number gutters hide together when the toggle is off. The
          sign column always stays, so add/delete coloring reads without them. */}
      {showLineNumbers ? (
        <>
          <span className="w-10 shrink-0 select-none px-1 text-right text-muted-foreground">
            {line.old_line ?? ""}
          </span>
          <span className="w-10 shrink-0 select-none px-1 text-right text-muted-foreground">
            {line.new_line ?? ""}
          </span>
        </>
      ) : null}
      <span className={`w-4 shrink-0 select-none text-center ${signClass}`}>
        {sign}
      </span>
      <span
        className="whitespace-pre"
        // Safe: highlight.js escapes the source; `html` is escaped plain text when no language.
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </div>
  )
}
