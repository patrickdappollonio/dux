import { useState, type ReactElement } from "react"
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  Ellipsis,
  EllipsisVertical,
  FileCode2,
  GitCommitVertical,
  Loader2,
  Minus,
  FolderOpen,
  MousePointerClick,
  PanelRightClose,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Square,
  SquareCheck,
  TriangleAlert,
  Undo2,
} from "lucide-react"
import { notifyError } from "@/lib/notify"
import { git } from "@/lib/git"
import { ConfirmDiscardFilesDialog } from "@/components/ConfirmDiscardFilesDialog"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Checkbox } from "@/components/ui/checkbox"
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
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import {
  changesQuietReason,
  folderWorkspace,
  supportsBranchGit,
} from "@/lib/agentWorkspace"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Separator } from "@/components/ui/separator"
import {
  fileStatusMeta,
  formatRecapCount,
  type ChangedFileSelection,
  type ChangedFilesRecap,
} from "@/lib/changedFiles"
import {
  forceRefreshChanges,
  openCommit,
  openDiscard,
  openEditor,
  refreshChanges,
  standaloneEditorHash,
  toggleChangesPane,
  useDux,
} from "@/lib/store"
import type { ChangesSlice } from "@/lib/store"
import { useIsMobile } from "@/hooks/use-mobile"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
import { cn } from "@/lib/utils"
import type { ChangedFileView, SessionView } from "@/lib/types"
import { agentRoot } from "@/lib/editorRoot"
import { formatRegularCount } from "@/lib/formatRegularCount"
import {
  useChangedFilesController,
  type ChangesBulkVerb,
  type ChangesBusyAction,
} from "@/components/useChangedFilesController"

// One height for every control in the bulk bar; they differ in width only.
const BULK_CONTROL = "h-9 max-md:h-11"

// The row's one leading slot: the status marker and the selection checkbox
// stacked in the same box, fixed on both axes so which one shows can never shift
// the path sideways.
//
// Mouse: a 20px box, centring the 14px glyph and the 16px checkbox, with the
// checkbox's own click halo suppressed (`after:hidden` below) so a near-miss
// lands on the row's open-diff click rather than an invisible target. A
// touchscreen reporting a fine pointer lands here too, where a fingertip inside
// the 16px box ticks the row instead of opening the diff.
const STATUS_SLOT = "size-5 pointer-coarse:size-11"
// Touch: the slot is the selection control, since a finger cannot hover, so it
// carries the 44px floor on both axes. Its only neighbours are the row's path
// and the row itself, where a stray tap costs a read-only diff.

interface StatusSlotProps {
  status: string
  path: string
  selected: boolean
  onToggleSelected: (path: string) => void
}

// The status marker, which becomes the selection checkbox on hover, on keyboard
// focus of that checkbox, or while the row is checked.
//
// The checkbox is always in the DOM and always focusable, since the swap is
// opacity and never `display`, so a keyboard can reach it on an unhovered row;
// the marker keeps its `role="img"` label throughout for the same reason. The
// tooltip sits on the slot rather than the marker, so hovering reveals the
// checkbox and shows the status word at once.
function StatusSlot({ status, path, selected, onToggleSelected }: StatusSlotProps) {
  const { label } = fileStatusMeta(status)
  // Same duration and easing as the row's trailing ellipsis wrapper, so both
  // things a hover reveals arrive together.
  const reveal = "transition-opacity duration-200 ease-out motion-reduce:transition-none"
  // Keyboard focus of the checkbox reveals it, never focus-within on the row:
  // focus-within also fires when the ellipsis menu hands focus back to its
  // trigger, stranding one row with a checkbox and no marker.
  const keyboard = "group-has-[[data-slot=checkbox]:focus-visible]:"

  return (
    // The tooltip belongs to the slot, not the marker: the marker is
    // pointer-transparent and fades out on the very hover that would open its
    // tooltip, so one attached to it never fires.
    <SimpleTooltip content={label}>
      {/* The click stops here: base-ui re-dispatches the root's click onto its
        * hidden input and both bubble, so a tick would also open the diff.
        * There is deliberately no shift-click range, because the rows carry no
        * keyboard model for one to be reachable through. */}
      <div
        className={cn(
          "relative flex shrink-0 items-center justify-center",
          STATUS_SLOT,
        )}
        onClick={(e) => e.stopPropagation()}
      >
        <span
          className={cn(
            "pointer-events-none absolute inset-0 flex items-center justify-center",
            reveal,
            "group-hover:opacity-0",
            `${keyboard}opacity-0`,
            selected && "opacity-0",
          )}
        >
          <FileStatusIcon status={status} tooltip={false} />
        </span>
        <Checkbox
          checked={selected}
          onCheckedChange={() => onToggleSelected(path)}
          aria-label={`Select ${path}`}
          className={cn(
            reveal,
            "opacity-0 group-hover:opacity-100",
            `${keyboard}opacity-100`,
            selected && "opacity-100",
            // On touch the halo is the tap target, grown to fill the slot: the
            // pseudo-element is sized from the 14px padding box, so 15px a side
            // makes 44px. On a mouse it is suppressed so it cannot reach past
            // the slot into the path.
            "after:hidden pointer-coarse:after:block pointer-coarse:after:-inset-[15px]",
          )}
        />
      </div>
    </SimpleTooltip>
  )
}

interface FileRowProps {
  file: ChangedFileView
  action: "stage" | "unstage"
  sessionId: string
  selected: boolean
  onToggleSelected: (path: string) => void
  onOpenDiff: (path: string) => void
}

function FileRow({
  file,
  action,
  sessionId,
  selected,
  onToggleSelected,
  onOpenDiff,
}: FileRowProps) {
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
      notifyError(err instanceof Error ? err.message : "git operation failed")
    } finally {
      setBusy(false)
    }
  }

  // Discard is offered on unstaged files only, mirroring the TUI. An untracked
  // file is deleted and a tracked one restored; the dialog distinguishes them.
  function runDiscard() {
    openDiscard({
      sessionId,
      path: file.path,
      untracked: kind === "untracked",
    })
  }

  return (
    <div
      role="row"
      className="group flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-muted max-md:min-h-11"
      onClick={() => onOpenDiff(file.path)}
    >
      {/* Leading slot: the status marker, which becomes the selection checkbox
        * on hover, on focus of the checkbox, or while the row is checked. */}
      <StatusSlot
        status={file.status}
        path={file.path}
        selected={selected}
        onToggleSelected={onToggleSelected}
      />

      {/* Path and counts share one baseline container: their line boxes differ,
        * so under the row's items-center the digits read as superscript. */}
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        {/* Long paths ellipsize at the start (direction:rtl) so the filename
          * stays visible, with text-left keeping short paths left-aligned. The
          * path is a <bdi> LTR isolate, or the rtl container reorders a dotfile
          * path's leading "." onto the end of the filename. */}
        <span className="min-w-0 flex-1 truncate text-left font-mono text-sm text-foreground [direction:rtl]">
          <bdi dir="ltr">{file.path}</bdi>
        </span>

        {/* Additions and deletions, coloured to match the diff viewer's gutter.
          * Binary files report none. */}
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
      </div>

      {/* The wrapper consumes no width until the row is hovered, the menu is
        * open, or an action is in flight (aria-busy, so the spinner outlives the
        * menu), so the path and counts otherwise use the full row. Always
        * visible on touch at a 44px target. The stopPropagation keeps clicks on
        * the trigger and on the portaled menu items off the row's open-diff
        * handler, which React routes through this ancestor. */}
      <div
        className={cn(
          "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none md:max-w-0 md:opacity-0 md:group-hover:max-w-10 md:group-hover:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100 md:has-[[aria-busy=true]]:max-w-10 md:has-[[aria-busy=true]]:opacity-100",
          ALWAYS_REVEALED_ON_TOUCH,
        )}
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
            {/* Open in editor, desktop only (Monaco is poor on touch). Skipped
                for deleted files (nothing on disk to edit). */}
            {kind !== "deleted" && (
              <DropdownMenuItem
                className="hidden md:flex"
                onClick={() => openEditor(agentRoot(sessionId), file.path)}
              >
                <Pencil />
                Edit
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={() => void runAction()}>
              {action === "stage" ? <Plus /> : <Minus />}
              {action === "stage" ? "Stage" : "Unstage"}
            </DropdownMenuItem>
            {/* Discard, on unstaged rows only. Destructive, so the trailing "…"
              * and the confirm dialog carry the danger and the item itself
              * stays neutral. */}
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
  // Summed over `files`, the filtered set, so the recap describes exactly the
  // rows visible beneath it.
  recap: ChangedFilesRecap
  filtering: boolean
  action: "stage" | "unstage"
  sessionId: string
  selected: Set<string>
  onToggleSelected: (path: string) => void
  onOpenDiff: (path: string) => void
}

// What a recap says out loud: the glyphs are a dense column of figures, so the
// spoken form spells the numbers out. It keeps the full number where the glyphs
// abbreviate, with no thousands separators, matching the figures elsewhere.
function recapLabel(scope: string, recap: ChangedFilesRecap): string {
  const lines = (n: number, verb: string) =>
    `${formatRegularCount(n, "line")} ${verb}`
  const parts: string[] = []
  if (recap.additions > 0) parts.push(lines(recap.additions, "added"))
  if (recap.deletions > 0) parts.push(lines(recap.deletions, "removed"))
  if (recap.binaryCount > 0) {
    parts.push(formatRegularCount(recap.binaryCount, "binary file"))
  }
  return `${scope}: ${parts.join(", ")}`
}

// The aggregate for a set of rows, rendered above them. It reuses the row's own
// +/- classes so the header and its rows read as one column of figures, with no
// thousands separators, because the rows carry none.
//
// Line sums of a thousand or more abbreviate, because this figure sits on a
// heading and is there to give a sense of scale; the file count and the binary
// marker stay raw, and the aria-label keeps the full numbers.
//
// Binary files contribute no lines, so they are counted apart in a quiet marker
// rather than pulling the sums toward zero.
function ChangesRecap({
  scope,
  recap,
  className,
}: {
  scope: string
  recap: ChangedFilesRecap
  className?: string
}) {
  const { additions, deletions, binaryCount } = recap
  const hasLines = additions > 0 || deletions > 0
  // Nothing to say: an empty set, or one whose files changed no lines and are
  // not binary either (a mode change, an empty new file). No "+0 −0".
  if (!hasLines && binaryCount === 0) return null

  return (
    <span
      className={cn("shrink-0 font-mono text-xs", className)}
      aria-label={recapLabel(scope, recap)}
    >
      {additions > 0 && (
        <span className="text-green-500">+{formatRecapCount(additions)}</span>
      )}
      {additions > 0 && deletions > 0 && " "}
      {deletions > 0 && (
        <span className="text-red-500">−{formatRecapCount(deletions)}</span>
      )}
      {binaryCount > 0 && (
        <span className="text-muted-foreground">
          {hasLines ? " · " : ""}
          {binaryCount} bin
        </span>
      )}
    </span>
  )
}

function FileGroup({
  heading,
  files,
  total,
  recap,
  filtering,
  action,
  sessionId,
  selected,
  onToggleSelected,
  onOpenDiff,
}: FileGroupProps) {
  const [open, setOpen] = useState(true)

  // A group empty in the source is hidden, and so is one whose files all fail
  // the filter; the empty state below covers no matches anywhere.
  if (files.length === 0) return null

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      {/* No checkbox here: the whole-list selection is the bulk bar's Select
          all / Select none, which spans both sections at once. */}
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded px-1 py-1 text-sm font-medium hover:bg-muted max-md:min-h-11">
        <span className="flex-1 text-left">{heading}</span>
        <ChangesRecap scope={heading} recap={recap} />
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
              selected={selected.has(f.path)}
              onToggleSelected={onToggleSelected}
              onOpenDiff={onOpenDiff}
            />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}

// The direct route to the editor for the agent whose changes are on screen. One
// button, one act: the in-page overlay on a computer, the same act as the menus'
// "Open editor here", so there is no second way to keep in step. The new-tab
// variant stays a menu item.
//
// On a phone the overlay does not exist, so this is a real `<a>` to the
// standalone editor's address, which also keeps long-press and middle-click
// doing what the browser makes them do.
//
// It matches the `⋯` on geometry and is quieter than it: this control navigates
// rather than acting, and the header's one outline control is the menu of acts.
function OpenEditorButton({
  sessionId,
  isMobile,
}: {
  sessionId: string
  isMobile: boolean
}) {
  const root = agentRoot(sessionId)
  const label = "Open editor"
  const shared = {
    size: "icon",
    variant: "ghost",
    "aria-label": label,
    className: "max-md:size-11",
  } as const
  return (
    <SimpleTooltip content={label}>
      {isMobile ? (
        <Button
          {...shared}
          // It really is an anchor, so the primitive is told not to expect a
          // native <button>: a link keeps a link's own semantics and gestures.
          nativeButton={false}
          render={
            <a
              href={standaloneEditorHash(root)}
              target="_blank"
              rel="noopener"
            />
          }
        >
          <FileCode2 />
        </Button>
      ) : (
        <Button {...shared} onClick={() => openEditor(root)}>
          <FileCode2 />
        </Button>
      )}
    </SimpleTooltip>
  )
}

interface ChangesHeaderProps {
  sessionId: string
  stagedCount: number
  // Summed over both groups' VISIBLE rows, the same rule the group headers
  // follow: the pane's recap describes exactly what is on screen under it.
  recap: ChangedFilesRecap
  branchGit: boolean
  isMobile: boolean
}

function ChangesHeader({
  sessionId,
  stagedCount,
  recap,
  branchGit,
  isMobile,
}: ChangesHeaderProps) {
  const runGit = (operation: "push" | "pull") => {
    git[operation](sessionId).catch((error) =>
      notifyError(
        error instanceof Error ? error.message : `${operation} failed`,
      ),
    )
  }

  return (
    <CardHeader className="flex items-center justify-between gap-2 border-b">
      <div className="flex min-w-0 items-baseline gap-2">
        <CardTitle className="shrink-0">Changes</CardTitle>
        {/* The pane's recap is the one figure with a control beside it: the
          * header is a two-cell grid whose second cell is the ⋯ trigger, so the
          * recap gives way, ellipsizing to nothing while the title stays whole
          * and the aria-label keeps saying it. The group headings need none of
          * this, their badge being inside the same shrinking row. */}
        <ChangesRecap
          scope="Changes"
          recap={recap}
          className="min-w-0 shrink truncate"
        />
      </div>
      {/* The cell is a row of its own: `gap-2` is the misclick spacing between
        * the editor button and the `⋯`, adjacent icon squares of one size. */}
      <CardAction className="flex shrink-0 items-center gap-2 self-center">
        <OpenEditorButton sessionId={sessionId} isMobile={isMobile} />
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
              onClick={() => openCommit(sessionId)}
              disabled={stagedCount === 0}
            >
              <GitCommitVertical />
              Commit…
            </DropdownMenuItem>
            {branchGit ? (
              <>
                <DropdownMenuItem onClick={() => runGit("push")}>
                  <ArrowUpFromLine />
                  Push
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => runGit("pull")}>
                  <ArrowDownToLine />
                  Pull
                </DropdownMenuItem>
              </>
            ) : null}
            <DropdownMenuItem
              onClick={() => {
                void forceRefreshChanges().catch((error) =>
                  notifyError(
                    error instanceof Error ? error.message : "refresh failed",
                  ),
                )
              }}
            >
              <RefreshCw />
              Refresh changes
            </DropdownMenuItem>
            {!isMobile ? (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => toggleChangesPane()}>
                  <PanelRightClose />
                  Hide Changes pane
                </DropdownMenuItem>
              </>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
      </CardAction>
    </CardHeader>
  )
}

interface BulkToolbarProps {
  selected: ChangedFileSelection
  busy: ChangesBusyAction
  visibleCount: number
  allVisibleChecked: boolean
  onRunBulk: (verb: ChangesBulkVerb) => void
  onDiscard: () => void
  onToggleVisible: () => void
  onClear: () => void
}

function BusyGlyph({ busy }: { busy: boolean }) {
  return busy ? <Loader2 className="motion-safe:animate-spin" /> : null
}

function BulkToolbar({
  selected,
  busy,
  visibleCount,
  allVisibleChecked,
  onRunBulk,
  onDiscard,
  onToggleVisible,
  onClear,
}: BulkToolbarProps) {
  return (
    <div
      role="toolbar"
      aria-label="Actions for the selected files"
      className="flex flex-wrap items-center gap-2 border-b p-2"
    >
      {selected.unstaged.size > 0 ? (
        <Button
          variant="outline"
          className={BULK_CONTROL}
          disabled={busy !== null}
          aria-busy={busy === "stage"}
          onClick={() => onRunBulk("stage")}
        >
          <BusyGlyph busy={busy === "stage"} />
          {busy !== "stage" ? <Plus /> : null}
          Stage {selected.unstaged.size}
        </Button>
      ) : null}
      {selected.staged.size > 0 ? (
        <Button
          variant="outline"
          className={BULK_CONTROL}
          disabled={busy !== null}
          aria-busy={busy === "unstage"}
          onClick={() => onRunBulk("unstage")}
        >
          <BusyGlyph busy={busy === "unstage"} />
          {busy !== "unstage" ? <Minus /> : null}
          Unstage {selected.staged.size}
        </Button>
      ) : null}
      {selected.unstaged.size > 0 ? (
        <Button
          variant="outline"
          className={BULK_CONTROL}
          disabled={busy !== null}
          aria-busy={busy === "discard"}
          onClick={onDiscard}
        >
          <BusyGlyph busy={busy === "discard"} />
          {busy !== "discard" ? <Undo2 /> : null}
          Discard {selected.unstaged.size}…
        </Button>
      ) : null}
      {visibleCount > 0 ? (
        <Button
          variant="outline"
          className={BULK_CONTROL}
          onClick={onToggleVisible}
        >
          {allVisibleChecked ? <Square /> : <SquareCheck />}
          {allVisibleChecked ? "Select none" : "Select all"}
        </Button>
      ) : null}
      <Button variant="outline" className={BULK_CONTROL} onClick={onClear}>
        Clear
      </Button>
    </div>
  )
}

interface ChangesListProps {
  changed: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  filtered: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  recap: { staged: ChangedFilesRecap; unstaged: ChangedFilesRecap }
  selected: ChangedFileSelection
  sessionId: string
  query: string
  filtering: boolean
  branchGit: boolean
  onToggle: (section: "staged" | "unstaged", path: string) => void
}

function ChangesList({
  changed,
  filtered,
  recap,
  selected,
  sessionId,
  query,
  filtering,
  branchGit,
  onToggle,
}: ChangesListProps) {
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0
  const hasMatches = filtered.staged.length > 0 || filtered.unstaged.length > 0
  const openDiff = (path: string) =>
    openEditor(agentRoot(sessionId), path, "diff")

  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="flex flex-col gap-1 p-3">
        {!hasChanges ? (
          <Empty className="border-0 py-6">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <Check />
              </EmptyMedia>
              <EmptyTitle>No changes</EmptyTitle>
              <EmptyDescription>
                {branchGit ? "This worktree is clean." : "This folder is clean."}
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : null}
        {hasChanges && filtering && !hasMatches ? (
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
        ) : null}
        <FileGroup
          heading="Staged"
          files={filtered.staged}
          total={changed.staged.length}
          recap={recap.staged}
          filtering={filtering}
          action="unstage"
          sessionId={sessionId}
          selected={selected.staged}
          onToggleSelected={(path) => onToggle("staged", path)}
          onOpenDiff={openDiff}
        />
        {filtered.staged.length > 0 && filtered.unstaged.length > 0 ? (
          <Separator className="my-1" />
        ) : null}
        <FileGroup
          heading="Unstaged"
          files={filtered.unstaged}
          total={changed.unstaged.length}
          recap={recap.unstaged}
          filtering={filtering}
          action="stage"
          sessionId={sessionId}
          selected={selected.unstaged}
          onToggleSelected={(path) => onToggle("unstaged", path)}
          onOpenDiff={openDiff}
        />
      </div>
    </ScrollArea>
  )
}

function unavailableChangesScreen(
  sessionId: string | null,
  session: SessionView | undefined,
  changes: ChangesSlice,
): ReactElement | null {
  if (!sessionId) {
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

  const quietReason = session ? changesQuietReason(session.workspace) : null
  if (quietReason) {
    const folder = session ? folderWorkspace(session.workspace) : null
    return (
      <Empty className="h-full border-0">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <FolderOpen />
          </EmptyMedia>
          <EmptyTitle>No changes to show</EmptyTitle>
          <EmptyDescription>{quietReason}</EmptyDescription>
          {folder ? (
            <EmptyDescription className="font-mono break-all">
              {folder.folder_label}
            </EmptyDescription>
          ) : null}
        </EmptyHeader>
      </Empty>
    )
  }

  const slice = changes.sessionId === sessionId ? changes : null
  const phase = slice?.phase ?? "loading"
  if (phase === "loading" || phase === "idle") {
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
  if (phase !== "error") return null

  return (
    <Empty className="h-full border-0">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <TriangleAlert />
        </EmptyMedia>
        <EmptyTitle>Couldn't load changes</EmptyTitle>
        <EmptyDescription>
          {slice?.error ?? "The changed files couldn't be loaded."}
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button
          variant="outline"
          onClick={() => refreshChanges()}
          className="max-md:min-h-11"
        >
          <RefreshCw />
          Refresh
        </Button>
      </EmptyContent>
    </Empty>
  )
}

export function ChangedFiles() {
  const { changes, selectedSessionId, spine } = useDux()
  // The hide-pane action is desktop-only: the mobile hub reaches Changes through
  // its own nav, so there's no panel to hide there.
  const isMobile = useIsMobile()
  const controller = useChangedFilesController(selectedSessionId, changes)

  const selectedSession = spine?.sessions.find(
    (session) => session.id === selectedSessionId,
  )
  const unavailable = unavailableChangesScreen(
    selectedSessionId,
    selectedSession,
    changes,
  )
  if (unavailable) return unavailable
  if (!selectedSessionId) return null

  const branchGit = selectedSession
    ? supportsBranchGit(selectedSession.workspace)
    : true
  const {
    changed,
    filtered,
    recap,
    query,
    filtering,
    selected,
    anySelected,
    visibleCount,
    allVisibleChecked,
    busy,
    discarding,
    setQuery,
    toggleOne,
    toggleVisible,
    clearSelection,
    openDiscard: openDiscardMany,
    closeDiscard: closeDiscardMany,
    runBulk,
    runDiscardMany,
  } = controller
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0
  const sessionId: string = selectedSessionId

  return (
    <>
      <Card className="h-full rounded-none border-0 ring-0">
        <ChangesHeader
          sessionId={sessionId}
          stagedCount={changed.staged.length}
          recap={recap.all}
          branchGit={branchGit}
          isMobile={isMobile}
        />
        <CardContent className="flex min-h-0 flex-1 flex-col p-0">
          {hasChanges ? (
            <div className="border-b p-2">
              <div className="relative">
                <Search className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  type="search"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Filter changed files…"
                  aria-label="Filter changed files"
                  className="h-9 pl-8 max-md:h-11"
                />
              </div>
            </div>
          ) : null}
          {anySelected ? (
            <BulkToolbar
              selected={selected}
              busy={busy}
              visibleCount={visibleCount}
              allVisibleChecked={allVisibleChecked}
              onRunBulk={(verb) => void runBulk(verb)}
              onDiscard={openDiscardMany}
              onToggleVisible={toggleVisible}
              onClear={clearSelection}
            />
          ) : null}
          <ChangesList
            changed={changed}
            filtered={filtered}
            recap={recap}
            selected={selected}
            sessionId={sessionId}
            query={query}
            filtering={filtering}
            branchGit={branchGit}
            onToggle={toggleOne}
          />
        </CardContent>
      </Card>
      <ConfirmDiscardFilesDialog
        open={discarding}
        paths={[...selected.unstaged]}
        unstaged={changed.unstaged}
        onCancel={closeDiscardMany}
        onConfirm={(paths) => void runDiscardMany(paths)}
      />
    </>
  )
}
