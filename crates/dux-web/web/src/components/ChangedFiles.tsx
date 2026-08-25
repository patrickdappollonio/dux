import { useState } from "react"
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  Ellipsis,
  EllipsisVertical,
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
import { notifyError, notifySuccess, notifyWarning } from "@/lib/notify"
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
  filterChangedFiles,
  reconcileSelection,
  type ChangedFileSelection,
} from "@/lib/changedFiles"
import {
  forceRefreshChanges,
  openCommit,
  openDiscard,
  openEditor,
  refreshChanges,
  toggleChangesPane,
  useDux,
} from "@/lib/store"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { cn } from "@/lib/utils"
import type { ChangedFileView } from "@/lib/types"
import { agentRoot } from "@/lib/editorRoot"

const fileCount = (n: number) => `${n} file${n === 1 ? "" : "s"}`

// One height for every control in the bulk bar; they differ in width only.
const BULK_CONTROL = "h-9 max-md:h-11"

// The row's ONE leading slot. It holds the status marker and the selection
// checkbox stacked in the same box, so which of the two is showing can never
// shift the path sideways. Fixed on both axes for that reason.
//
// Mouse: a 20px box. The 14px glyph and the 16px checkbox both centre in it,
// and the checkbox's own click halo is suppressed (`after:hidden` below) so a
// near-miss lands on the row's open-diff click rather than on a target the
// user cannot see. A touchscreen that reports a FINE pointer (a pen tablet, a
// hybrid laptop) lands here too: a fingertip inside the 16px box ticks the row
// instead of opening the diff, which is the one direction that trade can go
// wrong, and the neighbour it can be missed onto is the row's own read-only
// diff click.
const STATUS_SLOT_MOUSE = "size-5"
// Touch: the slot IS the selection control, since a finger cannot hover, so it
// carries the 40px floor on BOTH axes. Its only neighbours are the row's path
// and the row itself, whose click opens a read-only diff: a stray tap there
// costs nothing and is undone by closing the diff.
const STATUS_SLOT_TOUCH = "size-11"

interface StatusSlotProps {
  status: string
  path: string
  selected: boolean
  onToggleSelected: (path: string) => void
}

// The status marker, which becomes the selection checkbox on hover, on
// keyboard focus of that checkbox, or while the row is checked.
//
// The checkbox is ALWAYS in the DOM and always focusable: the swap is opacity,
// never `display`, so a keyboard user can reach it on a row nobody is hovering.
// The marker keeps its `role="img"` label throughout for the same reason, so a
// screen reader still hears the status of a row whose checkbox is showing.
//
// The status word stays reachable because the tooltip sits on the SLOT rather
// than on the marker: hovering the slot reveals the checkbox and shows the
// status at the same time.
function StatusSlot({ status, path, selected, onToggleSelected }: StatusSlotProps) {
  const coarse = useIsCoarsePointer()
  const { label } = fileStatusMeta(status)
  // Same duration and easing as the row's trailing ellipsis wrapper, so the two
  // things a hover reveals on this row arrive together rather than at two
  // visibly different speeds.
  const reveal = "transition-opacity duration-200 ease-out motion-reduce:transition-none"
  // Keyboard focus of the CHECKBOX reveals it, never focus-within on the row.
  // focus-within also fires when the row's ellipsis menu closes and hands focus
  // back to its trigger, and when a mouse tick leaves the checkbox focused,
  // either of which would strand that one row showing a checkbox and no marker
  // until focus moved on. `:focus-visible` keeps it to the keyboard.
  const keyboard = "group-has-[[data-slot=checkbox]:focus-visible]:"

  return (
    // The status tooltip belongs to the SLOT, not to the marker inside it: the
    // marker is pointer-transparent and fades out on exactly the hover that
    // would have opened its tooltip, so a tooltip on the marker itself is dead.
    // Here it answers a hover or a keyboard focus of either state of the slot.
    <SimpleTooltip content={label}>
      {/* The click stops here: base-ui re-dispatches the root's click onto its
          hidden input and both bubble, so without this every tick would also
          open the diff. There is deliberately no shift-click range: the rows
          carry no keyboard model, and a range gesture needs one to be
          reachable at all. */}
      <div
        className={cn(
          "relative flex shrink-0 items-center justify-center",
          coarse ? STATUS_SLOT_TOUCH : STATUS_SLOT_MOUSE,
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
            // On touch the halo is the tap target and is grown to fill the slot
            // exactly (16px box + 14px each side = 44px). On a mouse it is
            // suppressed so it cannot reach past the slot into the path.
            coarse ? "after:-inset-3.5" : "after:hidden",
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

  // Discard is only offered on unstaged files (the "stage" action rows), mirroring
  // the TUI which blocks discarding staged files. An untracked file ("?") will be
  // deleted; a tracked one is restored — the dialog distinguishes them.
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
          on hover, on keyboard focus of the checkbox, or while the row is
          checked. */}
      <StatusSlot
        status={file.status}
        path={file.path}
        selected={selected}
        onToggleSelected={onToggleSelected}
      />

      {/* Path and counts share ONE baseline container. Their line boxes differ
          (text-sm against text-xs), so under the row's items-center the digits
          would centre inside the taller box and read as superscript. */}
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        {/* File path — monospace (it's a path/code identifier). Long paths
            ellipsize at the START (direction:rtl) so the filename at the end
            stays visible; text-left keeps short paths normally left-aligned.
            The path itself is wrapped in a <bdi> LTR isolate so a leading
            bidi-neutral character in a dotfile path (e.g. ".github/...") isn't
            reordered by the rtl container to the visual end — without it the
            leading "." renders stuck onto the end of the filename. */}
        <span className="min-w-0 flex-1 truncate text-left font-mono text-sm text-foreground [direction:rtl]">
          <bdi dir="ltr">{file.path}</bdi>
        </span>

        {/* Additions / deletions (text-only, skip for binary). Added lines
            green, removed lines red, matching the diff viewer's gutter
            coloring. */}
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

      {/* Row actions consolidated into a single ⋯ menu (like the sidebar's
          project/session rows). The wrapper consumes NO width until the row is
          hovered, the menu is open (trigger data-popup-open), or an action is in
          flight (trigger aria-busy — so the spinner stays visible after the menu
          closes) — its max-width animates open, so the path/counts use the full
          row otherwise. Always visible on touch at a ≥44px target. The
          stopPropagation keeps clicks on the trigger AND on the (portaled) menu
          items from bubbling to the row's open-diff onClick — React routes portal
          events through this React-tree ancestor. */}
      <div
        className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none md:max-w-0 md:opacity-0 md:group-hover:max-w-10 md:group-hover:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100 md:has-[[aria-busy=true]]:max-w-10 md:has-[[aria-busy=true]]:opacity-100"
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
  selected: Set<string>
  onToggleSelected: (path: string) => void
  onOpenDiff: (path: string) => void
}

function FileGroup({
  heading,
  files,
  total,
  filtering,
  action,
  sessionId,
  selected,
  onToggleSelected,
  onOpenDiff,
}: FileGroupProps) {
  const [open, setOpen] = useState(true)

  // Hide a group that's empty in the source. While filtering, a group that has
  // source files but no matches stays hidden too (the overall empty state below
  // covers the "no matches anywhere" case).
  if (files.length === 0) return null

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      {/* No checkbox here: the whole-list selection is the bulk bar's Select
          all / Select none, which spans both sections at once. */}
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

export function ChangedFiles() {
  const { changes, selectedSessionId, spine } = useDux()
  // The hide-pane action is desktop-only: the mobile hub reaches Changes through
  // its own nav, so there's no panel to hide there.
  const isMobile = useIsMobile()

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

  // Checked paths, per section, stored against the session they belong to so a
  // session switch reads as an empty selection without an effect. The state is
  // local: leaving the mobile Changes screen, or hiding the pane, unmounts this
  // component and drops the selection. Accepted.
  const [selection, setSelection] = useState<
    { sessionId: string } & ChangedFileSelection
  >({ sessionId: "", staged: new Set(), unstaged: new Set() })
  // Which verb is in flight, so its button can say so and no second request can
  // start behind it.
  const [busy, setBusy] = useState<"stage" | "unstage" | "discard" | null>(null)
  const [discarding, setDiscarding] = useState(false)

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

  // A STANDALONE agent whose folder is not a repository: quiet, and honest
  // about which quiet this is. The sentence is the SERVER's (it is the one that
  // consulted git, and the terminal UI says the same thing), so the two
  // surfaces cannot describe the same folder differently.
  //
  // Checked before the slice below, because the answer is already known: the
  // server never runs git in such a folder and stores an empty-but-successful
  // result for it, so waiting on the fetch phase would only trade this sentence
  // for a spinner and then for a bare "No changes". Running git there would
  // report "the repository is busy" once per poll, a lie about a folder that
  // simply has no repository.
  const selectedSession = spine?.sessions.find(
    (s) => s.id === selectedSessionId,
  )
  const quietReason = selectedSession
    ? changesQuietReason(selectedSession.workspace)
    : null
  // Whether this agent has a branch dux manages, which is what Push and Pull
  // act on. Defaults to true with no session resolved, so nothing disappears
  // from an ordinary agent's panel while the spine is still loading.
  const branchGit = selectedSession
    ? supportsBranchGit(selectedSession.workspace)
    : true
  if (quietReason) {
    const folder = selectedSession
      ? folderWorkspace(selectedSession.workspace)
      : null
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

  // Read the changed-files slice only when it belongs to THIS client's selection
  // (the slice tracks one session at a time). The slice's phase replaces the old
  // global-watch readiness check: a real request is in flight, an error
  // self-heals on the next event, and there is no cross-client clobber.
  const slice = changes.sessionId === selectedSessionId ? changes : null
  const phase = slice?.phase ?? "loading"

  // Loading window: a fetch is in flight (or the slice hasn't caught up to the
  // just-changed selection). Show a spinner, never another session's files.
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

  // The fetch failed (git lock/rebase 409, a server error, or the network). Show
  // an explicit error with a Refresh affordance. The poller's recovery event
  // also self-heals this without a click.
  if (phase === "error") {
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

  const changed = { staged: slice?.staged ?? [], unstaged: slice?.unstaged ?? [] }
  const hasChanges = changed.staged.length > 0 || changed.unstaged.length > 0

  const filtering = query.trim() !== ""
  const filteredStaged = filterChangedFiles(changed.staged, query)
  const filteredUnstaged = filterChangedFiles(changed.unstaged, query)
  const hasMatches = filteredStaged.length > 0 || filteredUnstaged.length > 0
  const showSeparator = filteredStaged.length > 0 && filteredUnstaged.length > 0

  // The honest selection: a path that left its section is no longer checked.
  // Derived at render, so a refresh keeps it truthful with no effect.
  const selected = reconcileSelection(
    selection.sessionId === selectedSessionId
      ? selection
      : { staged: new Set<string>(), unstaged: new Set<string>() },
    changed,
  )
  const anySelected = selected.staged.size > 0 || selected.unstaged.size > 0

  // The Select all / Select none universe: every row the FILTER currently
  // shows, across BOTH sections, which is the union of what the two
  // per-section select-alls used to cover. A collapsed section is still part of
  // it: collapsing hides rows from view, but the filter is what decides which
  // files the pane is talking about.
  const visibleStaged = filteredStaged.map((f) => f.path)
  const visibleUnstaged = filteredUnstaged.map((f) => f.path)
  const visibleCount = visibleStaged.length + visibleUnstaged.length
  const allVisibleChecked =
    visibleCount > 0 &&
    visibleStaged.every((p) => selected.staged.has(p)) &&
    visibleUnstaged.every((p) => selected.unstaged.has(p))
  // Narrowed once here so the async handlers below see a plain string.
  const sessionId: string = selectedSessionId

  const writeSelection = (next: ChangedFileSelection) =>
    setSelection({ sessionId: selectedSessionId, ...next })

  // Every edit reads the CURRENT sets rather than the ones this render closed
  // over: a request in flight resolves into a selection the user has meanwhile
  // changed, and writing the old sets back would untick what they just ticked.
  // A set belonging to another session reads as empty, the same rule the render
  // above applies.
  const editSelection = (
    mutate: (next: { staged: Set<string>; unstaged: Set<string> }) => void,
  ) =>
    setSelection((prev) => {
      const base =
        prev.sessionId === sessionId
          ? prev
          : { staged: new Set<string>(), unstaged: new Set<string>() }
      const next = {
        staged: new Set(base.staged),
        unstaged: new Set(base.unstaged),
      }
      mutate(next)
      return { sessionId, ...next }
    })

  function toggleOne(section: "staged" | "unstaged", path: string) {
    editSelection((next) => {
      if (next[section].has(path)) next[section].delete(path)
      else next[section].add(path)
    })
  }

  function dropActed(section: "staged" | "unstaged", paths: string[]) {
    editSelection((next) => {
      for (const path of paths) next[section].delete(path)
    })
  }

  // Select all / Select none over the visible universe. Unlike Clear, it only
  // touches the rows on screen: a checked file the filter hides keeps its tick,
  // so the bar stays up and the label flips back to "Select all".
  function toggleVisible() {
    const wanted = !allVisibleChecked
    editSelection((next) => {
      for (const path of visibleStaged) {
        if (wanted) next.staged.add(path)
        else next.staged.delete(path)
      }
      for (const path of visibleUnstaged) {
        if (wanted) next.unstaged.add(path)
        else next.unstaged.delete(path)
      }
    })
  }

  // One request, one toast. The acted paths leave the selection as soon as the
  // server has answered, so the bar cannot fire twice at files that have
  // already moved, whatever the broadcast does next.
  async function runBulk(verb: "stage" | "unstage") {
    const section = verb === "stage" ? "unstaged" : "staged"
    const paths = [...selected[section]]
    if (busy !== null || paths.length === 0) return
    setBusy(verb)
    try {
      const result =
        verb === "stage"
          ? await git.stageMany(sessionId, paths)
          : await git.unstageMany(sessionId, paths)
      dropActed(section, paths)
      const past = verb === "stage" ? "staged" : "unstaged"
      if (result.refused.length === 0) {
        notifySuccess(`${fileCount(result.done.length)} ${past}.`)
      } else {
        notifyWarning(
          `${fileCount(result.done.length)} ${past}. ${fileCount(
            result.refused.length,
          )} had already left the list, starting with ${result.refused[0]}.`,
        )
      }
    } catch (err) {
      notifyError(
        err instanceof Error ? err.message : `could not ${verb} the files`,
      )
    } finally {
      setBusy(null)
    }
  }

  // Discard runs file by file, so a refusal on one ("unstage it first") cannot
  // block the rest; the outcomes are aggregated into a single toast.
  async function runDiscardMany(paths: string[]) {
    setDiscarding(false)
    if (busy !== null || paths.length === 0) return
    setBusy("discard")
    try {
      const result = await git.discardMany(sessionId, paths)
      dropActed("unstaged", paths)
      if (result.failed.length === 0) {
        notifySuccess(`Discarded the changes to ${fileCount(result.done.length)}.`)
      } else if (result.done.length === 0) {
        notifyError(
          `Nothing was discarded. ${result.failed[0]!.path}: ${result.failed[0]!.message}`,
        )
      } else {
        notifyWarning(
          `Discarded the changes to ${fileCount(result.done.length)}. ${fileCount(
            result.failed.length,
          )} could not be discarded, starting with ${result.failed[0]!.path}: ${
            result.failed[0]!.message
          }`,
        )
      }
    } finally {
      setBusy(null)
    }
  }

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
                  <GitCommitVertical />
                  Commit…
                </DropdownMenuItem>
                {/* Push and Pull publish a BRANCH, which a standalone agent
                    does not have even when its folder is a real repository.
                    Absent rather than offered and refused on click, the same
                    rule the agent row's menu follows. Committing IS offered:
                    that is folder work, and the server allows it. */}
                {branchGit && (
                  <>
                    <DropdownMenuItem
                      onClick={() => {
                        if (!selectedSessionId) return
                        git
                          .push(selectedSessionId)
                          .catch((e) =>
                            notifyError(e instanceof Error ? e.message : "push failed")
                          )
                      }}
                    >
                      <ArrowUpFromLine />
                      Push
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onClick={() => {
                        if (!selectedSessionId) return
                        git
                          .pull(selectedSessionId)
                          .catch((e) =>
                            notifyError(e instanceof Error ? e.message : "pull failed")
                          )
                      }}
                    >
                      <ArrowDownToLine />
                      Pull
                    </DropdownMenuItem>
                  </>
                )}
                {/* Ask git again NOW. dux has no file watcher: it drops its
                    cached answer when one of its own git or editor routes
                    changes a file, or a dropped file lands in the worktree, and
                    anything else (a file the user changed from a terminal, an
                    agent writing in its worktree) only appears on the next poll. Deliberately `forceRefreshChanges`
                    and not the store's `refreshChanges`: that one only re-GETs,
                    and the server would answer from the same cache, so the item
                    would look like it worked and change nothing. No trailing
                    ellipsis: it opens nothing and needs no confirmation. The
                    store reports both the failure and the counts it found. */}
                <DropdownMenuItem
                  onClick={() => {
                    void forceRefreshChanges().catch((e) =>
                      notifyError(
                        e instanceof Error ? e.message : "refresh failed"
                      )
                    )
                  }}
                >
                  <RefreshCw />
                  Refresh changes
                </DropdownMenuItem>
                {/* Hide the Changes pane entirely (desktop only), mirroring the
                    TUI's remove-git-pane command. The persisted default lives in
                    config.ui.show_changes_pane, which Preferences also sets. */}
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

          {/* Bulk bar: present only while something is checked, one height
              token, one variant. The verbs carry their count because the count
              is data. It holds no ellipsis of its own: the header's is the
              pane's one surface-scoped menu. */}
          {anySelected && (
            <div
              role="toolbar"
              aria-label="Actions for the selected files"
              className="flex flex-wrap items-center gap-2 border-b p-2"
            >
              {selected.unstaged.size > 0 && (
                <Button
                  variant="outline"
                  className={BULK_CONTROL}
                  disabled={busy !== null}
                  aria-busy={busy === "stage"}
                  onClick={() => void runBulk("stage")}
                >
                  {busy === "stage" ? (
                    <Loader2 className="motion-safe:animate-spin" />
                  ) : (
                    <Plus />
                  )}
                  Stage {selected.unstaged.size}
                </Button>
              )}
              {selected.staged.size > 0 && (
                <Button
                  variant="outline"
                  className={BULK_CONTROL}
                  disabled={busy !== null}
                  aria-busy={busy === "unstage"}
                  onClick={() => void runBulk("unstage")}
                >
                  {busy === "unstage" ? (
                    <Loader2 className="motion-safe:animate-spin" />
                  ) : (
                    <Minus />
                  )}
                  Unstage {selected.staged.size}
                </Button>
              )}
              {selected.unstaged.size > 0 && (
                <Button
                  variant="outline"
                  className={BULK_CONTROL}
                  disabled={busy !== null}
                  aria-busy={busy === "discard"}
                  onClick={() => setDiscarding(true)}
                >
                  {busy === "discard" ? (
                    <Loader2 className="motion-safe:animate-spin" />
                  ) : (
                    <Undo2 />
                  )}
                  Discard {selected.unstaged.size}…
                </Button>
              )}
              {/* Select all / Select none. Absent rather than disabled when
                  the filter leaves nothing on screen: there is no universe for
                  it to name. Deliberately NOT disabled while a verb is in
                  flight: it is a selection control, and ticking stays possible
                  throughout, exactly like the row checkboxes. */}
              {visibleCount > 0 && (
                <Button
                  variant="outline"
                  className={BULK_CONTROL}
                  onClick={toggleVisible}
                >
                  {allVisibleChecked ? <Square /> : <SquareCheck />}
                  {allVisibleChecked ? "Select none" : "Select all"}
                </Button>
              )}
              {/* Clear empties the WHOLE selection, the rows the filter hides
                  included; Select none above only lets go of what is on
                  screen. */}
              <Button
                variant="outline"
                className={BULK_CONTROL}
                onClick={() =>
                  writeSelection({ staged: new Set(), unstaged: new Set() })
                }
              >
                Clear
              </Button>
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
                    <EmptyDescription>
                      {branchGit
                        ? "This worktree is clean."
                        : "This folder is clean."}
                    </EmptyDescription>
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
                selected={selected.staged}
                onToggleSelected={(path) => toggleOne("staged", path)}
                onOpenDiff={(path) => openEditor(agentRoot(selectedSessionId), path, "diff")}
              />

              {showSeparator && <Separator className="my-1" />}

              <FileGroup
                heading="Unstaged"
                files={filteredUnstaged}
                total={changed.unstaged.length}
                filtering={filtering}
                action="stage"
                sessionId={selectedSessionId}
                selected={selected.unstaged}
                onToggleSelected={(path) => toggleOne("unstaged", path)}
                onOpenDiff={(path) => openEditor(agentRoot(selectedSessionId), path, "diff")}
              />
            </div>
          </ScrollArea>
        </CardContent>
      </Card>

      {/* The selection is local state, so this confirm takes its target as a
          prop rather than through the store, unlike the single-file one. */}
      <ConfirmDiscardFilesDialog
        open={discarding}
        paths={[...selected.unstaged]}
        unstaged={changed.unstaged}
        onCancel={() => setDiscarding(false)}
        onConfirm={(paths) => void runDiscardMany(paths)}
      />
    </>
  )
}
