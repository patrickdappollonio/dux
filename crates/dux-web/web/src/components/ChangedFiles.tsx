import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from "react"
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  ChevronRight,
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
import { notifyError, notifyInfo } from "@/lib/notify"
import { leftOutNotice } from "@/lib/discardOutcome"
import { git } from "@/lib/git"
import { ConfirmDiscardFilesDialog } from "@/components/ConfirmDiscardFilesDialog"
import { FileStatusIcon } from "@/components/FileStatusIcon"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { StartTruncatedText } from "@/components/StartTruncatedText"
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
  changedFileCount,
  discardActsOn,
  stageActsOn,
  fileStatusMeta,
  folderCountLabel,
  formatRecapCount,
  groupDigits,
  uncoveredPaths,
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
  useDuxSelector,
} from "@/lib/store"
import type { ChangesSlice, DuxState } from "@/lib/store"
import {
  buildChangesItems,
  changesListStructure,
  changesRowTree,
  layoutChangesItems,
  visibleChangesIndices,
  type ChangesItemHeights,
  type ChangesItemKind,
  type ChangesListItem,
  type ChangesRowPart,
  type ChangesSection,
} from "@/lib/changesWindow"
import { folderKey, isExpandable, type Expansions } from "@/lib/changesTree"
import {
  expansionsFor,
  reconcileFolders,
  retryFolderChildren,
  toggleFolder,
  useExpansions,
} from "@/lib/changesExpansion"
import { FOCUS_RING } from "@/lib/focusRing"
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
  // Out of the tab order unless its row is the active one.
  inTabOrder: boolean
}

// The status marker, which becomes the selection checkbox on hover, on keyboard
// focus of that checkbox, or while the row is checked.
//
// The checkbox is always in the DOM and always focusable, since the swap is
// opacity and never `display`, so a keyboard can reach it on an unhovered row;
// the marker keeps its `role="img"` label throughout for the same reason. The
// tooltip sits on the slot rather than the marker, so hovering reveals the
// checkbox and shows the status word at once.
function StatusSlot({
  status,
  path,
  selected,
  onToggleSelected,
  inTabOrder,
}: StatusSlotProps) {
  const { label } = fileStatusMeta(status)
  // The checkbox is named by an element it points at rather than by
  // `aria-label`. Without an explicit `aria-labelledby`, base-ui hunts for a
  // wrapping or sibling <label> after EVERY commit, and its last resort reads
  // the hidden input's `labels`, which walks the whole document: once per row
  // per render, so a long list cost O(n^2) and froze the tab. The spoken name
  // is unchanged, "Select <path>".
  const labelId = useId()
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
        role="gridcell"
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
        <span id={labelId} className="sr-only">
          Select {path}
        </span>
        <Checkbox
          checked={selected}
          onCheckedChange={() => onToggleSelected(path)}
          aria-labelledby={labelId}
          tabIndex={inTabOrder ? undefined : -1}
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

// The last segment of a path, keeping a folder's trailing slash.
function leafName(path: string): string {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path
  const leaf = trimmed.slice(trimmed.lastIndexOf("/") + 1)
  return path.endsWith("/") ? `${leaf}/` : leaf
}

// A row's name and counts: on a folder that can expand, a real button that
// expands and collapses it (the checkbox and the ⋯ still act on the whole
// folder); on any other row, a plain container. Its click stops at the button,
// so it never reaches the row. It fills the row's height, so on touch the
// target is the row's 44px, and it spans the row's free width.
function ExpandToggle({
  expandable,
  expanded,
  contentsId,
  inTabOrder,
  onToggle,
  children,
}: {
  expandable: boolean
  expanded: boolean
  contentsId?: string
  inTabOrder: boolean
  onToggle: () => void
  children: React.ReactNode
}) {
  if (!expandable) {
    return (
      <div role="gridcell" className="flex min-w-0 flex-1 items-baseline gap-2">
        {children}
      </div>
    )
  }
  return (
    <div role="gridcell" className="flex min-w-0 flex-1 self-stretch">
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={expanded ? contentsId : undefined}
        tabIndex={inTabOrder ? undefined : -1}
        onClick={(event) => {
          event.stopPropagation()
          onToggle()
        }}
        className={cn(
          "flex min-w-0 flex-1 items-baseline gap-2 self-stretch rounded border border-transparent text-left max-md:min-h-11 pointer-coarse:min-h-11",
          FOCUS_RING,
        )}
      >
        {children}
      </button>
    </div>
  )
}

// What the row's "Excl" marker says on hover. It names the cause (the
// repository's own .gitattributes) and the consequence (no counts), and says
// the file is still readable here, because "excluded" on its own reads as
// "dux cannot show you this".
const DIFF_EXCLUDED_HINT =
  "This repository excludes this file from diffs (-diff in .gitattributes), so git reports no line counts for it. Opening it still shows the diff."

interface FileRowProps {
  file: ChangedFileView
  action: "stage" | "unstage"
  sessionId: string
  selected: boolean
  onToggleSelected: (path: string) => void
  onOpenDiff: (path: string) => void
  // 0 for a row of the listing itself, one more per expanded folder above it.
  depth: number
  // A folded folder whose contents are showing under it.
  expanded: boolean
  // Expand or collapse this folder row; absent for a row that cannot expand.
  onToggleExpand?: (file: ChangedFileView) => void
  // The id of the container holding this folder's contents, which its toggle
  // names in aria-controls.
  contentsId?: string
  // Why the last quiet refresh of this expanded folder failed, or null.
  refreshError?: string | null
  // Ask for this folder's contents again.
  onRetryFolder?: (file: ChangedFileView) => void
  // Its place among the treegrid's rows and whether it is the tab stop, as
  // two primitives so the memoized row skips a render that moved neither.
  rowIndex: number
  active: boolean
}

// What makes an element a row of the treegrid's keyboard: its place among the
// rows, and whether it is the one row in the tab order (the roving tab stop).
// A row that is not the active one keeps its controls out of the tab order, so
// Tab enters the grid once, walks the active row's controls, and leaves.
interface GridRowProps {
  rowIndex: number
  active: boolean
}

// The row element's own attributes for a `GridRowProps`, and its focus ring.
function gridRowAttrs({ rowIndex, active }: GridRowProps) {
  return { "aria-rowindex": rowIndex, tabIndex: active ? 0 : -1 }
}
const ROW_FOCUS = cn("border border-transparent", FOCUS_RING)

// How far each level of an expanded folder steps its rows in.
const DEPTH_INDENT_REM = 1
function depthIndent(depth: number): React.CSSProperties | undefined {
  return depth > 0 ? { paddingLeft: `${0.25 + depth * DEPTH_INDENT_REM}rem` } : undefined
}

// Memoized: the list re-renders on every scroll step and selection change, and
// a row whose file, selection and handlers did not move has nothing to redraw.
const FileRow = memo(function FileRow({
  file,
  action,
  sessionId,
  selected,
  onToggleSelected,
  onOpenDiff,
  depth,
  expanded,
  onToggleExpand,
  contentsId,
  refreshError = null,
  onRetryFolder,
  rowIndex,
  active,
}: FileRowProps) {
  const grid: GridRowProps = { rowIndex, active }
  const inTabOrder = active
  const { kind } = fileStatusMeta(file.status)
  const [busy, setBusy] = useState(false)
  // The menu's content mounts on the first open and stays mounted after, so
  // its closing animation still plays: a row nobody opens a menu on pays for a
  // trigger only, not for a popup tree and its media subscription.
  const [menuMounted, setMenuMounted] = useState(false)

  async function runAction() {
    setBusy(true)
    try {
      if (action === "stage") {
        const report = await git.stage(sessionId, file.path)
        // A folder is staged without the repositories inside it; the rows
        // moving cannot say that, so a message does.
        const notice = leftOutNotice(report)
        if (notice) notifyInfo(notice)
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
      row: file,
    })
  }

  // A folded folder has no diff: opening one would read a directory as a file.
  // Its name and count are its expand toggle instead, which lists what is
  // inside it under it; a repository of its own is not looked inside.
  const folderCount = folderCountLabel(file)
  const expandable = onToggleExpand !== undefined && isExpandable(file)
  // The row menu's items, decided before the trigger is: Edit (desktop only,
  // and never for a deleted file or a folder), stage or unstage, and discard.
  const showEdit = kind !== "deleted" && folderCount === null
  const showStageAction = action === "unstage" || stageActsOn(file)
  const showDiscard = action === "stage" && discardActsOn(file)
  // Edit alone would leave a phone an empty menu (it is desktop only), so it
  // never makes the trigger exist by itself.
  const hasMenuItems = showStageAction || showDiscard
  const displayPath = folderCount === null ? file.path : `${file.path}/`

  return (
    <div
      role="row"
      // Headings are level 1, so a section's rows start at 2.
      aria-level={depth + 2}
      aria-expanded={expandable ? expanded : undefined}
      {...gridRowAttrs(grid)}
      className={cn(
        "group flex items-center gap-2 rounded px-1 py-1 hover:bg-muted max-md:min-h-11",
        ROW_FOCUS,
        folderCount === null && "cursor-pointer",
      )}
      style={depthIndent(depth)}
      onClick={folderCount === null ? () => onOpenDiff(file.path) : undefined}
    >
      {/* Leading slot: the status marker, which becomes the selection checkbox
        * on hover, on focus of the checkbox, or while the row is checked. */}
      <StatusSlot
        status={file.status}
        path={file.path}
        selected={selected}
        onToggleSelected={onToggleSelected}
        inTabOrder={inTabOrder}
      />

      {/* Path and counts share one baseline container: their line boxes differ,
        * so under the row's items-center the digits read as superscript. On a
        * folder that can expand, that container is the toggle: a real button
        * saying whether its contents show and which container holds them, and
        * on touch it fills the row's 44px height. */}
      <ExpandToggle
        expandable={expandable}
        expanded={expanded}
        contentsId={contentsId}
        inTabOrder={inTabOrder}
        onToggle={() => onToggleExpand?.(file)}
      >
        {expandable && (
          <ChevronRight
            aria-hidden
            className={cn(
              "size-3.5 shrink-0 self-center text-muted-foreground transition-transform motion-reduce:transition-none",
              expanded && "rotate-90",
            )}
          />
        )}
        {/* Long paths ellipsize at the start so the filename stays visible. A
          * row inside an expanded folder names only itself: the folder it
          * hangs from is the row above it. */}
        <StartTruncatedText
          text={depth > 0 ? leafName(displayPath) : displayPath}
          className="flex-1 font-mono text-sm text-foreground"
        />

        {folderCount !== null && (
          <span className="shrink-0 font-mono text-xs text-muted-foreground">
            {folderCount}
          </span>
        )}

        {/* A file the repository excludes from diffs has no counts to show,
          * and saying nothing there would read as "changed nothing". It is not
          * binary: the diff viewer opens it, so it gets its own quiet marker
          * in the counts slot rather than the binary treatment. */}
        {file.diff_excluded && (
          <SimpleTooltip content={DIFF_EXCLUDED_HINT}>
            <span className="shrink-0 font-mono text-xs text-dux-diff-excluded">
              Excl
            </span>
          </SimpleTooltip>
        )}

        {/* Additions and deletions, coloured to match the diff viewer's gutter.
          * Binary files report none. */}
        {!file.binary && !file.diff_excluded && (file.additions > 0 || file.deletions > 0) && (
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
      </ExpandToggle>

      {/* A quiet refresh of this expanded folder failed: its rows are the ones
        * from before, which the row says quietly, with the reason on hover and
        * a way to ask again. A 44px target on touch, like every row control. */}
      {refreshError !== null && onRetryFolder && (
        <div role="gridcell" className="shrink-0">
          <SimpleTooltip content={refreshError}>
            <Button
              variant="ghost"
              size="sm"
              tabIndex={inTabOrder ? undefined : -1}
              className="h-7 shrink-0 px-2 text-xs text-muted-foreground max-md:min-h-11 pointer-coarse:min-h-11"
              onClick={(event) => {
                event.stopPropagation()
                onRetryFolder(file)
              }}
            >
              <TriangleAlert />
              Couldn't refresh · Retry
            </Button>
          </SimpleTooltip>
        </div>
      )}

      {/* The menu's items are computed first, and a row with none gets no
        * trigger at all: a menu that opens empty is a control that does
        * nothing (a worktree of this repository, a folder holding only
        * repositories). Such a row's label already says what it is. */}
      {hasMenuItems && (
      /* The wrapper consumes no width until the row is hovered, the menu is
        * open, or an action is in flight (aria-busy, so the spinner outlives the
        * menu), so the path and counts otherwise use the full row. Always
        * visible on touch at a 44px target. The stopPropagation keeps clicks on
        * the trigger and on the portaled menu items off the row's open-diff
        * handler, which React routes through this ancestor. */
      <div
        role="gridcell"
        className={cn(
          "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none md:max-w-0 md:opacity-0 md:group-hover:max-w-10 md:group-hover:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100 md:has-[[aria-busy=true]]:max-w-10 md:has-[[aria-busy=true]]:opacity-100",
          ALWAYS_REVEALED_ON_TOUCH,
        )}
        onClick={(e) => e.stopPropagation()}
      >
        <DropdownMenu
          onOpenChange={(open) => {
            if (open) setMenuMounted(true)
          }}
        >
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                disabled={busy}
                aria-busy={busy}
                tabIndex={inTabOrder ? undefined : -1}
                aria-label={`Actions for ${file.path}`}
                className="shrink-0 max-md:size-11"
              />
            }
          >
            {busy ? <Loader2 className="motion-safe:animate-spin" /> : <Ellipsis />}
          </DropdownMenuTrigger>
          {menuMounted ? (
            <DropdownMenuContent side="bottom" align="end">
              {/* Open in editor, desktop only (Monaco is poor on touch). Skipped
                  for deleted files (nothing on disk to edit) and for a folded
                  folder (a directory is not a file to edit). */}
              {showEdit && (
                <DropdownMenuItem
                  className="hidden md:flex"
                  onClick={() => openEditor(agentRoot(sessionId), file.path)}
                >
                  <Pencil />
                  Edit
                </DropdownMenuItem>
              )}
              {showStageAction && (
                <DropdownMenuItem onClick={() => void runAction()}>
                  {action === "stage" ? <Plus /> : <Minus />}
                  {action === "stage" ? "Stage" : "Unstage"}
                </DropdownMenuItem>
              )}
              {/* Discard, on unstaged rows only. Destructive, so the trailing "…"
                * and the confirm dialog carry the danger and the item itself
                * stays neutral. */}
              {showDiscard && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onClick={runDiscard}>
                    <Undo2 />
                    Discard…
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          ) : null}
        </DropdownMenu>
      </div>
      )}
    </div>
  )
})

interface GroupHeaderProps {
  heading: string
  // The filtered group size, the rows listed beneath this heading.
  shown: number
  // The unfiltered group size, so the badge can show "N of M" while a search is
  // active. Equal to `shown` when nothing is filtered out.
  total: number
  // Summed over the filtered set, so the recap describes exactly the rows
  // visible beneath it.
  recap: ChangedFilesRecap
  filtering: boolean
  open: boolean
  // The id of the container holding this section's rows.
  controls: string
  onToggleOpen: () => void
  // Out of the tab order unless its row is the active one.
  inTabOrder: boolean
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
  if (recap.diffExcludedCount > 0) {
    parts.push(formatRegularCount(recap.diffExcludedCount, "excluded file"))
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
  const { additions, deletions, binaryCount, diffExcludedCount } = recap
  const hasLines = additions > 0 || deletions > 0
  // Nothing to say: an empty set, or one whose files changed no lines and are
  // neither binary nor excluded from diffs (a mode change, an empty new file).
  // No "+0 −0".
  if (!hasLines && binaryCount === 0 && diffExcludedCount === 0) return null

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
          {groupDigits(binaryCount)} bin
        </span>
      )}
      {diffExcludedCount > 0 && (
        <span className="text-dux-diff-excluded">
          {hasLines || binaryCount > 0 ? " · " : ""}
          {groupDigits(diffExcludedCount)} excl
        </span>
      )}
    </span>
  )
}

// A section's heading, which folds its rows away. The rows are not its
// children: the list is one flat window over both sections, so the heading is a
// disclosure button that says whether its rows are showing.
function GroupHeader({
  heading,
  shown,
  total,
  recap,
  filtering,
  open,
  controls,
  onToggleOpen,
  inTabOrder,
}: GroupHeaderProps) {
  return (
    // No checkbox here: the whole-list selection is the bulk bar's Select all /
    // Select none, which spans both sections at once.
    <button
      type="button"
      aria-expanded={open}
      aria-controls={controls}
      tabIndex={inTabOrder ? undefined : -1}
      onClick={onToggleOpen}
      className="flex w-full items-center gap-2 rounded px-1 py-1 text-sm font-medium outline-none hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 max-md:min-h-11"
    >
      <span className="flex-1 text-left">{heading}</span>
      <ChangesRecap scope={heading} recap={recap} />
      <Badge variant="secondary">
        {filtering ? `${groupDigits(shown)} of ${groupDigits(total)}` : groupDigits(shown)}
      </Badge>
    </button>
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
// It matches the `⋯` on variant as well as geometry. It navigates rather than
// acts, and the tenet would let it be quieter, but a ghost glyph beside an
// outlined square read as a decoration rather than a control, so the two share
// the header's one outline treatment.
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
    variant: "outline",
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
  // Files, not rows, per section: a checked folder counts what is inside it.
  // `discard` leaves out the rows a delete would not act on.
  counts: { staged: number; unstaged: number; discard: number }
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

// Why a bulk verb is disabled when every selected row is one it would leave
// out, rather than a click that silently does nothing.
const NOTHING_TO_STAGE =
  "Nothing selected can be staged: a worktree of this repository, and a folder holding only repositories, are left out of a stage."
const NOTHING_TO_DISCARD =
  "Nothing selected can be discarded: a worktree of this repository, and a folder holding only repositories, have nothing a delete would remove."

// A disabled button takes no pointer events, so its explanation hangs off a
// focusable wrapper around it, through the shared tooltip. With nothing to
// explain the button renders bare.
function ExplainedWhenIdle({
  why,
  children,
}: {
  why: string | null
  children: React.ReactElement
}) {
  if (why === null) return children
  return (
    <SimpleTooltip content={why}>
      <span tabIndex={0} className="inline-flex">
        {children}
      </span>
    </SimpleTooltip>
  )
}

function BulkToolbar({
  selected,
  counts,
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
        <ExplainedWhenIdle
          why={counts.unstaged === 0 ? NOTHING_TO_STAGE : null}
        >
          <Button
            variant="outline"
            className={BULK_CONTROL}
            disabled={busy !== null || counts.unstaged === 0}
            aria-busy={busy === "stage"}
            onClick={() => onRunBulk("stage")}
          >
            <BusyGlyph busy={busy === "stage"} />
            {busy !== "stage" ? <Plus /> : null}
            Stage {groupDigits(counts.unstaged)}
          </Button>
        </ExplainedWhenIdle>
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
          Unstage {groupDigits(counts.staged)}
        </Button>
      ) : null}
      {selected.unstaged.size > 0 ? (
        <ExplainedWhenIdle
          why={counts.discard === 0 ? NOTHING_TO_DISCARD : null}
        >
          <Button
            variant="outline"
            className={BULK_CONTROL}
            disabled={busy !== null || counts.discard === 0}
            aria-busy={busy === "discard"}
            onClick={onDiscard}
          >
            <BusyGlyph busy={busy === "discard"} />
            {busy !== "discard" ? <Undo2 /> : null}
            Discard {groupDigits(counts.discard)}…
          </Button>
        </ExplainedWhenIdle>
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

// Rows mounted beyond each edge of the viewport, so a fast scroll or a Tab to
// the next row lands on something already there.
const OVERSCAN = 10
// The list's own inset (`p-3`), which sits between the viewport's scroll
// position and the first item.
const LIST_PADDING = 12
const ITEM_KINDS: ChangesItemKind[] = [
  "header",
  "row",
  "loading",
  "failed",
  "empty",
  "separator",
]
// The space after each kind of item. It lives on the positioned holder so the
// measured height carries it and the stacking needs no gap of its own.
const ITEM_SPACING: Record<ChangesItemKind, string> = {
  header: "pb-1",
  row: "pb-0.5",
  loading: "pb-0.5",
  failed: "pb-0.5",
  empty: "pb-0.5",
  separator: "py-1",
}
// Desktop heights with a fine pointer, spacing included, standing in until the
// first item of each kind is measured.
const DEFAULT_ITEM_HEIGHTS: ChangesItemHeights = {
  header: 32,
  row: 42,
  loading: 30,
  failed: 42,
  empty: 30,
  separator: 9,
}

// The row under an expanded folder that turned out to hold nothing git lists
// (its files went between the listing and the answer).
function EmptyRow({ depth, grid }: { depth: number; grid: GridRowProps }) {
  return (
    <div
      role="row"
      aria-level={depth + 2}
      {...gridRowAttrs(grid)}
      className={cn(
        "flex items-center gap-2 rounded px-1 py-1 text-sm text-muted-foreground",
        ROW_FOCUS,
      )}
      style={depthIndent(depth)}
    >
      <div role="gridcell">Nothing git lists here anymore.</div>
    </div>
  )
}

// The row standing in for an expanded folder's contents while they are asked
// for. It says so in words, and a screen reader hears it once.
function LoadingRow({ depth, grid }: { depth: number; grid: GridRowProps }) {
  return (
    <div
      role="row"
      aria-level={depth + 2}
      {...gridRowAttrs(grid)}
      className={cn(
        "flex items-center gap-2 rounded px-1 py-1 text-sm text-muted-foreground",
        ROW_FOCUS,
      )}
      style={depthIndent(depth)}
    >
      <div role="gridcell" className="flex items-center gap-2">
        <Loader2 aria-hidden className="size-4 shrink-0 motion-safe:animate-spin" />
        <span role="status">Loading…</span>
      </div>
    </div>
  )
}

// The row standing in for an expanded folder's contents that could not be
// listed: why, and a way to ask again.
function FailedRow({
  depth,
  message,
  onRetry,
  grid,
}: {
  depth: number
  message: string
  onRetry: () => void
  grid: GridRowProps
}) {
  return (
    <div
      role="row"
      aria-level={depth + 2}
      {...gridRowAttrs(grid)}
      className={cn(
        "flex items-center gap-2 rounded px-1 py-1 text-sm max-md:min-h-11",
        ROW_FOCUS,
      )}
      style={depthIndent(depth)}
    >
      {/* The message is the whole sentence, the server's or the deadline's. */}
      <div role="gridcell" className="flex min-w-0 flex-1 items-center gap-2">
        <TriangleAlert aria-hidden className="size-4 shrink-0 text-destructive" />
        <span className="min-w-0 flex-1 text-muted-foreground">{message}</span>
      </div>
      <div role="gridcell" className="shrink-0">
        <Button
          variant="outline"
          size="sm"
          tabIndex={grid.active ? undefined : -1}
          className="h-8 max-md:h-11 pointer-coarse:min-h-11"
          onClick={onRetry}
        >
          <RefreshCw />
          Retry
        </Button>
      </div>
    </div>
  )
}

interface ChangesListProps {
  // The agent's expanded folders, whose contents are listed under them.
  expansions: Expansions
  changed: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  filtered: { staged: ChangedFileView[]; unstaged: ChangedFileView[] }
  recap: { staged: ChangedFilesRecap; unstaged: ChangedFilesRecap }
  selected: ChangedFileSelection
  sessionId: string
  query: string
  filtering: boolean
  branchGit: boolean
  onToggle: (section: "staged" | "unstaged", path: string) => void
  // Expand a folded folder row, or collapse it.
  onToggleFolder: (section: "staged" | "unstaged", file: ChangedFileView) => void
}

function ChangesList({
  expansions,
  changed,
  filtered,
  recap,
  selected,
  sessionId,
  query,
  filtering,
  branchGit,
  onToggle,
  onToggleFolder,
}: ChangesListProps) {
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0
  const hasMatches = filtered.staged.length > 0 || filtered.unstaged.length > 0
  // Stable per session, like the toggles below, so the memoized rows skip a
  // re-render of the list that did not touch them.
  const openDiff = useCallback(
    (path: string) => openEditor(agentRoot(sessionId), path, "diff"),
    [sessionId],
  )
  const toggleStaged = useCallback(
    (path: string) => onToggle("staged", path),
    [onToggle],
  )
  const toggleUnstaged = useCallback(
    (path: string) => onToggle("unstaged", path),
    [onToggle],
  )
  const retryStaged = useCallback(
    (file: ChangedFileView) => retryFolderChildren(sessionId, "staged", file.path),
    [sessionId],
  )
  const retryUnstaged = useCallback(
    (file: ChangedFileView) => retryFolderChildren(sessionId, "unstaged", file.path),
    [sessionId],
  )
  const expandStaged = useCallback(
    (file: ChangedFileView) => onToggleFolder("staged", file),
    [onToggleFolder],
  )
  const expandUnstaged = useCallback(
    (file: ChangedFileView) => onToggleFolder("unstaged", file),
    [onToggleFolder],
  )
  const [open, setOpen] = useState<Record<ChangesSection, boolean>>({
    staged: true,
    unstaged: true,
  })
  const toggleOpen = (section: ChangesSection) =>
    setOpen((previous) => ({ ...previous, [section]: !previous[section] }))

  const items = useMemo(
    () =>
      buildChangesItems({
        staged: { files: filtered.staged, open: open.staged },
        unstaged: { files: filtered.unstaged, open: open.unstaged },
        expansions,
      }),
    [filtered, open, expansions],
  )

  // Rows are windowed against the pane's own ScrollArea, the approach the
  // editor's file tree takes: only what is near the viewport is mounted, so a
  // worktree with tens of thousands of untracked files costs a screenful of
  // rows. Item heights are measured from the DOM rather than hard-coded,
  // because rows and headings grow on a phone and under a coarse pointer
  // through CSS alone; until one of each is mounted the defaults stand in.
  const [heights, setHeights] = useState<ChangesItemHeights>(DEFAULT_ITEM_HEIGHTS)
  const offsets = useMemo(() => layoutChangesItems(items, heights), [items, heights])
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null)
  // The same element for the keyboard's scrolling, which writes to it.
  const viewportRef = useRef<HTMLDivElement | null>(null)
  const attachViewport = useCallback((element: HTMLDivElement | null) => {
    viewportRef.current = element
    setViewportEl(element)
  }, [])
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(400)
  const [viewportWidth, setViewportWidth] = useState(0)
  const listRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!viewportEl) return
    // ResizeObserver delivers an initial notification on observe(), so this
    // both seeds the height and tracks later resizes.
    const observer = new ResizeObserver(() => {
      setViewportHeight(viewportEl.clientHeight)
      setViewportWidth(viewportEl.clientWidth)
    })
    observer.observe(viewportEl)
    return () => observer.disconnect()
  }, [viewportEl])

  // Re-measured when the items change and when the pane's width does, which is
  // what crosses the phone breakpoint and resizes rows and headings. One read
  // per kind, and a state update only when a height truly moved.
  useLayoutEffect(() => {
    const list = listRef.current
    if (!list) return
    let next: ChangesItemHeights | null = null
    for (const kind of ITEM_KINDS) {
      const el = list.querySelector<HTMLElement>(`[data-item-kind="${kind}"]`)
      const measured = el?.offsetHeight ?? 0
      if (measured > 0 && measured !== (next ?? heights)[kind]) {
        next = { ...(next ?? heights), [kind]: measured }
      }
    }
    if (next) setHeights(next)
  }, [items, heights, viewportWidth])

  // The item holding keyboard focus stays mounted however far it is scrolled
  // away, so wheel-scrolling past a focused checkbox never drops focus to the
  // page and loses the reader's place in the Tab order.
  const [focusedKey, setFocusedKey] = useState<string | null>(null)
  const pinned = useMemo(() => {
    if (focusedKey === null) return null
    const index = items.findIndex((item) => item.key === focusedKey)
    return index >= 0 ? index : null
  }, [items, focusedKey])
  const indices = visibleChangesIndices(
    offsets,
    scrollTop - LIST_PADDING,
    viewportHeight,
    OVERSCAN,
    pinned,
  )

  // The treegrid's keyboard (APG, row focus). Every item but the separator is
  // a row, numbered for aria-rowindex; one of them, the active row, is the
  // grid's tab stop, and the arrows move it.
  const rowItems = useMemo(
    () => items.flatMap((item, index) => (item.kind === "separator" ? [] : [index])),
    [items],
  )
  const rowNumber = useMemo(() => {
    const numbers = new Map<number, number>()
    rowItems.forEach((index, position) => numbers.set(index, position + 1))
    return numbers
  }, [rowItems])
  const [activeKey, setActiveKey] = useState<string | null>(null)
  const activeIndex = useMemo(() => {
    const found = activeKey === null ? -1 : items.findIndex((item) => item.key === activeKey)
    return found >= 0 && items[found]!.kind !== "separator" ? found : (rowItems[0] ?? -1)
  }, [items, activeKey, rowItems])
  const gridFor = (index: number): GridRowProps => ({
    rowIndex: rowNumber.get(index) ?? 0,
    active: index === activeIndex,
  })
  // A row the keyboard moved to may be outside the window: it is pinned (so it
  // mounts wherever it is), scrolled into view, and focused once it is there.
  const [pendingFocus, setPendingFocus] = useState<string | null>(null)
  const focusItem = (index: number) => {
    const item = items[index]
    if (!item) return
    setActiveKey(item.key)
    setFocusedKey(item.key)
    const top = offsets[index]! + LIST_PADDING
    const bottom = offsets[index + 1]! + LIST_PADDING
    let nextTop: number | null = null
    if (top < scrollTop) nextTop = top
    else if (bottom > scrollTop + viewportHeight) nextTop = Math.max(0, bottom - viewportHeight)
    if (nextTop !== null) {
      if (viewportRef.current) viewportRef.current.scrollTop = nextTop
      setScrollTop(nextTop)
    }
    setPendingFocus(item.key)
  }
  useLayoutEffect(() => {
    if (pendingFocus === null) return
    const holder = [...(listRef.current?.querySelectorAll<HTMLElement>("[data-item-key]") ?? [])].find(
      (element) => element.dataset.itemKey === pendingFocus,
    )
    const row = holder?.querySelector<HTMLElement>('[role="row"]')
    if (!row) return
    row.focus()
    setPendingFocus(null)
  }, [pendingFocus, focusedKey, scrollTop, items])

  // What the keys do on a focused row. Controls inside a row keep their own
  // keys: this answers only when the row itself holds focus.
  const onGridKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement
    if (target.getAttribute("role") !== "row") return
    const key = (target.closest("[data-item-key]") as HTMLElement | null)?.dataset.itemKey
    const index = items.findIndex((item) => item.key === key)
    if (index < 0) return
    const item = items[index]!
    const position = rowItems.indexOf(index)
    const depthOf = (at: number) => {
      const other = items[at]!
      return other.kind === "header" || other.kind === "separator" ? -1 : other.depth
    }
    // The row this one hangs from: the folder above it, or its section heading.
    const parentOf = (at: number) => {
      const depth = depthOf(at)
      for (let above = at - 1; above >= 0; above -= 1) {
        const other = items[above]!
        if (other.kind === "header") return above
        if (other.kind === "row" && other.depth < depth) return above
      }
      return -1
    }
    let handled = true
    switch (event.key) {
      case "ArrowDown":
        if (position + 1 < rowItems.length) focusItem(rowItems[position + 1]!)
        break
      case "ArrowUp":
        if (position > 0) focusItem(rowItems[position - 1]!)
        break
      case "Home":
        if (rowItems.length > 0) focusItem(rowItems[0]!)
        break
      case "End":
        if (rowItems.length > 0) focusItem(rowItems[rowItems.length - 1]!)
        break
      case "ArrowRight": {
        const next = rowItems[position + 1]
        if (item.kind === "header") {
          if (!open[item.section]) toggleOpen(item.section)
          else if (next !== undefined && items[next]!.kind !== "header") focusItem(next)
        } else if (item.kind === "row" && isExpandable(item.file)) {
          if (!item.expanded) onToggleFolder(item.section, item.file)
          else if (next !== undefined && depthOf(next) > item.depth) focusItem(next)
        }
        break
      }
      case "ArrowLeft":
        if (item.kind === "header") {
          if (open[item.section]) toggleOpen(item.section)
        } else if (item.kind === "row" && item.expanded) {
          onToggleFolder(item.section, item.file)
        } else {
          const parent = parentOf(index)
          if (parent >= 0) focusItem(parent)
        }
        break
      case "Enter":
        if (item.kind === "header") toggleOpen(item.section)
        else if (item.kind === "failed") retryFolderChildren(sessionId, item.section, item.dir)
        else if (item.kind === "row") {
          if (isExpandable(item.file)) onToggleFolder(item.section, item.file)
          else if (folderCountLabel(item.file) === null) openDiff(item.file.path)
        }
        break
      case " ":
        if (item.kind === "header") toggleOpen(item.section)
        else if (item.kind === "row") onToggle(item.section, item.file.path)
        break
      default:
        handled = false
    }
    if (handled) event.preventDefault()
  }

  // The list's outline in reading order: each heading, the container of that
  // section's rows (while it is open), and the separator. Walked once per
  // change of the items, never per render.
  const structure = useMemo(() => changesListStructure(items), [items])
  // Each section's rows as ranges and folder containers, built once per change
  // of the items rather than per scroll step.
  const rowTrees = useMemo(() => {
    const trees = new Map<number, ChangesRowPart[]>()
    for (const part of structure) {
      if (part.kind === "rows") trees.set(part.first, changesRowTree(items, part.first, part.last))
    }
    return trees
  }, [items, structure])
  const baseId = useId()
  const rowsId = (section: ChangesSection) => `${baseId}-${section}-rows`
  // An expanded folder's contents container, named from the folder row's key.
  const contentsId = (key: string) => `${baseId}-contents-${encodeURIComponent(key)}`

  // One positioned holder per mounted item, placed relative to `origin` (the
  // top of the container it sits in).
  const renderHolder = (index: number, origin: number) => {
    const item = items[index]!
    return (
      <div
        key={item.key}
        data-item-key={item.key}
        data-item-kind={item.kind}
        // Only positions the item; the row inside is what the treegrid holds,
        // and the separator is decoration.
        role="none"
        aria-hidden={item.kind === "separator" ? true : undefined}
        className={ITEM_SPACING[item.kind]}
        style={{
          position: "absolute",
          top: offsets[index]! - origin,
          left: 0,
          right: 0,
        }}
      >
        {renderItem(item, index)}
      </div>
    )
  }

  const renderItem = (item: ChangesListItem, index: number) => {
    if (item.kind === "separator") return <Separator />
    const section = item.section
    const grid = gridFor(index)
    if (item.kind === "header") {
      return (
        // A section's heading is a row of the treegrid, its toggle in a cell.
        <div
          role="row"
          aria-level={1}
          aria-expanded={open[section]}
          {...gridRowAttrs(grid)}
          className={cn("rounded", ROW_FOCUS)}
        >
          <div role="gridcell">
            <GroupHeader
              heading={section === "staged" ? "Staged" : "Unstaged"}
              // Files, not rows: a folded folder counts what is inside it.
              shown={recap[section].count}
              total={changed[section].reduce((sum, file) => sum + changedFileCount(file), 0)}
              recap={recap[section]}
              filtering={filtering}
              open={open[section]}
              controls={rowsId(section)}
              onToggleOpen={() => toggleOpen(section)}
              inTabOrder={grid.active}
            />
          </div>
        </div>
      )
    }
    if (item.kind === "loading") return <LoadingRow depth={item.depth} grid={grid} />
    if (item.kind === "empty") return <EmptyRow depth={item.depth} grid={grid} />
    if (item.kind === "failed") {
      return (
        <FailedRow
          depth={item.depth}
          grid={grid}
          message={item.message}
          onRetry={() => retryFolderChildren(sessionId, section, item.dir)}
        />
      )
    }
    return (
      <FileRow
        file={item.file}
        action={section === "staged" ? "unstage" : "stage"}
        sessionId={sessionId}
        selected={selected[section].has(item.file.path)}
        onToggleSelected={section === "staged" ? toggleStaged : toggleUnstaged}
        onOpenDiff={openDiff}
        depth={item.depth}
        expanded={item.expanded}
        refreshError={item.refreshError}
        onRetryFolder={section === "staged" ? retryStaged : retryUnstaged}
        onToggleExpand={section === "staged" ? expandStaged : expandUnstaged}
        contentsId={item.expanded ? contentsId(item.key) : undefined}
        rowIndex={grid.rowIndex}
        active={grid.active}
      />
    )
  }

  // A section's rows, windowed, with each expanded folder's contents in one
  // container of its own (the element its toggle's aria-controls names),
  // nested the way the folders are. A container spans its whole range, so it
  // exists whatever part of it is mounted. A render walks every part of the
  // tree and, for each run of rows, the mounted indices: its cost is the
  // mounted rows times the parts (the runs of rows and the expanded folders),
  // never the listing's length.
  const renderRowParts = (parts: ChangesRowPart[], origin: number): ReactElement[] =>
    parts.flatMap((part) => {
      if (part.kind === "items") {
        return indices
          .filter((index) => index >= part.first && index <= part.last)
          .map((index) => renderHolder(index, origin))
      }
      const top = offsets[part.first]!
      return [
        // A treegrid cannot nest rowgroups, so a folder's container only holds
        // its rows in place (and is what the toggle's aria-controls names);
        // the rows' levels say they are inside it.
        <div
          key={`contents:${part.key}`}
          id={contentsId(part.key)}
          role="none"
          style={{
            position: "absolute",
            top: top - origin,
            left: 0,
            right: 0,
            height: offsets[part.last + 1]! - top,
          }}
        >
          {renderRowParts(part.parts, top)}
        </div>,
      ]
    })

  return (
    <ScrollArea
      className="min-h-0 flex-1"
      viewportRef={attachViewport}
      onViewportScroll={(event) => {
        setScrollTop(event.currentTarget.scrollTop)
        // Tracked here too: cheap, and covers an inert ResizeObserver.
        setViewportHeight(event.currentTarget.clientHeight)
      }}
    >
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
        {items.length > 0 ? (
          // Rows here hold widgets (a checkbox, a toggle, a menu) and nest by
          // level, which the ARIA practices call a treegrid.
          <div
            ref={listRef}
            role="treegrid"
            aria-label="Changed files"
            aria-rowcount={rowItems.length}
            onKeyDown={onGridKeyDown}
            // The full height, so the scrollbar reflects the whole list.
            style={{ position: "relative", height: offsets[items.length] }}
            onFocus={(event) => {
              const holder = (event.target as HTMLElement).closest?.(
                "[data-item-key]",
              ) as HTMLElement | null
              // Focus inside a row's portaled menu has no holder in this DOM
              // subtree; the row keeps whatever pin it already had.
              if (holder && listRef.current?.contains(holder)) {
                setFocusedKey(holder.dataset.itemKey ?? null)
                // Wherever focus lands (a click into a row's checkbox, a Tab
                // into the grid), that row becomes the tab stop.
                if (holder.dataset.itemKind !== "separator") {
                  setActiveKey(holder.dataset.itemKey ?? null)
                }
              }
            }}
            onBlur={(event) => {
              const next = event.relatedTarget as HTMLElement | null
              if (next && listRef.current?.contains(next)) return
              // A row's ⋯ menu is portaled out of this subtree, so focus moving
              // into it looks like leaving the list. It is the row's own menu
              // (focus reaches a menu from its trigger), and unmounting the row
              // would take the open menu with it, so the pin stays. Focus
              // leaving that menu for anywhere else bubbles here through the
              // portal and clears the pin then.
              if (next?.closest?.('[data-slot="dropdown-menu-content"]')) return
              setFocusedKey(null)
            }}
          >
            {structure.map((part) => {
              if (part.kind !== "rows") {
                return indices.includes(part.index)
                  ? renderHolder(part.index, 0)
                  : null
              }
              // Each section's rows sit in one container, the element its
              // heading's `aria-controls` names. It spans the section's whole
              // range, so it exists whatever part of it is mounted.
              const top = offsets[part.first]!
              return (
                <div
                  key={`rows:${part.section}`}
                  id={rowsId(part.section)}
                  role="rowgroup"
                  aria-label={
                    part.section === "staged" ? "Staged files" : "Unstaged files"
                  }
                  style={{
                    position: "absolute",
                    top,
                    left: 0,
                    right: 0,
                    height: offsets[part.last + 1]! - top,
                  }}
                >
                  {renderRowParts(rowTrees.get(part.first) ?? [], top)}
                </div>
              )
            })}
          </div>
        ) : null}
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

// Memoized and subscribed selectively: the pane re-renders when its changes,
// its selected session or that session's record move, and not on the
// keystrokes, ticks and toasts that move the rest of the store. Its parents
// re-render on all of those, which with the pane's list was the whole tab.
export const ChangedFiles = memo(function ChangedFiles() {
  const changes = useDuxSelector(selectChanges)
  const selectedSessionId = useDuxSelector(selectSelectedSessionId)
  const selectedSession = useDuxSelector((state) =>
    state.spine?.sessions.find((session) => session.id === state.selectedSessionId),
  )
  // The hide-pane action is desktop-only: the mobile hub reaches Changes through
  // its own nav, so there's no panel to hide there.
  const isMobile = useIsMobile()
  // The agent's expanded folders, held outside the store for as long as the
  // page lives, so an expansion re-renders this pane and nothing else.
  const expansions = useExpansions(selectedSessionId ?? "")
  const controller = useChangedFilesController(selectedSessionId, changes, expansions)
  // A checked row inside a checked folder is already part of the folder.
  const discardPaths = useMemo(
    () => uncoveredPaths(controller.selected.unstaged),
    [controller.selected.unstaged],
  )
  // Expand a folder, or collapse it and uncheck everything under it, which is
  // no longer shown. Stable per agent, because every folder row holds it.
  const { uncheckUnder } = controller
  const toggleFolderRow = useCallback(
    (section: "staged" | "unstaged", file: ChangedFileView) => {
      if (!selectedSessionId) return
      if (expansionsFor(selectedSessionId).has(folderKey(section, file.path))) {
        uncheckUnder(section, file.path)
      }
      toggleFolder(selectedSessionId, section, file)
    },
    [selectedSessionId, uncheckUnder],
  )
  // Every new listing reconciles the expanded folders: one that is no longer a
  // folded row is forgotten, and one whose row moved is asked for again,
  // quietly, keeping what is on screen until the answer lands.
  const liveSlice = changes.sessionId === selectedSessionId ? changes : null
  useEffect(() => {
    if (!selectedSessionId || !liveSlice || liveSlice.phase !== "loaded") return
    reconcileFolders(selectedSessionId, liveSlice.staged, liveSlice.unstaged)
  }, [selectedSessionId, liveSlice])

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
    actionable,
    filtered,
    recap,
    query,
    filtering,
    selected,
    selectedCounts,
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
              counts={selectedCounts}
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
            expansions={expansions}
            changed={changed}
            filtered={filtered}
            recap={recap}
            selected={selected}
            sessionId={sessionId}
            query={query}
            filtering={filtering}
            branchGit={branchGit}
            onToggle={toggleOne}
            onToggleFolder={toggleFolderRow}
          />
        </CardContent>
      </Card>
      <ConfirmDiscardFilesDialog
        open={discarding}
        paths={discardPaths}
        unstaged={actionable.unstaged}
        onCancel={closeDiscardMany}
        onConfirm={(paths, confirmations) => void runDiscardMany(paths, confirmations)}
      />
    </>
  )
})

function selectChanges(state: DuxState): ChangesSlice {
  return state.changes
}

function selectSelectedSessionId(state: DuxState): string | null {
  return state.selectedSessionId
}
