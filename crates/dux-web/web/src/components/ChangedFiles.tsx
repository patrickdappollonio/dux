import { useState } from "react"
import { Button } from "@/components/ui/button"
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
import { cn } from "@/lib/utils"

// Map raw git status codes to a short display glyph.
function statusGlyph(status: string): string {
  switch (status.toUpperCase()) {
    case "M": return "M"
    case "A": return "A"
    case "D": return "D"
    case "?": return "?"
    case "??": return "?"
    default: return status.slice(0, 1).toUpperCase() || "?"
  }
}

// Truncate a path on the left if it is too long, preserving the filename.
function truncatePath(path: string, maxLen = 36): string {
  if (path.length <= maxLen) return path
  return "…" + path.slice(path.length - (maxLen - 1))
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
      className="group flex cursor-pointer items-center gap-1.5 rounded px-1.5 py-1 text-xs hover:bg-muted"
      onClick={() => onOpenDiff(file.path)}
    >
      <span
        className={cn(
          "w-3 shrink-0 font-mono font-semibold",
          glyph === "A" && "text-emerald-500",
          glyph === "D" && "text-red-500",
          glyph === "M" && "text-amber-500",
          glyph === "?" && "text-muted-foreground"
        )}
      >
        {glyph}
      </span>

      <span className="min-w-0 flex-1 truncate font-mono text-foreground">
        {truncatePath(file.path)}
      </span>

      {!file.binary && (file.additions > 0 || file.deletions > 0) && (
        <span className="shrink-0 font-mono text-muted-foreground">
          {file.additions > 0 && (
            <span className="text-emerald-500">+{file.additions}</span>
          )}
          {file.additions > 0 && file.deletions > 0 && (
            <span className="text-muted-foreground">/</span>
          )}
          {file.deletions > 0 && (
            <span className="text-red-500">−{file.deletions}</span>
          )}
        </span>
      )}

      <Button
        variant="ghost"
        size="sm"
        className="h-5 shrink-0 cursor-pointer px-1.5 py-0 text-xs opacity-0 group-hover:opacity-100"
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
  if (files.length === 0) return null
  return (
    <div className="flex flex-col gap-0.5">
      <h3 className="px-1.5 py-1 text-[0.65rem] font-semibold uppercase tracking-wide text-muted-foreground">
        {heading} ({files.length})
      </h3>
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
  )
}

export function ChangedFiles() {
  const { viewModel, selectedSessionId } = useDux()
  const [diffPath, setDiffPath] = useState<string | null>(null)

  if (!selectedSessionId) {
    return (
      <div className="flex h-full items-center justify-center p-4 text-xs text-muted-foreground">
        Select a session to see changes
      </div>
    )
  }

  const changed = viewModel?.changed_files ?? { staged: [], unstaged: [] }
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0

  function handleCommit() {
    if (selectedSessionId) openCommit(selectedSessionId)
  }

  function handlePush() {
    if (selectedSessionId) socket.sendCommand("push", { session_id: selectedSessionId })
  }

  return (
    <>
      <div className="flex h-full min-h-0 flex-col bg-background">
        {/* Header */}
        <div className="flex shrink-0 items-center gap-1.5 border-b border-border px-2 py-1.5">
          <h2 className="flex-1 text-[0.7rem] font-semibold uppercase tracking-wide text-muted-foreground">
            Changes
          </h2>
          <Button
            variant="ghost"
            size="sm"
            className="h-6 cursor-pointer px-2 py-0 text-xs"
            onClick={handleCommit}
            disabled={changed.staged.length === 0}
          >
            Commit…
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="h-6 cursor-pointer px-2 py-0 text-xs"
            onClick={handlePush}
          >
            Push
          </Button>
        </div>

        {/* File list */}
        <ScrollArea className="flex-1">
          <div className="flex flex-col gap-2 p-1.5">
            {!hasChanges && (
              <p className="px-1.5 py-2 text-xs text-muted-foreground">
                No changes.
              </p>
            )}
            <FileGroup
              heading="Staged"
              files={changed.staged}
              action="unstage"
              sessionId={selectedSessionId}
              onOpenDiff={setDiffPath}
            />
            {changed.staged.length > 0 && changed.unstaged.length > 0 && (
              <Separator />
            )}
            <FileGroup
              heading="Unstaged"
              files={changed.unstaged}
              action="stage"
              sessionId={selectedSessionId}
              onOpenDiff={setDiffPath}
            />
          </div>
        </ScrollArea>
      </div>

      {/* Diff sheet */}
      <Sheet open={diffPath !== null} onOpenChange={(open) => { if (!open) setDiffPath(null) }}>
        <SheetContent side="right" className="w-[min(600px,90vw)] sm:max-w-none">
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
