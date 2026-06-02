import { useState } from "react"
import { Check, MousePointerClick } from "lucide-react"
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
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { openCommit, socket, useDux } from "@/lib/store"
import type { ChangedFileView } from "@/lib/types"

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
      className="group flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-muted"
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

      {/* Stage / Unstage action */}
      <Button
        variant="ghost"
        size="sm"
        className="shrink-0 opacity-0 group-hover:opacity-100"
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
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded px-1 py-1 text-sm font-medium hover:bg-muted">
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
  const { viewModel, selectedSessionId } = useDux()
  const [diffPath, setDiffPath] = useState<string | null>(null)

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
          <CardAction className="flex items-center gap-1">
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
                onOpenDiff={setDiffPath}
              />

              {showSeparator && <Separator className="my-1" />}

              <FileGroup
                heading="Unstaged"
                files={changed.unstaged}
                action="stage"
                sessionId={selectedSessionId}
                onOpenDiff={setDiffPath}
              />
            </div>
          </ScrollArea>
        </CardContent>
      </Card>

      {/* Diff sheet — placeholder, structured diff lands in a follow-up */}
      <Sheet
        open={diffPath !== null}
        onOpenChange={(open) => { if (!open) setDiffPath(null) }}
      >
        <SheetContent side="right">
          <SheetHeader>
            <SheetTitle className="font-mono text-sm">{diffPath ?? ""}</SheetTitle>
            <SheetDescription>
              Structured diff rendering lands in a follow-up. Staging and the file list work today.
            </SheetDescription>
          </SheetHeader>
        </SheetContent>
      </Sheet>
    </>
  )
}
