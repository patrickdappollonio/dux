import { useMemo, useState } from "react"
import { Check, MousePointerClick } from "lucide-react"
import { BrailleSpinner } from "@/components/BrailleSpinner"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import { Separator } from "@/components/ui/separator"
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { highlightLine, languageForPath } from "@/lib/highlight"
import { closeDiff, openCommit, requestDiff, socket, useDux } from "@/lib/store"
import type { DuxState } from "@/lib/store"
import type { ChangedFileView, DiffLine, FileDiff } from "@/lib/types"

// Map raw git status codes to a short display glyph.
function statusGlyph(status: string): string {
  const upper = status.toUpperCase()
  switch (upper) {
    case "M":  return "M"
    case "A":  return "A"
    case "D":  return "D"
    case "?":
    case "??": return "?"
    case "R":  return "R"
    default:   return status.slice(0, 1).toUpperCase() || "?"
  }
}

interface FileRowProps {
  file: ChangedFileView
  action: "stage" | "unstage"
  sessionId: string
  onOpenDiff: (path: string) => void
}

function FileRow({ file, action, sessionId, onOpenDiff }: FileRowProps) {
  const glyph = statusGlyph(file.status)

  function handleAction(e: React.MouseEvent) {
    e.stopPropagation()
    const command = action === "stage" ? "stage_file" : "unstage_file"
    socket.sendCommand(command, { session_id: sessionId, path: file.path })
  }

  return (
    <div
      role="row"
      className="group flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-muted max-md:min-h-11"
      onClick={() => onOpenDiff(file.path)}
    >
      {/* Status glyph */}
      <Badge variant="outline" className="shrink-0 font-mono">
        {glyph}
      </Badge>

      {/* File path */}
      <span className="min-w-0 flex-1 truncate text-xs text-foreground">
        {file.path}
      </span>

      {/* Additions / deletions (text-only, skip for binary) */}
      {!file.binary && (file.additions > 0 || file.deletions > 0) && (
        <span className="shrink-0 text-xs text-muted-foreground">
          {file.additions > 0 && `+${file.additions}`}
          {file.additions > 0 && file.deletions > 0 && " "}
          {file.deletions > 0 && `−${file.deletions}`}
        </span>
      )}

      {/* Stage / Unstage action. Hover-reveal works on desktop, but touch has
          no hover — so on phones the button is always visible and ≥44px tall. */}
      <Button
        variant="ghost"
        size="sm"
        className="shrink-0 opacity-100 max-md:min-h-11 md:opacity-0 md:group-hover:opacity-100"
        onClick={handleAction}
      >
        {action === "stage" ? "Stage" : "Unstage"}
      </Button>
    </div>
  )
}

interface FileGroupProps {
  heading: string
  files: ChangedFileView[]
  action: "stage" | "unstage"
  sessionId: string
  onOpenDiff: (path: string) => void
}

function FileGroup({ heading, files, action, sessionId, onOpenDiff }: FileGroupProps) {
  const [open, setOpen] = useState(true)

  if (files.length === 0) return null

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded px-1 py-1 text-sm font-medium hover:bg-muted max-md:min-h-11">
        <span className="flex-1 text-left">{heading}</span>
        <Badge variant="secondary">{files.length}</Badge>
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
  const { viewModel, selectedSessionId, currentDiff } = useDux()

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

  const changed = viewModel?.changed_files ?? { staged: [], unstaged: [] }
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0
  const showSeparator = changed.staged.length > 0 && changed.unstaged.length > 0

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
              onClick={() => socket.sendCommand("push", { session_id: selectedSessionId })}
            >
              Push
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => socket.sendCommand("pull", { session_id: selectedSessionId })}
            >
              Pull
            </Button>
          </CardAction>
        </CardHeader>

        <CardContent className="flex min-h-0 flex-1 flex-col p-0">
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

              <FileGroup
                heading="Staged"
                files={changed.staged}
                action="unstage"
                sessionId={selectedSessionId}
                onOpenDiff={(path) => requestDiff(selectedSessionId, path)}
              />

              {showSeparator && <Separator className="my-1" />}

              <FileGroup
                heading="Unstaged"
                files={changed.unstaged}
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
          className="data-[side=right]:w-[92vw] data-[side=right]:sm:max-w-3xl"
        >
          <SheetHeader>
            <SheetTitle
              className="truncate font-mono text-sm"
              title={currentDiff?.path ?? ""}
            >
              {currentDiff?.path ?? ""}
            </SheetTitle>
          </SheetHeader>
          <div className="min-h-0 flex-1 overflow-auto px-4 pb-4">
            <DiffBody state={currentDiff} />
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}

type DiffState = DuxState["currentDiff"]

function DiffBody({ state }: { state: DiffState }) {
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
  return <DiffHunks diff={diff} />
}

// Renders the hunk rows for a real (non-binary, non-empty) diff. Lives in its
// own component so its hooks always run in a stable order — DiffBody's early
// returns must not gate the `useMemo`s below (React rules-of-hooks).
function DiffHunks({ diff }: { diff: FileDiff }) {
  const language = useMemo(() => languageForPath(diff.path), [diff.path])
  // Precompute highlighted HTML per line to avoid re-highlighting on re-render.
  const hunks = useMemo(
    () =>
      diff.hunks.map((hunk) => ({
        header: hunk.header,
        lines: hunk.lines.map((line) => ({
          line,
          html: highlightLine(line.content, language),
        })),
      })),
    [diff, language],
  )
  return (
    <div className="overflow-x-auto rounded border font-mono text-xs leading-relaxed">
      {hunks.map((hunk, hi) => (
        <div key={hi}>
          <div className="bg-muted px-2 py-0.5 text-muted-foreground">
            {hunk.header}
          </div>
          {hunk.lines.map(({ line, html }, li) => (
            <DiffRow key={li} line={line} html={html} />
          ))}
        </div>
      ))}
    </div>
  )
}

function DiffRow({ line, html }: { line: DiffLine; html: string }) {
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
      <span className="w-10 shrink-0 select-none px-1 text-right text-muted-foreground">
        {line.old_line ?? ""}
      </span>
      <span className="w-10 shrink-0 select-none px-1 text-right text-muted-foreground">
        {line.new_line ?? ""}
      </span>
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
